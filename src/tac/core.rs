use std::io::{self, IoSlice, Write};

/// Threshold for parallel processing (2MB).
/// With contiguous per-thread output buffers and std::thread::scope (no rayon overhead),
/// the parallel path amortizes thread creation (~100us) at 2MB+. Below 2MB, the
/// streaming IoSlice path handles it efficiently.
const PARALLEL_THRESHOLD: usize = 2 * 1024 * 1024;

/// Reverse records separated by a single byte.
/// For large data (>= 2MB): parallel scan + copy into contiguous output buffer, single write.
/// For small data: streaming IoSlice batches with write_vectored.
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

/// Split data into chunks at separator boundaries for parallel processing.
/// Returns chunk boundary positions (indices into data).
fn split_into_chunks(data: &[u8], sep: u8) -> Vec<usize> {
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
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

/// Contiguous buffer parallel after-separator mode: each thread scans its chunk
/// backward with SIMD memrchr and copies reversed records into a contiguous output
/// buffer. Then a single write_all outputs the entire reversed data.
///
/// For 100MB: eliminates ~2500 writev/vmsplice syscalls → 1 write_all call.
/// Trades metadata allocation (~40MB Vec<(usize,usize)>) for a same-sized output
/// buffer but with much faster output (1 syscall vs ~2500).
fn tac_bytes_after_contiguous(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let boundaries = split_into_chunks(data, sep);
    let n_chunks = boundaries.len() - 1;
    if n_chunks == 0 {
        return out.write_all(data);
    }

    let mut buf = alloc_uninit_buf(data.len());

    // Split buf into per-chunk output slices, chunks in reverse order.
    // Each thread gets its own exclusive &mut [u8] slice — no data races.
    {
        let mut remaining = buf.as_mut_slice();
        let mut slices: Vec<(usize, &mut [u8])> = Vec::with_capacity(n_chunks);

        for i in (0..n_chunks).rev() {
            let chunk_size = boundaries[i + 1] - boundaries[i];
            let (slice, rest) = remaining.split_at_mut(chunk_size);
            slices.push((i, slice));
            remaining = rest;
        }

        std::thread::scope(|s| {
            for (chunk_idx, dst) in slices {
                let start = boundaries[chunk_idx];
                let end = boundaries[chunk_idx + 1];
                s.spawn(move || {
                    tac_copy_after(&data[start..end], sep, dst);
                });
            }
        });
    } // mutable borrows end here

    out.write_all(&buf)
}

/// Contiguous buffer parallel before-separator mode.
fn tac_bytes_before_contiguous(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let mut boundaries = split_into_chunks(data, sep);
    let n_chunks = boundaries.len() - 1;
    if n_chunks == 0 {
        return out.write_all(data);
    }

    // Adjust boundaries for before mode: boundary at sep, not sep+1
    let last = boundaries.len() - 1;
    for b in boundaries[1..last].iter_mut() {
        *b -= 1;
    }

    let mut buf = alloc_uninit_buf(data.len());

    {
        let mut remaining = buf.as_mut_slice();
        let mut slices: Vec<(usize, &mut [u8])> = Vec::with_capacity(n_chunks);

        for i in (0..n_chunks).rev() {
            let chunk_size = boundaries[i + 1] - boundaries[i];
            let (slice, rest) = remaining.split_at_mut(chunk_size);
            slices.push((i, slice));
            remaining = rest;
        }

        std::thread::scope(|s| {
            for (chunk_idx, dst) in slices {
                let start = boundaries[chunk_idx];
                let end = boundaries[chunk_idx + 1];
                s.spawn(move || {
                    tac_copy_before(&data[start..end], sep, dst);
                });
            }
        });
    }

    out.write_all(&buf)
}

/// Copy reversed records (after-separator mode) from src into dst.
/// Scans backward with SIMD memrchr, copies each record forward into dst.
#[inline]
fn tac_copy_after(src: &[u8], sep: u8, dst: &mut [u8]) {
    let mut end = src.len();
    let mut wp = 0;

    for pos in memchr::memrchr_iter(sep, src) {
        let rec_start = pos + 1;
        if rec_start < end {
            let len = end - rec_start;
            dst[wp..wp + len].copy_from_slice(&src[rec_start..end]);
            wp += len;
        }
        end = rec_start;
    }

    if end > 0 {
        dst[wp..wp + end].copy_from_slice(&src[..end]);
    }
}

/// Copy reversed records (before-separator mode) from src into dst.
#[inline]
fn tac_copy_before(src: &[u8], sep: u8, dst: &mut [u8]) {
    let mut end = src.len();
    let mut wp = 0;

    for pos in memchr::memrchr_iter(sep, src) {
        if pos < end {
            let len = end - pos;
            dst[wp..wp + len].copy_from_slice(&src[pos..end]);
            wp += len;
        }
        end = pos;
    }

    if end > 0 {
        dst[wp..wp + end].copy_from_slice(&src[..end]);
    }
}

/// Allocate a Vec<u8> of given length without zero-initialization.
/// Uses MADV_HUGEPAGE on Linux for buffers >= 2MB to reduce TLB misses.
#[inline]
#[allow(clippy::uninit_vec)]
fn alloc_uninit_buf(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    unsafe { v.set_len(len) };
    #[cfg(target_os = "linux")]
    if len >= 2 * 1024 * 1024 {
        unsafe {
            libc::madvise(
                v.as_mut_ptr() as *mut libc::c_void,
                len,
                libc::MADV_HUGEPAGE,
            );
        }
    }
    v
}

/// Zero-copy after-separator mode: streaming IoSlice directly from input data.
/// No buffer allocation — scans backward and emits IoSlice batches of 1024.
fn tac_bytes_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    const BATCH: usize = 1024;
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(BATCH);
    let mut end = data.len();

    for pos in memchr::memrchr_iter(sep, data) {
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

    const BATCH: usize = 1024;
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(BATCH);
    let mut end = data.len();

    for pos in memchr::memrchr_iter(sep, data) {
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
