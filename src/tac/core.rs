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
/// Uses forward SIMD scan (memchr_iter) to collect separator positions,
/// then builds IoSlice references in reverse order for zero-copy writev output.
/// No output buffer allocation — references mmap'd data directly.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // Forward SIMD scan: collect all separator positions in one pass.
    let positions: Vec<usize> = memchr::memchr_iter(separator, data).collect();

    if positions.is_empty() {
        return out.write_all(data);
    }

    // Build IoSlice references in reverse order, write in batches.
    // Zero-copy: references mmap'd data directly, no output buffer needed.
    let mut batch: Vec<IoSlice<'_>> = Vec::with_capacity(IOV_BATCH.min(positions.len() + 2));

    if !before {
        // separator-after mode: records end with separator
        let last_sep = *positions.last().unwrap();

        // Trailing content without separator — output first
        if last_sep + 1 < data.len() {
            batch.push(IoSlice::new(&data[last_sep + 1..]));
        }

        // Records in reverse: each record is from (prev_sep+1) to (cur_sep+1)
        for i in (0..positions.len()).rev() {
            let start = if i == 0 { 0 } else { positions[i - 1] + 1 };
            let end = positions[i] + 1;
            batch.push(IoSlice::new(&data[start..end]));
            if batch.len() >= IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
            }
        }
    } else {
        // separator-before mode: records start with separator
        for i in (0..positions.len()).rev() {
            let start = positions[i];
            let end = if i + 1 < positions.len() {
                positions[i + 1]
            } else {
                data.len()
            };
            batch.push(IoSlice::new(&data[start..end]));
            if batch.len() >= IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
            }
        }

        // Leading content before first separator
        if positions[0] > 0 {
            batch.push(IoSlice::new(&data[..positions[0]]));
        }
    }

    if !batch.is_empty() {
        write_all_slices(out, &batch)?;
    }

    Ok(())
}

/// Reverse records using a multi-byte string separator.
/// Uses SIMD-accelerated memmem for substring search + IoSlice/writev output.
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

    // Build IoSlice references in reverse order, write in batches.
    let mut batch: Vec<IoSlice<'_>> = Vec::with_capacity(IOV_BATCH.min(positions.len() + 2));

    if !before {
        let last_end = positions.last().unwrap() + sep_len;
        if last_end < data.len() {
            batch.push(IoSlice::new(&data[last_end..]));
        }
        for i in (0..positions.len()).rev() {
            let rec_start = if i == 0 {
                0
            } else {
                positions[i - 1] + sep_len
            };
            batch.push(IoSlice::new(&data[rec_start..positions[i] + sep_len]));
            if batch.len() >= IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
            }
        }
    } else {
        for i in (0..positions.len()).rev() {
            let start = positions[i];
            let end = if i + 1 < positions.len() {
                positions[i + 1]
            } else {
                data.len()
            };
            batch.push(IoSlice::new(&data[start..end]));
            if batch.len() >= IOV_BATCH {
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

    // Build IoSlice references in reverse order, write in batches.
    let mut batch: Vec<IoSlice<'_>> = Vec::with_capacity(IOV_BATCH);

    if !before {
        let last_end = matches.last().unwrap().1;

        if last_end < data.len() {
            batch.push(IoSlice::new(&data[last_end..]));
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            batch.push(IoSlice::new(&data[rec_start..matches[i].1]));
            if batch.len() >= IOV_BATCH {
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
            if batch.len() >= IOV_BATCH {
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
