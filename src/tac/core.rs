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
/// Uses a single forward SIMD scan to collect all separator positions,
/// then builds IoSlice references in reverse order for zero-copy output.
/// Eliminates per-record memrchr calls AND the copy to an output buffer.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // For small data, use simple backward scan with direct writes
    if data.len() < 64 * 1024 {
        return tac_bytes_small(data, separator, before, out);
    }

    // Single forward SIMD scan — O(n) with memchr auto-vectorization
    let positions: Vec<usize> = memchr::memchr_iter(separator, data).collect();

    if positions.is_empty() {
        return out.write_all(data);
    }

    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(positions.len() + 2);

    if !before {
        // separator-after mode: records end with separator
        let last_pos = *positions.last().unwrap();

        // Trailing content without separator — output first
        if last_pos < data.len() - 1 {
            slices.push(IoSlice::new(&data[last_pos + 1..]));
        }

        // Records in reverse order (each includes its trailing separator)
        for i in (0..positions.len()).rev() {
            let start = if i == 0 { 0 } else { positions[i - 1] + 1 };
            slices.push(IoSlice::new(&data[start..positions[i] + 1]));
        }
    } else {
        // separator-before mode: records start with separator
        for i in (0..positions.len()).rev() {
            let end = if i + 1 < positions.len() {
                positions[i + 1]
            } else {
                data.len()
            };
            slices.push(IoSlice::new(&data[positions[i]..end]));
        }

        // Leading content before first separator
        if positions[0] > 0 {
            slices.push(IoSlice::new(&data[..positions[0]]));
        }
    }

    write_all_slices(out, &slices)?;
    Ok(())
}

/// Small-file path: backward scan with direct write_all calls.
fn tac_bytes_small(
    data: &[u8],
    separator: u8,
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if !before {
        let mut end = data.len();

        // Check for trailing content after last separator
        if let Some(last_sep) = memchr::memrchr(separator, data) {
            if last_sep < data.len() - 1 {
                out.write_all(&data[last_sep + 1..])?;
                end = last_sep + 1;
            }
        } else {
            out.write_all(data)?;
            return Ok(());
        }

        let mut cursor = end;
        while cursor > 0 {
            let search_end = cursor - 1;
            let prev_sep = if search_end > 0 {
                memchr::memrchr(separator, &data[..search_end])
            } else {
                None
            };
            let start = match prev_sep {
                Some(pos) => pos + 1,
                None => 0,
            };
            out.write_all(&data[start..cursor])?;
            cursor = start;
        }
    } else {
        let mut cursor = data.len();
        while cursor > 0 {
            match memchr::memrchr(separator, &data[..cursor]) {
                Some(pos) => {
                    out.write_all(&data[pos..cursor])?;
                    cursor = pos;
                }
                None => {
                    out.write_all(&data[..cursor])?;
                    break;
                }
            }
        }
    }
    Ok(())
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
    let positions: Vec<usize> = memchr::memmem::find_iter(data, separator).collect();

    if positions.is_empty() {
        out.write_all(data)?;
        return Ok(());
    }

    let sep_len = separator.len();

    if !before {
        let last_end = positions.last().unwrap() + sep_len;
        let has_trailing_sep = last_end == data.len();
        let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(positions.len() + 4);

        // Trailing content without separator — output as-is
        if !has_trailing_sep {
            slices.push(IoSlice::new(&data[last_end..]));
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
            slices.push(IoSlice::new(&data[rec_start..sep_start + sep_len]));
        }

        write_all_slices(out, &slices)?;
    } else {
        let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(positions.len() + 2);

        let mut i = positions.len();
        while i > 0 {
            i -= 1;
            let start = positions[i];
            let end = if i + 1 < positions.len() {
                positions[i + 1]
            } else {
                data.len()
            };
            slices.push(IoSlice::new(&data[start..end]));
        }

        if positions[0] > 0 {
            slices.push(IoSlice::new(&data[..positions[0]]));
        }

        write_all_slices(out, &slices)?;
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

    if !before {
        let last_end = matches.last().unwrap().1;
        let has_trailing_sep = last_end == data.len();
        let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(matches.len() + 4);

        // Trailing content without separator — output as-is
        if !has_trailing_sep {
            slices.push(IoSlice::new(&data[last_end..]));
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            let rec_end = matches[i].1;
            slices.push(IoSlice::new(&data[rec_start..rec_end]));
        }

        write_all_slices(out, &slices)?;
    } else {
        let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(matches.len() + 2);

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

        write_all_slices(out, &slices)?;
    }

    Ok(())
}
