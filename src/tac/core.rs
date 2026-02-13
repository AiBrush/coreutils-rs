use std::io::{self, IoSlice, Write};

use rayon::prelude::*;

/// Max IoSlice entries per write_vectored batch.
/// Linux UIO_MAXIOV is 1024; we use that as our batch limit.
const MAX_IOV: usize = 1024;

/// Maximum data size for single-allocation reverse approach.
/// Below this limit, allocate one output buffer and do a single write_all
/// instead of building IoSlice vectors and doing multiple writev syscalls.
const SINGLE_ALLOC_LIMIT: usize = 256 * 1024 * 1024;

/// Minimum data size to trigger parallel record copying (1MB).
/// Below this, sequential copy is faster than rayon overhead.
const PARALLEL_TAC_THRESHOLD: usize = 1024 * 1024;

/// Threshold above which we split output into smaller write chunks.
/// For very large outputs (> 16MB), a single write_all can stall due to
/// pipe backpressure or kernel buffer limits. Chunking into 4MB writes
/// allows the kernel to process data incrementally.
const CHUNKED_WRITE_THRESHOLD: usize = 16 * 1024 * 1024;

/// Chunk size for chunked output writing.
const WRITE_CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Write buf to out, using chunked writes for large buffers to avoid
/// pipe backpressure stalls. After writing, hint the kernel to release
/// the buffer pages immediately (MADV_DONTNEED) since we're done with them.
#[inline]
fn write_maybe_chunked(out: &mut impl Write, buf: &[u8]) -> io::Result<()> {
    let result = if buf.len() > CHUNKED_WRITE_THRESHOLD {
        for chunk in buf.chunks(WRITE_CHUNK_SIZE) {
            out.write_all(chunk)?;
        }
        Ok(())
    } else {
        out.write_all(buf)
    };

    // Hint kernel to release these pages immediately — we're done with the buffer.
    // This reduces RSS for large files and avoids LRU pressure on the page cache.
    #[cfg(target_os = "linux")]
    if buf.len() >= 1024 * 1024 {
        unsafe {
            libc::madvise(
                buf.as_ptr() as *mut libc::c_void,
                buf.len(),
                libc::MADV_DONTNEED,
            );
        }
    }

    result
}

/// Copy records from data into buf at computed offsets.
/// Uses rayon parallel copy for large data with many records, sequential otherwise.
/// SAFETY: records must contain valid (start, len) pairs within data bounds.
/// Offsets must be a prefix sum of record lengths, fitting within buf.
#[inline]
fn copy_records_to_buf(data: &[u8], buf: &mut [u8], records: &[(usize, usize)], offsets: &[usize]) {
    if data.len() >= PARALLEL_TAC_THRESHOLD && records.len() >= 64 {
        let dst_base = buf.as_mut_ptr() as usize;
        let src_base = data.as_ptr() as usize;

        // SAFETY: Each record writes to a non-overlapping region of buf.
        // We pass base addresses as usize (which is Send+Sync) and
        // reconstruct pointers inside the closure.
        records.par_iter().zip(offsets.par_iter()).for_each(
            |(&(src_start, src_len), &dst_off)| unsafe {
                std::ptr::copy_nonoverlapping(
                    (src_base + src_start) as *const u8,
                    (dst_base + dst_off) as *mut u8,
                    src_len,
                );
            },
        );
    } else {
        let buf_ptr = buf.as_mut_ptr();
        let data_ptr = data.as_ptr();
        for (&(src_start, src_len), &dst_off) in records.iter().zip(offsets.iter()) {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data_ptr.add(src_start),
                    buf_ptr.add(dst_off),
                    src_len,
                );
            }
        }
    }
}

/// Flush a batch of IoSlice entries using write_vectored.
/// Falls back to individual write_all for each slice if write_vectored
/// doesn't write everything (handles partial writes).
#[inline]
fn flush_iov(out: &mut impl Write, slices: &[IoSlice]) -> io::Result<()> {
    if slices.is_empty() {
        return Ok(());
    }
    // Try write_vectored first for the whole batch
    let total: usize = slices.iter().map(|s| s.len()).sum();

    // Fast path: single writev call often writes everything
    let written = match out.write_vectored(slices) {
        Ok(n) if n >= total => return Ok(()),
        Ok(n) => n,
        Err(e) => return Err(e),
    };

    // Slow path: partial write — fall back to write_all per remaining slice
    let mut skip = written;
    for slice in slices {
        let slen = slice.len();
        if skip >= slen {
            skip -= slen;
            continue;
        }
        if skip > 0 {
            out.write_all(&slice[skip..])?;
            skip = 0;
        } else {
            out.write_all(slice)?;
        }
    }
    Ok(())
}

/// Reverse records separated by a single byte.
/// Uses backward SIMD scan (memrchr) — zero Vec allocation, single pass.
/// Output uses write_vectored (writev) for zero-copy from mmap'd data.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if !before {
        tac_bytes_backward_after(data, separator, out)
    } else {
        tac_bytes_backward_before(data, separator, out)
    }
}

/// After-separator mode: backward scan with memrchr.
/// Each record includes its trailing separator byte.
/// Uses IoSlice batching for zero-copy output directly from mmap'd data.
fn tac_bytes_backward_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    if data.len() <= SINGLE_ALLOC_LIMIT {
        return tac_bytes_backward_after_alloc(data, sep, out);
    }

    let mut iov: Vec<IoSlice> = Vec::with_capacity(MAX_IOV);

    let mut end = data.len();

    let Some(mut pos) = memchr::memrchr(sep, data) else {
        return out.write_all(data);
    };

    // Trailing content after last separator
    if pos + 1 < end {
        iov.push(IoSlice::new(&data[pos + 1..end]));
    }
    end = pos + 1;

    // Scan backward for remaining separators
    while pos > 0 {
        match memchr::memrchr(sep, &data[..pos]) {
            Some(prev) => {
                iov.push(IoSlice::new(&data[prev + 1..end]));
                if iov.len() >= MAX_IOV {
                    flush_iov(out, &iov)?;
                    iov.clear();
                }
                end = prev + 1;
                pos = prev;
            }
            None => break,
        }
    }

    // First record (from start of data)
    iov.push(IoSlice::new(&data[0..end]));
    flush_iov(out, &iov)?;

    Ok(())
}

/// Before-separator mode: backward scan with memrchr.
/// Each record starts with its separator byte.
/// Uses IoSlice batching for zero-copy output.
fn tac_bytes_backward_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    if data.len() <= SINGLE_ALLOC_LIMIT {
        return tac_bytes_backward_before_alloc(data, sep, out);
    }

    let mut iov: Vec<IoSlice> = Vec::with_capacity(MAX_IOV);

    let mut end = data.len();

    let Some(pos) = memchr::memrchr(sep, data) else {
        return out.write_all(data);
    };

    // Last record: from last separator to end
    iov.push(IoSlice::new(&data[pos..end]));
    end = pos;

    // Scan backward
    while end > 0 {
        match memchr::memrchr(sep, &data[..end]) {
            Some(prev) => {
                iov.push(IoSlice::new(&data[prev..end]));
                if iov.len() >= MAX_IOV {
                    flush_iov(out, &iov)?;
                    iov.clear();
                }
                end = prev;
            }
            None => break,
        }
    }

    // Leading content before first separator
    if end > 0 {
        iov.push(IoSlice::new(&data[0..end]));
    }

    flush_iov(out, &iov)?;
    Ok(())
}

/// Reverse records using a multi-byte string separator.
/// Uses backward SIMD-accelerated memmem (FinderRev) + IoSlice zero-copy output.
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

    // Single-alloc fast path for multi-byte separators
    if data.len() <= SINGLE_ALLOC_LIMIT {
        return if !before {
            tac_string_backward_after_alloc(data, separator, out)
        } else {
            tac_string_backward_before_alloc(data, separator, out)
        };
    }

    let sep_len = separator.len();
    let finder = memchr::memmem::FinderRev::new(separator);
    let mut iov: Vec<IoSlice> = Vec::with_capacity(MAX_IOV);

    if !before {
        let mut end = data.len();

        let Some(mut pos) = finder.rfind(data) else {
            return out.write_all(data);
        };

        // Trailing content after last separator
        if pos + sep_len < end {
            iov.push(IoSlice::new(&data[pos + sep_len..end]));
        }
        end = pos + sep_len;

        // Scan backward
        while pos > 0 {
            match finder.rfind(&data[..pos]) {
                Some(prev) => {
                    iov.push(IoSlice::new(&data[prev + sep_len..end]));
                    if iov.len() >= MAX_IOV {
                        flush_iov(out, &iov)?;
                        iov.clear();
                    }
                    end = prev + sep_len;
                    pos = prev;
                }
                None => break,
            }
        }

        // First record
        iov.push(IoSlice::new(&data[0..end]));
    } else {
        let mut end = data.len();

        let Some(pos) = finder.rfind(data) else {
            return out.write_all(data);
        };

        // Last record: from last separator to end
        iov.push(IoSlice::new(&data[pos..end]));
        end = pos;

        // Scan backward
        while end > 0 {
            match finder.rfind(&data[..end]) {
                Some(prev) => {
                    iov.push(IoSlice::new(&data[prev..end]));
                    if iov.len() >= MAX_IOV {
                        flush_iov(out, &iov)?;
                        iov.clear();
                    }
                    end = prev;
                }
                None => break,
            }
        }

        // Leading content before first separator
        if end > 0 {
            iov.push(IoSlice::new(&data[0..end]));
        }
    }

    flush_iov(out, &iov)?;
    Ok(())
}

// ============================================================================
// Single-allocation reverse functions (data <= SINGLE_ALLOC_LIMIT)
// ============================================================================

/// Single-allocation reverse for after-separator mode (single byte).
/// Two-phase approach: Phase 1 scans all separator positions (sequential, SIMD memrchr).
/// Phase 2 copies records into output buffer in parallel using rayon.
fn tac_bytes_backward_after_alloc(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let Some(first_sep) = memchr::memrchr(sep, data) else {
        return out.write_all(data);
    };

    // Phase 1: Find ALL separator positions using backward SIMD scan.
    // Collect into Vec in reverse order (last separator first).
    let mut sep_positions: Vec<usize> = Vec::new();
    let mut pos = first_sep;
    sep_positions.push(pos);
    while pos > 0 {
        match memchr::memrchr(sep, &data[..pos]) {
            Some(prev) => {
                sep_positions.push(prev);
                pos = prev;
            }
            None => break,
        }
    }
    // sep_positions is in reverse order: [last_sep, ..., first_sep]

    // Build record descriptors: (src_start, src_len) in output order (reversed).
    // Record layout (after-separator mode):
    //   - Trailing data after last separator (if any)
    //   - For each separator pair: data[prev+1..cur+1] (includes separator at cur)
    //   - First record: data[0..first_sep_in_data+1]
    let num_records = sep_positions.len() + 1 + if first_sep + 1 < data.len() { 1 } else { 0 };
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(num_records);

    // Trailing content after last separator
    if first_sep + 1 < data.len() {
        records.push((first_sep + 1, data.len() - (first_sep + 1)));
    }

    // Records between separators (sep_positions is already in reverse = output order)
    for i in 0..sep_positions.len() {
        let cur_sep = sep_positions[i];
        let prev_end = if i + 1 < sep_positions.len() {
            sep_positions[i + 1] + 1
        } else {
            0
        };
        let start = prev_end;
        let end = cur_sep + 1;
        if end > start {
            records.push((start, end - start));
        }
    }

    // Compute output offsets (prefix sum of record lengths)
    let total_len: usize = records.iter().map(|&(_, len)| len).sum();
    let mut offsets: Vec<usize> = Vec::with_capacity(records.len());
    let mut off = 0usize;
    for &(_, len) in &records {
        offsets.push(off);
        off += len;
    }

    // Allocate output buffer (uninit for speed -- every byte will be written)
    let mut buf: Vec<u8> = Vec::with_capacity(total_len);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(total_len);
    }

    // Phase 2: Copy records (parallel for large data, sequential for small).
    copy_records_to_buf(data, &mut buf, &records, &offsets);

    write_maybe_chunked(out, &buf[..total_len])
}

/// Single-allocation reverse for before-separator mode (single byte).
/// Two-phase: scan separators, then parallel copy records.
fn tac_bytes_backward_before_alloc(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let Some(last_sep) = memchr::memrchr(sep, data) else {
        return out.write_all(data);
    };

    // Phase 1: Find ALL separator positions using backward SIMD scan.
    let mut sep_positions: Vec<usize> = Vec::new();
    let mut pos = last_sep;
    sep_positions.push(pos);
    while pos > 0 {
        match memchr::memrchr(sep, &data[..pos]) {
            Some(prev) => {
                sep_positions.push(prev);
                pos = prev;
            }
            None => break,
        }
    }
    // sep_positions is in reverse order: [last_sep, ..., first_sep]

    // Build record descriptors: (src_start, src_len) in output order.
    // Before-separator mode: each record starts with its separator byte.
    // Output order: last record first.
    //   - Last record: data[last_sep..data.len()]
    //   - Between separators (in reverse): data[sep_i..sep_{i-1 in original order}]
    //   - Leading content before first separator (if any)
    let first_sep_in_data = *sep_positions.last().unwrap();
    let has_leading = first_sep_in_data > 0;
    let num_records = sep_positions.len() + if has_leading { 1 } else { 0 };
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(num_records);

    // sep_positions[0] = last_sep, sep_positions[last] = first_sep
    // Output: last_sep..end, then prev_sep..last_sep, etc.
    for i in 0..sep_positions.len() {
        let start = sep_positions[i];
        let end = if i == 0 {
            data.len()
        } else {
            sep_positions[i - 1]
        };
        if end > start {
            records.push((start, end - start));
        }
    }

    // Leading content before first separator
    if has_leading {
        records.push((0, first_sep_in_data));
    }

    // Compute output offsets (prefix sum)
    let total_len: usize = records.iter().map(|&(_, len)| len).sum();
    let mut offsets: Vec<usize> = Vec::with_capacity(records.len());
    let mut off = 0usize;
    for &(_, len) in &records {
        offsets.push(off);
        off += len;
    }

    // Allocate output buffer
    let mut buf: Vec<u8> = Vec::with_capacity(total_len);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(total_len);
    }

    // Phase 2: Copy records (parallel for large data, sequential for small).
    copy_records_to_buf(data, &mut buf, &records, &offsets);

    write_maybe_chunked(out, &buf[..total_len])
}

/// Single-allocation reverse for string separator, after mode.
/// Two-phase: scan separator positions, then parallel copy.
fn tac_string_backward_after_alloc(
    data: &[u8],
    separator: &[u8],
    out: &mut impl Write,
) -> io::Result<()> {
    let sep_len = separator.len();
    let finder = memchr::memmem::FinderRev::new(separator);

    let Some(first_sep) = finder.rfind(data) else {
        return out.write_all(data);
    };

    // Phase 1: Find all separator positions (reverse order)
    let mut sep_positions: Vec<usize> = Vec::new();
    let mut pos = first_sep;
    sep_positions.push(pos);
    while pos > 0 {
        match finder.rfind(&data[..pos]) {
            Some(prev) => {
                sep_positions.push(prev);
                pos = prev;
            }
            None => break,
        }
    }

    // Build record descriptors in output order (reversed)
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(sep_positions.len() + 2);

    // Trailing content after last separator
    if first_sep + sep_len < data.len() {
        records.push((first_sep + sep_len, data.len() - (first_sep + sep_len)));
    }

    // Records between separators
    for i in 0..sep_positions.len() {
        let cur_sep = sep_positions[i];
        let prev_end = if i + 1 < sep_positions.len() {
            sep_positions[i + 1] + sep_len
        } else {
            0
        };
        let end = cur_sep + sep_len;
        if end > prev_end {
            records.push((prev_end, end - prev_end));
        }
    }

    // Compute output offsets and total
    let total_len: usize = records.iter().map(|&(_, len)| len).sum();
    let mut offsets: Vec<usize> = Vec::with_capacity(records.len());
    let mut off = 0usize;
    for &(_, len) in &records {
        offsets.push(off);
        off += len;
    }

    let mut buf: Vec<u8> = Vec::with_capacity(total_len);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(total_len);
    }

    copy_records_to_buf(data, &mut buf, &records, &offsets);

    write_maybe_chunked(out, &buf[..total_len])
}

/// Single-allocation reverse for string separator, before mode.
/// Two-phase: scan separator positions, then parallel copy.
fn tac_string_backward_before_alloc(
    data: &[u8],
    separator: &[u8],
    out: &mut impl Write,
) -> io::Result<()> {
    let finder = memchr::memmem::FinderRev::new(separator);

    let Some(last_sep) = finder.rfind(data) else {
        return out.write_all(data);
    };

    // Phase 1: Find all separator positions (reverse order)
    let mut sep_positions: Vec<usize> = Vec::new();
    let mut pos = last_sep;
    sep_positions.push(pos);
    while pos > 0 {
        match finder.rfind(&data[..pos]) {
            Some(prev) => {
                sep_positions.push(prev);
                pos = prev;
            }
            None => break,
        }
    }

    // Build record descriptors in output order
    let first_sep_in_data = *sep_positions.last().unwrap();
    let has_leading = first_sep_in_data > 0;
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(sep_positions.len() + 1);

    for i in 0..sep_positions.len() {
        let start = sep_positions[i];
        let end = if i == 0 {
            data.len()
        } else {
            sep_positions[i - 1]
        };
        if end > start {
            records.push((start, end - start));
        }
    }

    if has_leading {
        records.push((0, first_sep_in_data));
    }

    // Compute output offsets and total
    let total_len: usize = records.iter().map(|&(_, len)| len).sum();
    let mut offsets: Vec<usize> = Vec::with_capacity(records.len());
    let mut off = 0usize;
    for &(_, len) in &records {
        offsets.push(off);
        off += len;
    }

    let mut buf: Vec<u8> = Vec::with_capacity(total_len);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(total_len);
    }

    copy_records_to_buf(data, &mut buf, &records, &offsets);

    write_maybe_chunked(out, &buf[..total_len])
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

    let mut iov: Vec<IoSlice> = Vec::with_capacity(matches.len().min(MAX_IOV) + 2);

    if !before {
        let last_end = matches.last().unwrap().1;

        if last_end < data.len() {
            iov.push(IoSlice::new(&data[last_end..]));
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            iov.push(IoSlice::new(&data[rec_start..matches[i].1]));
            if iov.len() >= MAX_IOV {
                flush_iov(out, &iov)?;
                iov.clear();
            }
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
            iov.push(IoSlice::new(&data[start..end]));
            if iov.len() >= MAX_IOV {
                flush_iov(out, &iov)?;
                iov.clear();
            }
        }

        if matches[0].0 > 0 {
            iov.push(IoSlice::new(&data[..matches[0].0]));
        }
    }

    flush_iov(out, &iov)?;
    Ok(())
}
