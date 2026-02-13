use std::io::{self, IoSlice, Write};

/// Maximum number of IoSlice entries for write_vectored.
/// Linux UIO_MAXIOV is 1024, so we batch in groups of 1024.
const MAX_IOV: usize = 1024;

/// Threshold for using bulk buffer output instead of vectored I/O.
/// For large data, a single write_all of a pre-built buffer is faster
/// than many write_vectored syscalls.
const BULK_THRESHOLD: usize = 4 * 1024 * 1024;

/// Reverse records separated by a single byte.
/// Uses forward SIMD scan (memchr_iter) to collect separator positions,
/// then writes records in reverse order.
/// For large data (>4MB), pre-builds a single output buffer and writes at once.
/// For small data, uses vectored I/O (writev).
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // Forward SIMD scan: collect all separator positions in one pass.
    let positions: Vec<usize> = memchr::memchr_iter(separator, data).collect();

    if positions.is_empty() {
        return out.write_all(data);
    }

    if !before {
        // separator-after mode: records end with separator
        tac_bytes_after(data, &positions, out)
    } else {
        // separator-before mode: records start with separator
        tac_bytes_before(data, &positions, out)
    }
}

/// Separator-after mode.
/// Each record runs from (prev_sep+1) to (cur_sep+1), inclusive of separator.
/// For large data, pre-builds output buffer for single write_all.
/// For small data, uses vectored I/O (writev).
fn tac_bytes_after(data: &[u8], positions: &[usize], out: &mut impl Write) -> io::Result<()> {
    let last_sep = *positions.last().unwrap();

    // For large data, build a single output buffer and write at once
    if data.len() >= BULK_THRESHOLD {
        let mut output = Vec::with_capacity(data.len());

        // Trailing content without separator — output first
        if last_sep + 1 < data.len() {
            output.extend_from_slice(&data[last_sep + 1..]);
        }

        // Records in reverse order
        for i in (0..positions.len()).rev() {
            let start = if i == 0 { 0 } else { positions[i - 1] + 1 };
            let end = positions[i] + 1;
            output.extend_from_slice(&data[start..end]);
        }

        return out.write_all(&output);
    }

    // Small data: build list of record slices in reverse order
    let mut slices: Vec<&[u8]> = Vec::with_capacity(positions.len() + 1);

    // Trailing content without separator — output first
    if last_sep + 1 < data.len() {
        slices.push(&data[last_sep + 1..]);
    }

    // Records in reverse order
    for i in (0..positions.len()).rev() {
        let start = if i == 0 { 0 } else { positions[i - 1] + 1 };
        let end = positions[i] + 1;
        slices.push(&data[start..end]);
    }

    // Write all slices using vectored I/O in batches
    write_vectored_batched(out, &slices)
}

/// Separator-before mode.
/// For large data, pre-builds output buffer for single write_all.
/// For small data, uses vectored I/O (writev).
fn tac_bytes_before(data: &[u8], positions: &[usize], out: &mut impl Write) -> io::Result<()> {
    // For large data, build a single output buffer and write at once
    if data.len() >= BULK_THRESHOLD {
        let mut output = Vec::with_capacity(data.len());

        // Records in reverse order (each starts with separator)
        for i in (0..positions.len()).rev() {
            let start = positions[i];
            let end = if i + 1 < positions.len() {
                positions[i + 1]
            } else {
                data.len()
            };
            output.extend_from_slice(&data[start..end]);
        }

        // Leading content before first separator
        if positions[0] > 0 {
            output.extend_from_slice(&data[..positions[0]]);
        }

        return out.write_all(&output);
    }

    // Small data: vectored I/O
    let mut slices: Vec<&[u8]> = Vec::with_capacity(positions.len() + 1);

    // Records in reverse order (each starts with separator)
    for i in (0..positions.len()).rev() {
        let start = positions[i];
        let end = if i + 1 < positions.len() {
            positions[i + 1]
        } else {
            data.len()
        };
        slices.push(&data[start..end]);
    }

    // Leading content before first separator
    if positions[0] > 0 {
        slices.push(&data[..positions[0]]);
    }

    write_vectored_batched(out, &slices)
}

/// Write a list of slices using batched write_vectored (writev syscall).
/// Batches up to MAX_IOV slices per syscall to stay within OS limits.
fn write_vectored_batched(out: &mut impl Write, slices: &[&[u8]]) -> io::Result<()> {
    // For small number of slices, use direct write_vectored
    if slices.len() <= MAX_IOV {
        let io_slices: Vec<IoSlice<'_>> = slices.iter().map(|s| IoSlice::new(s)).collect();
        write_all_vectored(out, &io_slices)?;
        return Ok(());
    }

    // For large files with many records, batch in groups
    for batch in slices.chunks(MAX_IOV) {
        let io_slices: Vec<IoSlice<'_>> = batch.iter().map(|s| IoSlice::new(s)).collect();
        write_all_vectored(out, &io_slices)?;
    }
    Ok(())
}

/// Write all IoSlices, handling partial writes.
fn write_all_vectored(out: &mut impl Write, slices: &[IoSlice<'_>]) -> io::Result<()> {
    // Compute total bytes
    let total: usize = slices.iter().map(|s| s.len()).sum();
    if total == 0 {
        return Ok(());
    }

    // Try write_vectored first
    let mut written = 0;
    let mut remaining_slices = slices;

    while written < total {
        match out.write_vectored(remaining_slices) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write all data",
                ));
            }
            Ok(n) => {
                written += n;
                if written >= total {
                    break;
                }
                // Find where we left off — skip fully-written slices
                let mut skip_bytes = n;
                let mut skip_idx = 0;
                for (idx, slice) in remaining_slices.iter().enumerate() {
                    if skip_bytes >= slice.len() {
                        skip_bytes -= slice.len();
                        skip_idx = idx + 1;
                    } else {
                        break;
                    }
                }
                remaining_slices = &remaining_slices[skip_idx..];
                // If we're partway through a slice, fall back to write_all for remainder
                if skip_bytes > 0 && !remaining_slices.is_empty() {
                    out.write_all(&remaining_slices[0][skip_bytes..])?;
                    written += remaining_slices[0].len() - skip_bytes;
                    remaining_slices = &remaining_slices[1..];
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Reverse records using a multi-byte string separator.
/// Uses SIMD-accelerated memmem for substring search.
/// For large data, pre-builds output buffer for single write_all.
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

    // Find all occurrences of the separator using SIMD-accelerated memmem
    let positions: Vec<usize> = memchr::memmem::find_iter(data, separator).collect();

    if positions.is_empty() {
        return out.write_all(data);
    }

    let sep_len = separator.len();

    // For large data, build a single output buffer
    if data.len() >= BULK_THRESHOLD {
        let mut output = Vec::with_capacity(data.len());

        if !before {
            let last_end = positions.last().unwrap() + sep_len;
            if last_end < data.len() {
                output.extend_from_slice(&data[last_end..]);
            }
            for i in (0..positions.len()).rev() {
                let rec_start = if i == 0 {
                    0
                } else {
                    positions[i - 1] + sep_len
                };
                output.extend_from_slice(&data[rec_start..positions[i] + sep_len]);
            }
        } else {
            for i in (0..positions.len()).rev() {
                let start = positions[i];
                let end = if i + 1 < positions.len() {
                    positions[i + 1]
                } else {
                    data.len()
                };
                output.extend_from_slice(&data[start..end]);
            }
            if positions[0] > 0 {
                output.extend_from_slice(&data[..positions[0]]);
            }
        }

        return out.write_all(&output);
    }

    // Small data: build list of record slices in reverse order, then use vectored I/O
    let mut slices: Vec<&[u8]> = Vec::with_capacity(positions.len() + 1);

    if !before {
        let last_end = positions.last().unwrap() + sep_len;
        if last_end < data.len() {
            slices.push(&data[last_end..]);
        }
        for i in (0..positions.len()).rev() {
            let rec_start = if i == 0 {
                0
            } else {
                positions[i - 1] + sep_len
            };
            slices.push(&data[rec_start..positions[i] + sep_len]);
        }
    } else {
        for i in (0..positions.len()).rev() {
            let start = positions[i];
            let end = if i + 1 < positions.len() {
                positions[i + 1]
            } else {
                data.len()
            };
            slices.push(&data[start..end]);
        }
        if positions[0] > 0 {
            slices.push(&data[..positions[0]]);
        }
    }

    write_vectored_batched(out, &slices)
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

    // Build slices in reverse order
    let mut slices: Vec<&[u8]> = Vec::with_capacity(matches.len() + 1);

    if !before {
        let last_end = matches.last().unwrap().1;

        if last_end < data.len() {
            slices.push(&data[last_end..]);
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            slices.push(&data[rec_start..matches[i].1]);
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
            slices.push(&data[start..end]);
        }

        if matches[0].0 > 0 {
            slices.push(&data[..matches[0].0]);
        }
    }

    write_vectored_batched(out, &slices)
}
