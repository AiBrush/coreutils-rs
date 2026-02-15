use std::io::{self, IoSlice, Write};

/// Threshold for parallel processing (2MB).
/// With contiguous per-thread output buffers and std::thread::scope (no rayon overhead),
/// the parallel path amortizes thread creation (~100us) at 2MB+. Below 2MB, the
/// contiguous sequential buffer path handles it efficiently.
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

/// Parallel after-separator mode: each thread scans its chunk with memrchr_iter
/// (backward), copying records in reverse into a contiguous output buffer.
/// Then a single writev of N buffers (in reverse chunk order) outputs everything.
///
/// This eliminates:
/// - The Vec<u32> positions allocation (~10MB for 100MB)
/// - The sequential IoSlice-per-record loop (~2.5M iterations)
/// - ~2500 writev calls (reduced to 1)
///
/// Each thread's contiguous buffer uses MADV_HUGEPAGE to reduce page faults
/// from ~6400 (4KB pages for ~25MB) to ~12 (2MB pages).
fn tac_bytes_after_parallel(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let boundaries = split_into_chunks(data, sep);
    let n_chunks = boundaries.len() - 1;
    if n_chunks == 0 {
        return out.write_all(data);
    }

    // Each thread scans its chunk backward with memrchr_iter, copying records
    // in reverse into a contiguous output buffer. No positions Vec needed.
    let results: Vec<Vec<u8>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..n_chunks)
            .map(|i| {
                let start = boundaries[i];
                let end = boundaries[i + 1];
                s.spawn(move || {
                    let chunk = &data[start..end];
                    let chunk_len = chunk.len();

                    // Allocate contiguous output buffer with MADV_HUGEPAGE
                    let mut buf: Vec<u8> = Vec::with_capacity(chunk_len);
                    #[cfg(target_os = "linux")]
                    if chunk_len >= 2 * 1024 * 1024 {
                        unsafe {
                            libc::madvise(
                                buf.as_mut_ptr() as *mut libc::c_void,
                                chunk_len,
                                libc::MADV_HUGEPAGE,
                            );
                        }
                    }

                    // Scan backward with memrchr_iter, copy records in reverse
                    let mut write_pos = 0usize;
                    let mut rec_end = chunk_len;
                    for pos in memchr::memrchr_iter(sep, chunk) {
                        let rec_start = pos + 1;
                        let rec_len = rec_end - rec_start;
                        if rec_len > 0 {
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    chunk.as_ptr().add(rec_start),
                                    buf.as_mut_ptr().add(write_pos),
                                    rec_len,
                                );
                            }
                            write_pos += rec_len;
                        }
                        rec_end = rec_start;
                    }
                    // Content before first separator in this chunk
                    if rec_end > 0 {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                chunk.as_ptr(),
                                buf.as_mut_ptr().add(write_pos),
                                rec_end,
                            );
                        }
                        write_pos += rec_end;
                    }

                    unsafe { buf.set_len(write_pos) };
                    buf
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    // Single writev of N iovecs in reverse chunk order
    let slices: Vec<IoSlice<'_>> = results.iter().rev().map(|r| IoSlice::new(r)).collect();
    write_all_vectored(out, &slices)?;
    Ok(())
}

/// Parallel before-separator mode: contiguous per-thread output buffers.
fn tac_bytes_before_parallel(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let boundaries = split_into_chunks(data, sep);
    let n_chunks = boundaries.len() - 1;
    if n_chunks == 0 {
        return out.write_all(data);
    }

    // Each thread scans backward, copying records (separator-first) in reverse
    let results: Vec<Vec<u8>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..n_chunks)
            .map(|i| {
                let start = boundaries[i];
                let end = boundaries[i + 1];
                s.spawn(move || {
                    let chunk = &data[start..end];
                    let chunk_len = chunk.len();

                    let mut buf: Vec<u8> = Vec::with_capacity(chunk_len);
                    #[cfg(target_os = "linux")]
                    if chunk_len >= 2 * 1024 * 1024 {
                        unsafe {
                            libc::madvise(
                                buf.as_mut_ptr() as *mut libc::c_void,
                                chunk_len,
                                libc::MADV_HUGEPAGE,
                            );
                        }
                    }

                    let mut write_pos = 0usize;
                    let mut rec_end = chunk_len;
                    for pos in memchr::memrchr_iter(sep, chunk) {
                        let rec_len = rec_end - pos;
                        if rec_len > 0 {
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    chunk.as_ptr().add(pos),
                                    buf.as_mut_ptr().add(write_pos),
                                    rec_len,
                                );
                            }
                            write_pos += rec_len;
                        }
                        rec_end = pos;
                    }
                    if rec_end > 0 {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                chunk.as_ptr(),
                                buf.as_mut_ptr().add(write_pos),
                                rec_end,
                            );
                        }
                        write_pos += rec_end;
                    }

                    unsafe { buf.set_len(write_pos) };
                    buf
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let slices: Vec<IoSlice<'_>> = results.iter().rev().map(|r| IoSlice::new(r)).collect();
    write_all_vectored(out, &slices)?;
    Ok(())
}

/// After-separator mode: contiguous buffer backward scan.
///
/// Scans backward with memrchr_iter, copying records in reverse order into
/// a contiguous output buffer. Single write_all at the end — eliminates the
/// overhead of many writev calls with small IoSlice entries. For 1MB data
/// with 25K lines, this replaces ~25 writev calls (1024 entries each) with
/// a single write_all call plus fast memcpy.
fn tac_bytes_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let data_len = data.len();
    let mut buf: Vec<u8> = Vec::with_capacity(data_len);
    let src = data.as_ptr();
    let dst = buf.as_mut_ptr();
    let mut wp: usize = 0;
    let mut end = data_len;

    for pos in memchr::memrchr_iter(sep, data) {
        let rec_start = pos + 1;
        let rec_len = end - rec_start;
        if rec_len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(src.add(rec_start), dst.add(wp), rec_len);
            }
            wp += rec_len;
        }
        end = rec_start;
    }

    if end > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst.add(wp), end);
        }
        wp += end;
    }

    unsafe { buf.set_len(wp) };
    out.write_all(&buf)
}

/// Before-separator mode: contiguous buffer backward scan.
fn tac_bytes_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let data_len = data.len();
    let mut buf: Vec<u8> = Vec::with_capacity(data_len);
    let src = data.as_ptr();
    let dst = buf.as_mut_ptr();
    let mut wp: usize = 0;
    let mut end = data_len;

    for pos in memchr::memrchr_iter(sep, data) {
        let rec_len = end - pos;
        if rec_len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(src.add(pos), dst.add(wp), rec_len);
            }
            wp += rec_len;
        }
        end = pos;
    }

    if end > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst.add(wp), end);
        }
        wp += end;
    }

    unsafe { buf.set_len(wp) };
    out.write_all(&buf)
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
