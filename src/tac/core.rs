use std::io::{self, IoSlice, Write};

use rayon::prelude::*;

/// Threshold for parallel processing (64MB).
/// Each benchmark invocation is a fresh process, so rayon pool init (~0.5-1ms)
/// is paid every time. For 10MB files, single-threaded memrchr scan (0.3ms) is
/// faster than rayon init + parallel scan. Only use parallelism for genuinely
/// large files where multi-core scanning pays off.
const PARALLEL_THRESHOLD: usize = 64 * 1024 * 1024;

/// Maximum IoSlice entries per write_vectored batch.
/// Matches Linux UIO_MAXIOV limit (1024).
const IOSLICE_BATCH_SIZE: usize = 1024;

/// Reverse records separated by a single byte.
/// For large data (>= 64MB): parallel forward SIMD scan + zero-copy IoSlice output.
/// For small data: single-threaded backward SIMD scan + IoSlice batches.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if data.len() >= PARALLEL_THRESHOLD {
        if !before {
            tac_bytes_after_contiguous(data, separator, out)
        } else {
            tac_bytes_before_contiguous(data, separator, out)
        }
    } else if !before {
        tac_bytes_after(data, separator, out)
    } else {
        tac_bytes_before(data, separator, out)
    }
}

/// Reverse records of an owned Vec. Delegates to tac_bytes.
pub fn tac_bytes_owned(
    data: &mut [u8],
    separator: u8,
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    tac_bytes(data, separator, before, out)
}

/// Collect multi-byte separator positions with pre-allocated Vec.
#[inline]
fn collect_positions_str(data: &[u8], separator: &[u8]) -> Vec<usize> {
    let estimated = data.len() / 40 + 64;
    let mut positions = Vec::with_capacity(estimated);
    for pos in memchr::memmem::find_iter(data, separator) {
        positions.push(pos);
    }
    positions
}

/// Parallel forward scan to collect all separator positions.
/// Splits data into chunks, each processed by a rayon worker thread.
/// Returns per-chunk position vectors in chunk order (positions are absolute).
fn parallel_scan_positions(data: &[u8], sep: u8) -> Vec<Vec<usize>> {
    let n_threads = rayon::current_num_threads().max(1);
    let chunk_size = (data.len() + n_threads - 1) / n_threads;

    (0..n_threads)
        .into_par_iter()
        .map(|i| {
            let start = i * chunk_size;
            if start >= data.len() {
                return Vec::new();
            }
            let end = (start + chunk_size).min(data.len());
            let chunk = &data[start..end];
            let estimated = chunk.len() / 40 + 64;
            let mut positions = Vec::with_capacity(estimated);
            for pos in memchr::memchr_iter(sep, chunk) {
                positions.push(start + pos);
            }
            positions
        })
        .collect()
}

/// Zero-copy parallel after-separator mode: parallel forward SIMD scan to collect
/// separator positions, then zero-copy reverse output via IoSlice batches pointing
/// directly into the input data. No output buffer allocation needed.
///
/// vs old contiguous-copy approach for 100MB:
/// - Old: alloc 100MB + parallel scan+copy (200MB bandwidth) + 1 write_all
/// - New: parallel scan only (100MB bandwidth) + ~2500 write_vectored (zero-copy)
/// Eliminates 100MB allocation and 100MB of copy bandwidth.
fn tac_bytes_after_contiguous(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let chunk_positions = parallel_scan_positions(data, sep);

    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(IOSLICE_BATCH_SIZE);
    let mut end = data.len();

    for positions in chunk_positions.iter().rev() {
        for &pos in positions.iter().rev() {
            let rec_start = pos + 1;
            if rec_start < end {
                slices.push(IoSlice::new(&data[rec_start..end]));
                if slices.len() >= IOSLICE_BATCH_SIZE {
                    write_all_vectored(out, &slices)?;
                    slices.clear();
                }
            }
            end = rec_start;
        }
    }

    if end > 0 {
        slices.push(IoSlice::new(&data[..end]));
    }
    if !slices.is_empty() {
        write_all_vectored(out, &slices)?;
    }
    Ok(())
}

/// Zero-copy parallel before-separator mode.
fn tac_bytes_before_contiguous(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let chunk_positions = parallel_scan_positions(data, sep);

    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(IOSLICE_BATCH_SIZE);
    let mut end = data.len();

    for positions in chunk_positions.iter().rev() {
        for &pos in positions.iter().rev() {
            if pos < end {
                slices.push(IoSlice::new(&data[pos..end]));
                if slices.len() >= IOSLICE_BATCH_SIZE {
                    write_all_vectored(out, &slices)?;
                    slices.clear();
                }
            }
            end = pos;
        }
    }

    if end > 0 {
        slices.push(IoSlice::new(&data[..end]));
    }
    if !slices.is_empty() {
        write_all_vectored(out, &slices)?;
    }
    Ok(())
}

/// Zero-copy after-separator mode: streaming IoSlice directly from input data.
/// No buffer allocation — scans backward and emits IoSlice batches of 1024.
fn tac_bytes_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(IOSLICE_BATCH_SIZE);
    let mut end = data.len();

    for pos in memchr::memrchr_iter(sep, data) {
        let rec_start = pos + 1;
        if rec_start < end {
            slices.push(IoSlice::new(&data[rec_start..end]));
            if slices.len() >= IOSLICE_BATCH_SIZE {
                write_all_vectored(out, &slices)?;
                slices.clear();
            }
        }
        end = rec_start;
    }

    if end > 0 {
        slices.push(IoSlice::new(&data[..end]));
    }
    if !slices.is_empty() {
        write_all_vectored(out, &slices)?;
    }
    Ok(())
}

/// Zero-copy before-separator mode: streaming IoSlice directly from input data.
fn tac_bytes_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(IOSLICE_BATCH_SIZE);
    let mut end = data.len();

    for pos in memchr::memrchr_iter(sep, data) {
        if pos < end {
            slices.push(IoSlice::new(&data[pos..end]));
            if slices.len() >= IOSLICE_BATCH_SIZE {
                write_all_vectored(out, &slices)?;
                slices.clear();
            }
        }
        end = pos;
    }

    if end > 0 {
        slices.push(IoSlice::new(&data[..end]));
    }
    if !slices.is_empty() {
        write_all_vectored(out, &slices)?;
    }
    Ok(())
}

/// Reverse records using a multi-byte string separator.
/// Uses SIMD-accelerated memmem + write_all output.
///
/// For single-byte separators, delegates to tac_bytes which uses memchr (faster).
pub fn tac_string_separator(
    data: &[u8],
    separator: &[u8],
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    if separator.len() == 1 {
        return tac_bytes(data, separator[0], before, out);
    }

    let sep_len = separator.len();

    if !before {
        tac_string_after(data, separator, sep_len, out)
    } else {
        tac_string_before(data, separator, sep_len, out)
    }
}

/// Multi-byte string separator, after mode. Uses writev batching.
fn tac_string_after(
    data: &[u8],
    separator: &[u8],
    sep_len: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let positions = collect_positions_str(data, separator);

    if positions.is_empty() {
        return out.write_all(data);
    }

    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(IOSLICE_BATCH_SIZE);
    let mut end = data.len();

    for &pos in positions.iter().rev() {
        let rec_start = pos + sep_len;
        if rec_start < end {
            slices.push(IoSlice::new(&data[rec_start..end]));
            if slices.len() >= IOSLICE_BATCH_SIZE {
                write_all_vectored(out, &slices)?;
                slices.clear();
            }
        }
        end = rec_start;
    }
    if end > 0 {
        slices.push(IoSlice::new(&data[..end]));
    }
    if !slices.is_empty() {
        write_all_vectored(out, &slices)?;
    }
    Ok(())
}

/// Multi-byte string separator, before mode. Uses writev batching.
fn tac_string_before(
    data: &[u8],
    separator: &[u8],
    _sep_len: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let positions = collect_positions_str(data, separator);

    if positions.is_empty() {
        return out.write_all(data);
    }

    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(IOSLICE_BATCH_SIZE);
    let mut end = data.len();

    for &pos in positions.iter().rev() {
        if pos < end {
            slices.push(IoSlice::new(&data[pos..end]));
            if slices.len() >= IOSLICE_BATCH_SIZE {
                write_all_vectored(out, &slices)?;
                slices.clear();
            }
        }
        end = pos;
    }
    if end > 0 {
        slices.push(IoSlice::new(&data[..end]));
    }
    if !slices.is_empty() {
        write_all_vectored(out, &slices)?;
    }
    Ok(())
}

/// Find regex matches using backward scanning, matching GNU tac's re_search behavior.
fn find_regex_matches_backward(data: &[u8], re: &regex::bytes::Regex) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    let mut past_end = data.len();

    while past_end > 0 {
        let buf = &data[..past_end];
        let mut found = false;

        let mut pos = past_end;
        while pos > 0 {
            pos -= 1;
            if let Some(m) = re.find_at(buf, pos) {
                if m.start() == pos {
                    matches.push((m.start(), m.end()));
                    past_end = m.start();
                    found = true;
                    break;
                }
            }
        }

        if !found {
            break;
        }
    }

    matches.reverse();
    matches
}

/// Reverse records using a regex separator.
/// Uses write_vectored for regex path (typically few large records).
pub fn tac_regex_separator(
    data: &[u8],
    pattern: &str,
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let re = match regex::bytes::Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid regex '{}': {}", pattern, e),
            ));
        }
    };

    let matches = find_regex_matches_backward(data, &re);

    if matches.is_empty() {
        out.write_all(data)?;
        return Ok(());
    }

    // For regex separators, use write_vectored since there are typically
    // few large records. Build all IoSlices at once and flush.
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(matches.len() + 2);

    if !before {
        let last_end = matches.last().unwrap().1;

        if last_end < data.len() {
            slices.push(IoSlice::new(&data[last_end..]));
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            slices.push(IoSlice::new(&data[rec_start..matches[i].1]));
        }
    } else {
        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let start = matches[i].0;
            let end = if i + 1 < matches.len() {
                matches[i + 1].0
            } else {
                data.len()
            };
            slices.push(IoSlice::new(&data[start..end]));
        }

        if matches[0].0 > 0 {
            slices.push(IoSlice::new(&data[..matches[0].0]));
        }
    }

    write_all_vectored(out, &slices)
}

/// Write all IoSlice entries, handling partial writes.
/// Hot path: single write_vectored succeeds fully (common on Linux pipes/files).
/// Cold path: partial write handled out-of-line to keep hot path tight.
#[inline(always)]
fn write_all_vectored(out: &mut impl Write, slices: &[IoSlice<'_>]) -> io::Result<()> {
    let total: usize = slices.iter().map(|s| s.len()).sum();
    let written = out.write_vectored(slices)?;
    if written >= total {
        return Ok(());
    }
    if written == 0 {
        return Err(io::Error::new(io::ErrorKind::WriteZero, "write zero"));
    }
    flush_vectored_slow(out, slices, written)
}

/// Handle partial write (cold path, never inlined).
#[cold]
#[inline(never)]
fn flush_vectored_slow(
    out: &mut impl Write,
    slices: &[IoSlice<'_>],
    mut skip: usize,
) -> io::Result<()> {
    for slice in slices {
        let len = slice.len();
        if skip >= len {
            skip -= len;
            continue;
        }
        out.write_all(&slice[skip..])?;
        skip = 0;
    }
    Ok(())
}
