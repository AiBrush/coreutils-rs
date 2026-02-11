use std::io::{self, IoSlice, Write};

/// Maximum number of iovecs per writev() call (Linux IOV_MAX is 1024).
const IOV_BATCH: usize = 1024;

/// Maximum records for vectored I/O. Beyond this, use BufWriter to avoid
/// excessive memory for IoSlice entries (16 bytes each).
const VECTORED_MAX_RECORDS: usize = 256 * 1024;

/// Write all IoSlices to the writer, handling partial writes.
/// Batches into IOV_BATCH-sized groups for writev() efficiency.
fn write_all_slices(out: &mut impl Write, slices: &[IoSlice<'_>]) -> io::Result<()> {
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
        // Skip fully-written slices
        let mut remaining = n;
        while offset < end && remaining >= slices[offset].len() {
            remaining -= slices[offset].len();
            offset += 1;
        }
        // Handle partial write within a slice — use write_all for the remainder
        if remaining > 0 && offset < end {
            out.write_all(&slices[offset][remaining..])?;
            offset += 1;
        }
    }
    Ok(())
}

/// Reverse the records in `data` separated by a single byte `separator` and write to `out`.
/// If `before` is true, the separator is attached before the record instead of after.
/// Uses vectored I/O (writev) to write directly from mmap'd data — zero intermediate copies.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // Forward SIMD scan to collect all separator positions
    let positions: Vec<usize> = memchr::memchr_iter(separator, data).collect();

    if positions.is_empty() {
        out.write_all(data)?;
        return Ok(());
    }

    // For files with many records, use BufWriter to avoid excessive IoSlice memory.
    // For smaller record counts, use vectored I/O for zero-copy writes.
    if positions.len() > VECTORED_MAX_RECORDS {
        return tac_bytes_bufwriter(data, separator, before, &positions, out);
    }

    // Build IoSlice list pointing directly into mmap'd data — zero copies
    let sep_byte = [separator];

    if !before {
        let has_trailing_sep = *positions.last().unwrap() == data.len() - 1;
        let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(positions.len() + 4);

        // GNU tac appends separator to trailing content without one
        if !has_trailing_sep {
            let last_sep = *positions.last().unwrap();
            slices.push(IoSlice::new(&data[last_sep + 1..]));
            slices.push(IoSlice::new(&sep_byte));
        }

        let mut i = positions.len();
        while i > 0 {
            i -= 1;
            let end = positions[i] + 1;
            let start = if i == 0 { 0 } else { positions[i - 1] + 1 };
            slices.push(IoSlice::new(&data[start..end]));
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

/// BufWriter fallback for tac_bytes when there are too many records for vectored I/O.
fn tac_bytes_bufwriter(
    data: &[u8],
    separator: u8,
    before: bool,
    positions: &[usize],
    out: &mut impl Write,
) -> io::Result<()> {
    let mut buf = io::BufWriter::with_capacity(4 * 1024 * 1024, out);

    if !before {
        let has_trailing_sep = *positions.last().unwrap() == data.len() - 1;
        if !has_trailing_sep {
            let last_sep = *positions.last().unwrap();
            buf.write_all(&data[last_sep + 1..])?;
            buf.write_all(&[separator])?;
        }
        let mut i = positions.len();
        while i > 0 {
            i -= 1;
            let end = positions[i] + 1;
            let start = if i == 0 { 0 } else { positions[i - 1] + 1 };
            buf.write_all(&data[start..end])?;
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
            buf.write_all(&data[start..end])?;
        }
        if positions[0] > 0 {
            buf.write_all(&data[..positions[0]])?;
        }
    }
    buf.flush()?;
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

        // GNU tac appends separator to trailing content without one
        if !has_trailing_sep {
            slices.push(IoSlice::new(&data[last_end..]));
            slices.push(IoSlice::new(separator));
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

/// Reverse records using a regex separator.
/// Uses regex::bytes for direct byte-level matching (no UTF-8 conversion needed).
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

    // Collect all match positions (start, end) in forward order
    let matches: Vec<(usize, usize)> = re.find_iter(data).map(|m| (m.start(), m.end())).collect();

    if matches.is_empty() {
        out.write_all(data)?;
        return Ok(());
    }

    if !before {
        let last_end = matches.last().unwrap().1;
        let has_trailing_sep = last_end == data.len();
        let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(matches.len() + 4);

        // GNU tac appends the last separator match to close trailing content
        if !has_trailing_sep {
            slices.push(IoSlice::new(&data[last_end..]));
            let last_match = matches.last().unwrap();
            slices.push(IoSlice::new(&data[last_match.0..last_match.1]));
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
