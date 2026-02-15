use std::io::{self, IoSlice, Write};

use rayon::prelude::*;

/// Threshold for parallel processing (2MB).
/// Rayon thread pool initialization costs ~300µs on first use but is cached
/// after that. For files under 2MB that process in ~200-400µs, the overhead
/// dominates. Files 2MB+ benefit from parallel memchr scanning.
const PARALLEL_THRESHOLD: usize = 2 * 1024 * 1024;

/// Reverse records separated by a single byte.
/// Scans for separators with SIMD memchr, then outputs records in reverse
/// order directly from the input buffer. Expects the caller to provide
/// a buffered writer (e.g., BufWriter) for efficient syscall batching.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if data.len() >= PARALLEL_THRESHOLD {
        if !before {
            tac_bytes_after_parallel(data, separator, out)
        } else {
            tac_bytes_before_parallel(data, separator, out)
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

/// Collect separator positions with pre-allocated Vec.
/// memchr_iter's size_hint returns (0, Some(len)), so collect() starts at
/// capacity 0 and doubles ~20 times for 1M+ separators. Pre-allocating
/// with an estimated line length avoids all reallocations.
#[inline]
fn collect_positions_byte(data: &[u8], sep: u8) -> Vec<usize> {
    let estimated = data.len() / 40 + 64; // ~40 bytes per line, conservative
    let mut positions = Vec::with_capacity(estimated);
    for pos in memchr::memchr_iter(sep, data) {
        positions.push(pos);
    }
    positions
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

/// Split data into chunks at separator boundaries for parallel processing.
/// Returns chunk boundary positions (indices into data).
fn split_into_chunks(data: &[u8], sep: u8) -> Vec<usize> {
    let num_threads = rayon::current_num_threads().max(1);
    let chunk_target = data.len() / num_threads;
    let mut boundaries = vec![0usize];
    for i in 1..num_threads {
        let target = i * chunk_target;
        if target >= data.len() {
            break;
        }
        if let Some(p) = memchr::memchr(sep, &data[target..]) {
            let b = target + p + 1;
            if b > *boundaries.last().unwrap() && b <= data.len() {
                boundaries.push(b);
            }
        }
    }
    boundaries.push(data.len());
    boundaries
}

/// Parallel after-separator mode: find separator positions in parallel,
/// then stream IoSlice entries pointing directly into the source data.
/// Zero-copy: no output buffer allocation or memcpy. IoSlice entries
/// reference the original mmap/owned data, and write_vectored (or vmsplice
/// on Linux) transfers them directly to the pipe/file.
///
/// Uses streaming 1024-entry batches instead of pre-allocating all IoSlice
/// entries at once. For 100MB with 2.5M lines, this avoids a ~40MB allocation
/// (2.5M * 16 bytes) and its associated page fault overhead (~10ms for 10K pages).
fn tac_bytes_after_parallel(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let boundaries = split_into_chunks(data, sep);
    let n_chunks = boundaries.len() - 1;
    if n_chunks == 0 {
        return out.write_all(data);
    }

    // Parallel: find separator positions within each chunk.
    let chunk_positions: Vec<Vec<usize>> = (0..n_chunks)
        .into_par_iter()
        .map(|i| {
            let start = boundaries[i];
            let end = boundaries[i + 1];
            let chunk = &data[start..end];
            let estimated = chunk.len() / 40 + 64;
            let mut positions = Vec::with_capacity(estimated);
            for p in memchr::memchr_iter(sep, chunk) {
                positions.push(start + p);
            }
            positions
        })
        .collect();

    // Stream IoSlice entries in 1024-entry batches. Chunks are processed in
    // reverse order, records within each chunk in reverse. Each IoSlice
    // points into the original data buffer (zero-copy).
    const BATCH: usize = 1024;
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(BATCH);

    for i in (0..n_chunks).rev() {
        let chunk_start = boundaries[i];
        let chunk_end = boundaries[i + 1];
        let positions = &chunk_positions[i];

        let mut end = chunk_end;
        for &pos in positions.iter().rev() {
            let rec_start = pos + 1;
            if rec_start < end {
                slices.push(IoSlice::new(&data[rec_start..end]));
                if slices.len() >= BATCH {
                    write_all_vectored(out, &slices)?;
                    slices.clear();
                }
            }
            end = rec_start;
        }
        if end > chunk_start {
            slices.push(IoSlice::new(&data[chunk_start..end]));
            if slices.len() >= BATCH {
                write_all_vectored(out, &slices)?;
                slices.clear();
            }
        }
    }

    if !slices.is_empty() {
        write_all_vectored(out, &slices)?;
    }
    Ok(())
}

/// Parallel before-separator mode: same streaming zero-copy writev approach.
fn tac_bytes_before_parallel(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let boundaries = split_into_chunks(data, sep);
    let n_chunks = boundaries.len() - 1;
    if n_chunks == 0 {
        return out.write_all(data);
    }

    // Parallel: find separator positions within each chunk.
    let chunk_positions: Vec<Vec<usize>> = (0..n_chunks)
        .into_par_iter()
        .map(|i| {
            let start = boundaries[i];
            let end = boundaries[i + 1];
            let chunk = &data[start..end];
            let estimated = chunk.len() / 40 + 64;
            let mut positions = Vec::with_capacity(estimated);
            for p in memchr::memchr_iter(sep, chunk) {
                positions.push(start + p);
            }
            positions
        })
        .collect();

    // Stream IoSlice entries in 1024-entry batches (same as after mode).
    const BATCH: usize = 1024;
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(BATCH);

    for i in (0..n_chunks).rev() {
        let chunk_start = boundaries[i];
        let chunk_end = boundaries[i + 1];
        let positions = &chunk_positions[i];

        let mut end = chunk_end;
        for &pos in positions.iter().rev() {
            if pos < end {
                slices.push(IoSlice::new(&data[pos..end]));
                if slices.len() >= BATCH {
                    write_all_vectored(out, &slices)?;
                    slices.clear();
                }
            }
            end = pos;
        }
        if end > chunk_start {
            slices.push(IoSlice::new(&data[chunk_start..end]));
            if slices.len() >= BATCH {
                write_all_vectored(out, &slices)?;
                slices.clear();
            }
        }
    }

    if !slices.is_empty() {
        write_all_vectored(out, &slices)?;
    }
    Ok(())
}

/// After-separator mode: zero-copy writev from source data.
/// Records are output in reverse order using IoSlice entries pointing
/// directly into the input buffer. No output buffer allocation or copy needed.
/// For files <512KB, this avoids ~100-500KB of allocation + memcpy overhead,
/// trading it for batched writev syscalls (~3-13 calls for typical data).
fn tac_bytes_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let positions = collect_positions_byte(data, sep);

    if positions.is_empty() {
        return out.write_all(data);
    }

    const BATCH: usize = 1024;
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(BATCH);
    let mut end = data.len();

    for &pos in positions.iter().rev() {
        let rec_start = pos + 1;
        if rec_start < end {
            slices.push(IoSlice::new(&data[rec_start..end]));
            if slices.len() >= BATCH {
                write_all_vectored(out, &slices)?;
                slices.clear();
            }
        }
        end = rec_start;
    }

    // Remaining prefix before the first separator
    if end > 0 {
        slices.push(IoSlice::new(&data[..end]));
    }
    if !slices.is_empty() {
        write_all_vectored(out, &slices)?;
    }

    Ok(())
}

/// Before-separator mode: zero-copy writev from source data.
/// Same approach as after mode: IoSlice entries pointing into source data.
fn tac_bytes_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let positions = collect_positions_byte(data, sep);

    if positions.is_empty() {
        return out.write_all(data);
    }

    const BATCH: usize = 1024;
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(BATCH);
    let mut end = data.len();

    for &pos in positions.iter().rev() {
        if pos < end {
            slices.push(IoSlice::new(&data[pos..end]));
            if slices.len() >= BATCH {
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

    const BATCH: usize = 1024;
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(BATCH);
    let mut end = data.len();

    for &pos in positions.iter().rev() {
        let rec_start = pos + sep_len;
        if rec_start < end {
            slices.push(IoSlice::new(&data[rec_start..end]));
            if slices.len() >= BATCH {
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

    const BATCH: usize = 1024;
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(BATCH);
    let mut end = data.len();

    for &pos in positions.iter().rev() {
        if pos < end {
            slices.push(IoSlice::new(&data[pos..end]));
            if slices.len() >= BATCH {
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
