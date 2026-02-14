use std::io::{self, IoSlice, Write};

/// Maximum number of IoSlice entries per write_vectored call.
/// Linux IOV_MAX is 1024, so we batch at this limit.
const IOV_BATCH: usize = 1024;

/// Reverse records separated by a single byte.
/// Zero-copy: uses write_vectored (writev) to output slices from the original
/// mmap data in reverse record order. No output buffer allocation, no memcpy.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if !before {
        tac_bytes_zerocopy_after(data, separator, out)
    } else {
        tac_bytes_zerocopy_before(data, separator, out)
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
    // For before-separator mode, fall back to the zero-copy approach
    if before {
        return tac_bytes(data, separator, before, out);
    }

    // In-place reversal only works correctly when data ends with separator.
    // When it doesn't, separators get misplaced (e.g., "A\nB" -> "B\nA" instead of "BA\n").
    // Fall back to the zero-copy approach for that case.
    let len = data.len();
    if data[len - 1] != separator {
        return tac_bytes(data, separator, false, out);
    }

    // Step 1: Reverse the entire buffer.
    // The trailing separator moves to position 0.
    data.reverse();

    // Step 2: Instead of rotate_left(1) (expensive full memmove), we write
    // data[1..] then data[0..1] separately. This avoids O(n) memmove.
    // "A\nB\n" -> reverse -> "\nB\nA" -> write [1..] then [0..1] -> "B\nA\n"
    let saved_byte = data[0];

    // Step 3: Reverse each record within the buffer (excluding the leading byte).
    // After step 1, records are in the right order but each record's bytes are reversed.
    let sub = &mut data[1..];
    let sub_len = sub.len();
    let est_capacity = (sub_len / 50).max(64);
    let mut positions: Vec<usize> = Vec::with_capacity(est_capacity);
    positions.extend(memchr::memchr_iter(separator, sub));
    let mut start = 0;
    for &pos in &positions {
        if pos > start {
            sub[start..pos].reverse();
        }
        start = pos + 1;
    }
    // Reverse the last segment (after the last separator, if any)
    if start < sub_len {
        sub[start..sub_len].reverse();
    }

    // Write data[1..] then the saved leading byte (the separator)
    out.write_all(&data[1..])?;
    out.write_all(&[saved_byte])
}

/// After-separator mode: zero-copy write from mmap in reverse record order.
/// For files with many short records, copies record data into a contiguous
/// buffer and writes with large write_all calls (cheaper than millions of
/// writev IoSlice entries). For files with few records, uses writev directly.
fn tac_bytes_zerocopy_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    // Single-pass collect with estimated capacity.
    // Estimate: assume ~50 bytes average per line. For 100MB, that's ~2M lines.
    // Even if the estimate is off, Vec::collect only reallocates log2(N) times,
    // and each reallocation is a memcpy (not a scan), which is cheaper than
    // a second full SIMD memchr scan of the entire file.
    let est_capacity = (data.len() / 50).max(64);
    let mut positions: Vec<usize> = Vec::with_capacity(est_capacity);
    positions.extend(memchr::memchr_iter(sep, data));

    if positions.is_empty() {
        // No separators found — output data as-is
        return out.write_all(data);
    }

    // Streaming writev: build and write IoSlice batches of IOV_BATCH at a time.
    // This avoids allocating the full Vec<IoSlice> for all records. For 200K records,
    // this saves 3.2MB (200K * 16 bytes/IoSlice) of allocation.
    // Each batch of IOV_BATCH entries is written with a single writev syscall.
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(IOV_BATCH);
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        let rec_start = pos + 1;
        if rec_start < end {
            slices.push(IoSlice::new(&data[rec_start..end]));
            if slices.len() >= IOV_BATCH {
                write_ioslices(out, &slices)?;
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
        write_ioslices(out, &slices)?;
    }
    Ok(())
}

/// Before-separator mode: zero-copy write from mmap in reverse record order.
fn tac_bytes_zerocopy_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    // Single-pass collect with estimated capacity.
    let est_capacity = (data.len() / 50).max(64);
    let mut positions: Vec<usize> = Vec::with_capacity(est_capacity);
    positions.extend(memchr::memchr_iter(sep, data));

    if positions.is_empty() {
        return out.write_all(data);
    }

    // Streaming writev: build and write batches of IOV_BATCH at a time.
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(IOV_BATCH);
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        if pos < end {
            slices.push(IoSlice::new(&data[pos..end]));
            if slices.len() >= IOV_BATCH {
                write_ioslices(out, &slices)?;
                slices.clear();
            }
        }
        end = pos;
    }
    if end > 0 {
        slices.push(IoSlice::new(&data[..end]));
    }
    if !slices.is_empty() {
        write_ioslices(out, &slices)?;
    }
    Ok(())
}

/// Write a slice array in batches of IOV_BATCH using write_vectored.
/// Handles partial writes correctly by advancing past consumed slices.
fn write_ioslices(out: &mut impl Write, slices: &[IoSlice<'_>]) -> io::Result<()> {
    let mut offset = 0;
    while offset < slices.len() {
        let end = (offset + IOV_BATCH).min(slices.len());
        let batch = &slices[offset..end];
        let expected: usize = batch.iter().map(|s| s.len()).sum();
        if expected == 0 {
            offset = end;
            continue;
        }

        let n = out.write_vectored(batch)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "write_vectored returned 0",
            ));
        }

        if n >= expected {
            // All slices in this batch were consumed
            offset = end;
        } else {
            // Partial write — advance past fully consumed slices,
            // then fall back to write_all for the remainder.
            let mut consumed = n;
            for (i, slice) in batch.iter().enumerate() {
                if consumed >= slice.len() {
                    consumed -= slice.len();
                } else {
                    // Partial slice — write remaining
                    out.write_all(&slice[consumed..])?;
                    // Write remaining full slices
                    for s in &batch[i + 1..] {
                        out.write_all(s)?;
                    }
                    break;
                }
            }
            offset = end;
        }
    }
    Ok(())
}

/// Reverse records using a multi-byte string separator.
/// Uses chunk-based forward SIMD-accelerated memmem + zero-copy writev output.
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
/// Zero-copy writev from input data in reverse record order.
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

    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(positions.len() + 1);
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        let rec_start = pos + sep_len;
        if rec_start < end {
            slices.push(IoSlice::new(&data[rec_start..end]));
        }
        end = rec_start;
    }
    if end > 0 {
        slices.push(IoSlice::new(&data[..end]));
    }

    write_ioslices(out, &slices)
}

/// Multi-byte string separator, before mode (separator at start of record).
/// Zero-copy writev from input data in reverse record order.
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

    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(positions.len() + 1);
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        if pos < end {
            slices.push(IoSlice::new(&data[pos..end]));
        }
        end = pos;
    }
    if end > 0 {
        slices.push(IoSlice::new(&data[..end]));
    }

    write_ioslices(out, &slices)
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
/// Zero-copy writev from input data.
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

    // Build records in reverse order as (start, len) pairs
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

    // Build IoSlice array and write with writev
    let slices: Vec<IoSlice<'_>> = records
        .iter()
        .filter(|&&(_, len)| len > 0)
        .map(|&(start, len)| IoSlice::new(&data[start..start + len]))
        .collect();

    write_ioslices(out, &slices)
}
