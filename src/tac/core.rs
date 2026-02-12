use std::io::{self, IoSlice, Write};

/// Maximum number of iovecs per writev() call (Linux IOV_MAX is 1024).
const IOV_BATCH: usize = 1024;

/// Write all IoSlices to the writer, handling partial writes.
fn write_all_slices(out: &mut impl Write, slices: &[IoSlice<'_>]) -> io::Result<()> {
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
/// Uses backward SIMD scan (memrchr_iter) to process records from back to front.
/// Large files use zero-copy writev; small files use contiguous buffer + single write.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // For small/medium data (< 16MB), use backward scan with buffer + single write.
    if data.len() < 16 * 1024 * 1024 {
        return tac_bytes_to_buf(data, separator, before, out);
    }

    // Large data: backward scan with batched writev for zero-copy output
    let mut batch: Vec<IoSlice<'_>> = Vec::with_capacity(IOV_BATCH);

    if !before {
        let mut iter = memchr::memrchr_iter(separator, data);

        let first_sep = match iter.next() {
            Some(pos) => pos,
            None => return out.write_all(data),
        };

        if first_sep + 1 < data.len() {
            batch.push(IoSlice::new(&data[first_sep + 1..]));
        }

        let mut end = first_sep + 1;

        for pos in iter {
            batch.push(IoSlice::new(&data[pos + 1..end]));
            end = pos + 1;
            if batch.len() == IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
            }
        }

        if end > 0 {
            batch.push(IoSlice::new(&data[0..end]));
        }
    } else {
        let mut end = data.len();

        for pos in memchr::memrchr_iter(separator, data) {
            batch.push(IoSlice::new(&data[pos..end]));
            end = pos;
            if batch.len() == IOV_BATCH {
                write_all_slices(out, &batch)?;
                batch.clear();
            }
        }

        if end > 0 {
            batch.push(IoSlice::new(&data[0..end]));
        }
    }

    if !batch.is_empty() {
        write_all_slices(out, &batch)?;
    }
    Ok(())
}

/// Sequential path: backward SIMD scan into contiguous buffer, single write.
fn tac_bytes_to_buf(
    data: &[u8],
    separator: u8,
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    let mut outbuf = Vec::with_capacity(data.len());

    if !before {
        let mut iter = memchr::memrchr_iter(separator, data);

        let first_sep = match iter.next() {
            Some(pos) => pos,
            None => return out.write_all(data),
        };

        if first_sep + 1 < data.len() {
            outbuf.extend_from_slice(&data[first_sep + 1..]);
        }

        let mut end = first_sep + 1;

        for pos in iter {
            outbuf.extend_from_slice(&data[pos + 1..end]);
            end = pos + 1;
        }

        if end > 0 {
            outbuf.extend_from_slice(&data[0..end]);
        }
    } else {
        let mut end = data.len();

        for pos in memchr::memrchr_iter(separator, data) {
            outbuf.extend_from_slice(&data[pos..end]);
            end = pos;
        }

        if end > 0 {
            outbuf.extend_from_slice(&data[0..end]);
        }
    }

    out.write_all(&outbuf)
}

/// Reverse records using a multi-byte string separator.
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

    // Use contiguous buffer + single write for all sizes
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

    out.write_all(&outbuf)
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

    out.write_all(&outbuf)
}
