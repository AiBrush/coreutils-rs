use std::io::{self, IoSlice, Write};

/// Maximum number of iovecs per writev() call (Linux IOV_MAX is 1024).
const IOV_BATCH: usize = 1024;

/// Write all IoSlices to the writer, handling partial writes.
/// For large numbers of slices, batches into IOV_BATCH-sized groups.
fn write_all_slices(out: &mut impl Write, slices: &[IoSlice<'_>]) -> io::Result<()> {
    // Small number of slices: use simple write_all for each
    if slices.len() <= 4 {
        for s in slices {
            out.write_all(s)?;
        }
        return Ok(());
    }

    let mut offset = 0;
    while offset < slices.len() {
        let end = (offset + IOV_BATCH).min(slices.len());
        let n = out.write_vectored(&slices[offset..end])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "failed to write any data",
            ));
        }
        let mut remaining = n;
        while offset < end && remaining >= slices[offset].len() {
            remaining -= slices[offset].len();
            offset += 1;
        }
        if remaining > 0 && offset < end {
            out.write_all(&slices[offset][remaining..])?;
            offset += 1;
        }
    }
    Ok(())
}

/// Reverse records separated by a single byte.
/// Uses backward SIMD scan (memrchr_iter) to process records from back to front,
/// building batched IoSlice references for zero-copy writev output.
/// O(IOV_BATCH) memory — no positions Vec allocation needed at all.
/// For 100MB/2.5M lines: saves ~20MB positions Vec + ~40MB IoSlice Vec.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // For small/medium data (< 2MB), use simple backward scan with direct writes.
    // A single write_all is faster than writev with many small segments.
    if data.len() < 2 * 1024 * 1024 {
        return tac_bytes_small(data, separator, before, out);
    }

    let mut batch: Vec<IoSlice<'_>> = Vec::with_capacity(IOV_BATCH);

    if !before {
        // separator-after mode: records end with separator
        let mut iter = memchr::memrchr_iter(separator, data);

        let first_sep = match iter.next() {
            Some(pos) => pos,
            None => return out.write_all(data), // no separator found
        };

        // Trailing content without separator — output first
        if first_sep + 1 < data.len() {
            batch.push(IoSlice::new(&data[first_sep + 1..]));
        }

        let mut end = first_sep + 1; // exclusive end of current record

        // Process remaining separators from back to front
        for pos in iter {
            batch.push(IoSlice::new(&data[pos + 1..end]));
            end = pos + 1;
            if batch.len() == IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
            }
        }

        // First record (no separator before it)
        if end > 0 {
            batch.push(IoSlice::new(&data[0..end]));
        }
    } else {
        // separator-before mode: records start with separator
        let mut end = data.len();

        for pos in memchr::memrchr_iter(separator, data) {
            batch.push(IoSlice::new(&data[pos..end]));
            end = pos;
            if batch.len() == IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
            }
        }

        // Leading content before first separator
        if end > 0 {
            batch.push(IoSlice::new(&data[0..end]));
        }
    }

    if !batch.is_empty() {
        write_all_slices(out, &batch)?;
    }
    Ok(())
}

/// Small-file path: backward SIMD scan, copy records into contiguous buffer, single write.
/// Uses memrchr_iter for backward scanning (matches large-path approach).
/// Avoids positions Vec + IoSlice Vec allocations and writev syscalls.
/// Single write_all is faster than writev with many small segments for small data.
fn tac_bytes_small(
    data: &[u8],
    separator: u8,
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    // Use extend_from_slice instead of zero-init + index copy.
    // Avoids zero-initializing the entire buffer (saves memset overhead).
    let mut outbuf = Vec::with_capacity(data.len());

    if !before {
        // separator-after mode: records end with separator
        let mut iter = memchr::memrchr_iter(separator, data);

        let first_sep = match iter.next() {
            Some(pos) => pos,
            None => return out.write_all(data),
        };

        // Trailing content without separator — output first
        if first_sep + 1 < data.len() {
            outbuf.extend_from_slice(&data[first_sep + 1..]);
        }

        let mut end = first_sep + 1;

        for pos in iter {
            outbuf.extend_from_slice(&data[pos + 1..end]);
            end = pos + 1;
        }

        // First record
        if end > 0 {
            outbuf.extend_from_slice(&data[0..end]);
        }
    } else {
        // separator-before mode: records start with separator
        let mut end = data.len();

        for pos in memchr::memrchr_iter(separator, data) {
            outbuf.extend_from_slice(&data[pos..end]);
            end = pos;
        }

        // Leading content before first separator
        if end > 0 {
            outbuf.extend_from_slice(&data[0..end]);
        }
    }

    out.write_all(&outbuf)
}

/// Reverse records using a multi-byte string separator.
/// Uses SIMD-accelerated memmem for substring search.
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
    let estimated = (data.len() / separator.len().max(40)).max(64);
    let mut positions: Vec<usize> = Vec::with_capacity(estimated);
    for pos in memchr::memmem::find_iter(data, separator) {
        positions.push(pos);
    }

    if positions.is_empty() {
        out.write_all(data)?;
        return Ok(());
    }

    let sep_len = separator.len();

    // Small data: contiguous buffer + single write (avoids IoSlice/writev overhead)
    if data.len() < 2 * 1024 * 1024 {
        let mut outbuf = Vec::with_capacity(data.len());

        if !before {
            let last_end = positions.last().unwrap() + sep_len;

            if last_end < data.len() {
                outbuf.extend_from_slice(&data[last_end..]);
            }

            let mut i = positions.len();
            while i > 0 {
                i -= 1;
                let sep_start = positions[i];
                let rec_start = if i == 0 {
                    0
                } else {
                    positions[i - 1] + sep_len
                };
                outbuf.extend_from_slice(&data[rec_start..sep_start + sep_len]);
            }
        } else {
            let mut i = positions.len();
            while i > 0 {
                i -= 1;
                let start = positions[i];
                let end = if i + 1 < positions.len() {
                    positions[i + 1]
                } else {
                    data.len()
                };
                outbuf.extend_from_slice(&data[start..end]);
            }
            if positions[0] > 0 {
                outbuf.extend_from_slice(&data[..positions[0]]);
            }
        }
        return out.write_all(&outbuf);
    }

    // Large data: batched IoSlice/writev for zero-copy output
    let mut batch: Vec<IoSlice<'_>> = Vec::with_capacity(IOV_BATCH);

    if !before {
        let last_end = positions.last().unwrap() + sep_len;
        let has_trailing_sep = last_end == data.len();

        if !has_trailing_sep {
            batch.push(IoSlice::new(&data[last_end..]));
        }

        let mut i = positions.len();
        while i > 0 {
            i -= 1;
            let sep_start = positions[i];
            let rec_start = if i == 0 {
                0
            } else {
                positions[i - 1] + sep_len
            };
            batch.push(IoSlice::new(&data[rec_start..sep_start + sep_len]));
            if batch.len() == IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
            }
        }
    } else {
        let mut i = positions.len();
        while i > 0 {
            i -= 1;
            let start = positions[i];
            let end = if i + 1 < positions.len() {
                positions[i + 1]
            } else {
                data.len()
            };
            batch.push(IoSlice::new(&data[start..end]));
            if batch.len() == IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
            }
        }

        if positions[0] > 0 {
            batch.push(IoSlice::new(&data[..positions[0]]));
        }
    }

    if !batch.is_empty() {
        write_all_slices(out, &batch)?;
    }

    Ok(())
}

/// Find regex matches using backward scanning, matching GNU tac's re_search behavior.
/// GNU tac scans backward from the end, finding the rightmost starting position first.
/// This produces different matches than forward scanning for patterns like [0-9]+.
/// The matches are returned in left-to-right order.
fn find_regex_matches_backward(data: &[u8], re: &regex::bytes::Regex) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    let mut past_end = data.len();

    while past_end > 0 {
        let buf = &data[..past_end];
        let mut found = false;

        // Scan backward: try positions from past_end-1 down to 0
        // We need the LAST match starting position in buf, so we try from the end
        let mut pos = past_end;
        while pos > 0 {
            pos -= 1;
            if let Some(m) = re.find_at(buf, pos) {
                if m.start() == pos {
                    // Match starts at exactly this position — this is the rightmost match start
                    matches.push((m.start(), m.end()));
                    past_end = m.start();
                    found = true;
                    break;
                }
                // Match starts later than pos — skip to before that match
                // No point checking positions between pos and m.start() since
                // find_at already told us the leftmost match from pos starts at m.start()
                // But we need matches that START before m.start(), so continue decrementing
            }
            // If None, there's no match at pos or later, but there might be one earlier
            // (find_at only searches forward from pos)
        }

        if !found {
            break;
        }
    }

    matches.reverse(); // Convert from backward order to left-to-right order
    matches
}

/// Reverse records using a regex separator.
/// Uses regex::bytes for direct byte-level matching (no UTF-8 conversion needed).
/// NOTE: GNU tac uses POSIX Basic Regular Expressions (BRE), so we convert to ERE first.
/// Uses backward scanning to match GNU tac's re_search behavior.
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

    // Use backward scanning to match GNU tac's re_search behavior
    let matches = find_regex_matches_backward(data, &re);

    if matches.is_empty() {
        out.write_all(data)?;
        return Ok(());
    }

    // Small data: contiguous buffer + single write (avoids IoSlice/writev overhead)
    if data.len() < 2 * 1024 * 1024 {
        let mut outbuf = Vec::with_capacity(data.len());

        if !before {
            let last_end = matches.last().unwrap().1;

            if last_end < data.len() {
                outbuf.extend_from_slice(&data[last_end..]);
            }

            let mut i = matches.len();
            while i > 0 {
                i -= 1;
                let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
                outbuf.extend_from_slice(&data[rec_start..matches[i].1]);
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
                outbuf.extend_from_slice(&data[start..end]);
            }
            if matches[0].0 > 0 {
                outbuf.extend_from_slice(&data[..matches[0].0]);
            }
        }
        return out.write_all(&outbuf);
    }

    // Large data: batched IoSlice/writev for zero-copy output
    let mut batch: Vec<IoSlice<'_>> = Vec::with_capacity(IOV_BATCH);

    if !before {
        let last_end = matches.last().unwrap().1;
        let has_trailing_sep = last_end == data.len();

        if !has_trailing_sep {
            batch.push(IoSlice::new(&data[last_end..]));
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            let rec_end = matches[i].1;
            batch.push(IoSlice::new(&data[rec_start..rec_end]));
            if batch.len() == IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
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
            batch.push(IoSlice::new(&data[start..end]));
            if batch.len() == IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
            }
        }

        if matches[0].0 > 0 {
            batch.push(IoSlice::new(&data[..matches[0].0]));
        }
    }

    if !batch.is_empty() {
        write_all_slices(out, &batch)?;
    }

    Ok(())
}
