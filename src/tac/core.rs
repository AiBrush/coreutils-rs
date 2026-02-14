use std::io::{self, Write};

/// Threshold above which we use parallel copy for building the output buffer.
/// Below this, the single-threaded memcpy is fast enough.
const PARALLEL_THRESHOLD: usize = 4 * 1024 * 1024;

/// Reverse records separated by a single byte.
/// Uses single forward SIMD pass to find separators, then builds a contiguous
/// output buffer with records in reverse order and writes via single write_all.
///
/// The contiguous-buffer approach is much faster than writev because:
/// - writev with 200K IoSlice entries requires ~200 syscalls (1024 per batch)
/// - Single write_all of contiguous buffer = 1-2 syscalls
/// - The memcpy cost to build the buffer is much less than the syscall overhead
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if !before {
        tac_bytes_contiguous_after(data, separator, out)
    } else {
        tac_bytes_contiguous_before(data, separator, out)
    }
}

/// Reverse records of an owned Vec in-place, then write.
/// Avoids allocating a second output buffer by using a two-pass approach:
/// 1. Reverse all bytes in the Vec
/// 2. Reverse each individual record (between separators)
/// This produces the same output as copying records in reverse order.
///
/// Only works for after-separator mode with single-byte separator.
pub fn tac_bytes_owned(
    data: &mut [u8],
    separator: u8,
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    // For before-separator mode, fall back to the copy approach
    if before {
        return tac_bytes(data, separator, before, out);
    }

    let len = data.len();

    // Step 1: Reverse the entire buffer
    data.reverse();

    // Step 2: Reverse each record within the reversed buffer.
    // After step 1, records are in the right order but each record's bytes are reversed.
    // Collect separator positions first (memchr SIMD pass), then reverse each segment.
    let positions: Vec<usize> = memchr::memchr_iter(separator, data).collect();
    let mut start = 0;
    for &pos in &positions {
        if pos > start {
            data[start..pos].reverse();
        }
        start = pos + 1;
    }
    // Reverse the last segment (no trailing separator)
    if start < len {
        data[start..len].reverse();
    }

    out.write_all(data)
}

/// After-separator mode: Build contiguous output buffer with records in reverse order.
/// Single forward SIMD memchr pass to find all separator positions, then copy
/// records into output buffer in reverse order. Single write_all at the end.
///
/// Directly iterates the positions array in reverse to copy records, avoiding
/// the intermediate records Vec allocation (saves ~3MB for 10MB files).
///
/// For large files (>4MB), uses rayon parallel copy to fill the output buffer.
fn tac_bytes_contiguous_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    // Collect separator positions with forward SIMD memchr (single fast pass).
    let positions: Vec<usize> = memchr::memchr_iter(sep, data).collect();

    if positions.is_empty() {
        // No separators found — output data as-is
        return out.write_all(data);
    }

    let total = data.len();

    // Allocate output buffer (no zero-init needed)
    let mut output: Vec<u8> = Vec::with_capacity(total);
    // SAFETY: every byte in [0..total) is written by the copy loop below.
    #[allow(clippy::uninit_vec)]
    unsafe {
        output.set_len(total);
    }

    if total >= PARALLEL_THRESHOLD && positions.len() > 100 {
        // Parallel copy: split positions array into chunks, each thread copies its records
        parallel_copy_after(data, &positions, &mut output);
    } else {
        // Sequential copy: iterate positions in reverse, copy records directly
        let src = data.as_ptr();
        let dst = output.as_mut_ptr();
        let mut wp = 0usize;
        let mut end = data.len();
        for &pos in positions.iter().rev() {
            let rec_start = pos + 1;
            if rec_start < end {
                let len = end - rec_start;
                unsafe {
                    std::ptr::copy_nonoverlapping(src.add(rec_start), dst.add(wp), len);
                }
                wp += len;
            }
            end = rec_start;
        }
        // First record (before the first separator)
        if end > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(src, dst.add(wp), end);
            }
        }
    }

    out.write_all(&output)
}

/// Before-separator mode: Build contiguous output buffer with records in reverse order.
/// Directly iterates positions to copy records, avoiding intermediate Vec.
fn tac_bytes_contiguous_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let positions: Vec<usize> = memchr::memchr_iter(sep, data).collect();

    if positions.is_empty() {
        return out.write_all(data);
    }

    let total = data.len();

    let mut output: Vec<u8> = Vec::with_capacity(total);
    #[allow(clippy::uninit_vec)]
    unsafe {
        output.set_len(total);
    }

    if total >= PARALLEL_THRESHOLD && positions.len() > 100 {
        parallel_copy_before(data, &positions, &mut output);
    } else {
        let src = data.as_ptr();
        let dst = output.as_mut_ptr();
        let mut wp = 0usize;
        let mut end = data.len();
        for &pos in positions.iter().rev() {
            if pos < end {
                let len = end - pos;
                unsafe {
                    std::ptr::copy_nonoverlapping(src.add(pos), dst.add(wp), len);
                }
                wp += len;
            }
            end = pos;
        }
        if end > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(src, dst.add(wp), end);
            }
        }
    }

    out.write_all(&output)
}

/// Parallel copy for after-separator mode.
/// Splits the reversed positions array into chunks, each thread copies its records.
fn parallel_copy_after(data: &[u8], positions: &[usize], output: &mut [u8]) {
    use rayon::prelude::*;

    // Build (start, len) records from positions in reverse order
    // We need this intermediate step for parallel chunking
    let num_records = positions.len() + 1;
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(num_records);
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        let rec_start = pos + 1;
        if rec_start < end {
            records.push((rec_start, end - rec_start));
        }
        end = rec_start;
    }
    if end > 0 {
        records.push((0, end));
    }

    let n = records.len();
    let num_threads = rayon::current_num_threads().max(1);
    let chunk_size = (n + num_threads - 1) / num_threads;

    let chunk_sizes: Vec<usize> = records
        .chunks(chunk_size)
        .map(|chunk| chunk.iter().map(|&(_, len)| len).sum())
        .collect();

    let mut chunk_offsets: Vec<usize> = Vec::with_capacity(chunk_sizes.len() + 1);
    chunk_offsets.push(0usize);
    let mut total = 0usize;
    for &sz in &chunk_sizes {
        total += sz;
        chunk_offsets.push(total);
    }

    let output_addr = output.as_mut_ptr() as usize;
    let data_addr = data.as_ptr() as usize;

    records
        .par_chunks(chunk_size)
        .enumerate()
        .for_each(|(ci, chunk)| {
            let op = output_addr as *mut u8;
            let dp = data_addr as *const u8;
            let mut wp = chunk_offsets[ci];
            for &(start, len) in chunk {
                unsafe {
                    std::ptr::copy_nonoverlapping(dp.add(start), op.add(wp), len);
                }
                wp += len;
            }
        });
}

/// Parallel copy for before-separator mode.
fn parallel_copy_before(data: &[u8], positions: &[usize], output: &mut [u8]) {
    use rayon::prelude::*;

    let mut records: Vec<(usize, usize)> = Vec::with_capacity(positions.len() + 1);
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        if pos < end {
            records.push((pos, end - pos));
        }
        end = pos;
    }
    if end > 0 {
        records.push((0, end));
    }

    let n = records.len();
    let num_threads = rayon::current_num_threads().max(1);
    let chunk_size = (n + num_threads - 1) / num_threads;

    let chunk_sizes: Vec<usize> = records
        .chunks(chunk_size)
        .map(|chunk| chunk.iter().map(|&(_, len)| len).sum())
        .collect();

    let mut chunk_offsets: Vec<usize> = Vec::with_capacity(chunk_sizes.len() + 1);
    chunk_offsets.push(0usize);
    let mut total = 0usize;
    for &sz in &chunk_sizes {
        total += sz;
        chunk_offsets.push(total);
    }

    let output_addr = output.as_mut_ptr() as usize;
    let data_addr = data.as_ptr() as usize;

    records
        .par_chunks(chunk_size)
        .enumerate()
        .for_each(|(ci, chunk)| {
            let op = output_addr as *mut u8;
            let dp = data_addr as *const u8;
            let mut wp = chunk_offsets[ci];
            for &(start, len) in chunk {
                unsafe {
                    std::ptr::copy_nonoverlapping(dp.add(start), op.add(wp), len);
                }
                wp += len;
            }
        });
}

/// Reverse records using a multi-byte string separator.
/// Uses chunk-based forward SIMD-accelerated memmem + contiguous buffer output.
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

/// Multi-byte string separator, after mode (separator at end of record).
/// Builds contiguous output buffer with direct copy from positions array.
fn tac_string_after(
    data: &[u8],
    separator: &[u8],
    sep_len: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let positions: Vec<usize> = memchr::memmem::find_iter(data, separator).collect();

    if positions.is_empty() {
        return out.write_all(data);
    }

    // Compute total output size
    let total = data.len();
    let mut output: Vec<u8> = Vec::with_capacity(total);
    #[allow(clippy::uninit_vec)]
    unsafe {
        output.set_len(total);
    }

    let src = data.as_ptr();
    let dst = output.as_mut_ptr();
    let mut wp = 0usize;
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        let rec_start = pos + sep_len;
        if rec_start < end {
            let len = end - rec_start;
            unsafe {
                std::ptr::copy_nonoverlapping(src.add(rec_start), dst.add(wp), len);
            }
            wp += len;
        }
        end = rec_start;
    }
    if end > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst.add(wp), end);
        }
    }

    out.write_all(&output)
}

/// Multi-byte string separator, before mode (separator at start of record).
/// Builds contiguous output buffer with direct copy from positions array.
fn tac_string_before(
    data: &[u8],
    separator: &[u8],
    _sep_len: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let positions: Vec<usize> = memchr::memmem::find_iter(data, separator).collect();

    if positions.is_empty() {
        return out.write_all(data);
    }

    let total = data.len();
    let mut output: Vec<u8> = Vec::with_capacity(total);
    #[allow(clippy::uninit_vec)]
    unsafe {
        output.set_len(total);
    }

    let src = data.as_ptr();
    let dst = output.as_mut_ptr();
    let mut wp = 0usize;
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        if pos < end {
            let len = end - pos;
            unsafe {
                std::ptr::copy_nonoverlapping(src.add(pos), dst.add(wp), len);
            }
            wp += len;
        }
        end = pos;
    }
    if end > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst.add(wp), end);
        }
    }

    out.write_all(&output)
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
/// Builds contiguous output buffer instead of using writev.
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

    // Build records in reverse order
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(matches.len() + 2);

    if !before {
        let last_end = matches.last().unwrap().1;

        if last_end < data.len() {
            records.push((last_end, data.len() - last_end));
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            records.push((rec_start, matches[i].1 - rec_start));
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
            records.push((start, end - start));
        }

        if matches[0].0 > 0 {
            records.push((0, matches[0].0));
        }
    }

    let total: usize = records.iter().map(|&(_, len)| len).sum();
    if total == 0 {
        return Ok(());
    }

    let mut output: Vec<u8> = Vec::with_capacity(total);
    #[allow(clippy::uninit_vec)]
    unsafe {
        output.set_len(total);
    }

    let src = data.as_ptr();
    let dst = output.as_mut_ptr();
    let mut wp = 0usize;
    for &(start, len) in &records {
        unsafe {
            std::ptr::copy_nonoverlapping(src.add(start), dst.add(wp), len);
        }
        wp += len;
    }

    out.write_all(&output)
}
