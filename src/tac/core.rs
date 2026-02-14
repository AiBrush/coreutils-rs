use std::io::{self, BufWriter, Write};

/// Buffer size for the BufWriter wrapping. 16MB keeps within L3 cache
/// while ensuring only a few write syscalls for even 100MB+ files.
const BUF_SIZE: usize = 16 * 1024 * 1024;

/// Reverse records separated by a single byte.
/// Zero-copy: writes directly from the input data in reverse record order
/// through a BufWriter, eliminating the need for a separate output buffer.
/// For 100MB input this avoids 100MB alloc + 100MB memcpy.
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
    let positions: Vec<usize> = memchr::memchr_iter(separator, sub).collect();
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
/// Uses a BufWriter to coalesce the small writes into large kernel writes.
/// No output buffer allocation, no memcpy — writes directly from input data.
fn tac_bytes_zerocopy_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    // Collect separator positions with forward SIMD memchr (single fast pass).
    let positions: Vec<usize> = memchr::memchr_iter(sep, data).collect();

    if positions.is_empty() {
        // No separators found — output data as-is
        return out.write_all(data);
    }

    // Wrap in BufWriter to coalesce writes. This avoids one-syscall-per-record.
    let mut bw = BufWriter::with_capacity(BUF_SIZE, out);

    // After mode: records split at separator positions. Iterate in reverse,
    // writing the data between each pair of separator+1 boundaries.
    // This is the same logic as the old contiguous buffer copy, but writes
    // directly to BufWriter instead of copy_nonoverlapping to an output Vec.
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        let rec_start = pos + 1;
        if rec_start < end {
            bw.write_all(&data[rec_start..end])?;
        }
        end = rec_start;
    }
    // Remaining prefix before the first separator
    if end > 0 {
        bw.write_all(&data[..end])?;
    }

    bw.flush()
}

/// Before-separator mode: zero-copy write from mmap in reverse record order.
/// Uses a BufWriter to coalesce the small writes into large kernel writes.
fn tac_bytes_zerocopy_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let positions: Vec<usize> = memchr::memchr_iter(sep, data).collect();

    if positions.is_empty() {
        return out.write_all(data);
    }

    let mut bw = BufWriter::with_capacity(BUF_SIZE, out);

    // Before mode: separator belongs to the following record.
    // Records are: [0..sep[0]), [sep[0]..sep[1]), ..., [sep[n-1]..data.len())
    // Output in reverse: last record first.
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        if pos < end {
            bw.write_all(&data[pos..end])?;
        }
        end = pos;
    }
    // Remaining prefix before first separator
    if end > 0 {
        bw.write_all(&data[..end])?;
    }

    bw.flush()
}

/// Reverse records using a multi-byte string separator.
/// Uses chunk-based forward SIMD-accelerated memmem + zero-copy output.
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
/// Zero-copy: writes directly from input data in reverse record order.
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

    let mut bw = BufWriter::with_capacity(BUF_SIZE, out);
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        let rec_start = pos + sep_len;
        if rec_start < end {
            bw.write_all(&data[rec_start..end])?;
        }
        end = rec_start;
    }
    if end > 0 {
        bw.write_all(&data[..end])?;
    }

    bw.flush()
}

/// Multi-byte string separator, before mode (separator at start of record).
/// Zero-copy: writes directly from input data in reverse record order.
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

    let mut bw = BufWriter::with_capacity(BUF_SIZE, out);
    let mut end = data.len();
    for &pos in positions.iter().rev() {
        if pos < end {
            bw.write_all(&data[pos..end])?;
        }
        end = pos;
    }
    if end > 0 {
        bw.write_all(&data[..end])?;
    }

    bw.flush()
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
/// Zero-copy: writes directly from input data via BufWriter.
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

    // Zero-copy output via BufWriter
    let mut bw = BufWriter::with_capacity(BUF_SIZE, out);
    for &(start, len) in &records {
        if len > 0 {
            bw.write_all(&data[start..start + len])?;
        }
    }

    bw.flush()
}
