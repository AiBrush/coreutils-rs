use std::io::{self, IoSlice, Write};

/// Reverse records separated by a single byte.
/// Zero-copy writev approach: scans for separators with SIMD memchr,
/// then outputs records in reverse order directly from input buffer
/// using batched write_vectored (writev) syscalls.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if !before {
        tac_bytes_writev_after(data, separator, out)
    } else {
        tac_bytes_writev_before(data, separator, out)
    }
}

/// Reverse records of an owned Vec. Delegates to tac_bytes.
pub fn tac_bytes_owned(
    data: &mut [u8],
    separator: u8,
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    tac_bytes(data, separator, before, out)
}

/// Flush a batch of record slices using write_vectored (writev syscall).
/// Falls back to individual write_all for partial writes.
#[inline]
fn flush_writev(data: &[u8], records: &[(usize, usize)], out: &mut impl Write) -> io::Result<()> {
    let slices: Vec<IoSlice<'_>> = records
        .iter()
        .map(|&(start, end)| IoSlice::new(&data[start..end]))
        .collect();

    let total: usize = records.iter().map(|&(s, e)| e - s).sum();
    if total == 0 {
        return Ok(());
    }

    let written = out.write_vectored(&slices)?;
    if written >= total {
        return Ok(());
    }

    // Partial write: find where we stopped and write_all the remainder
    let mut skip = written;
    for &(start, end) in records {
        let len = end - start;
        if skip >= len {
            skip -= len;
            continue;
        }
        out.write_all(&data[start + skip..end])?;
        skip = 0;
    }
    Ok(())
}

/// After-separator mode: zero-copy writev directly from input buffer.
/// Uses memrchr_iter to scan backwards (SIMD-accelerated), collecting record
/// boundaries in batches, then uses write_vectored (writev) to output records
/// without copying. Eliminates the full-size output buffer allocation entirely.
fn tac_bytes_writev_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    // Quick check: if no separators exist, output as-is.
    if memchr::memchr(sep, data).is_none() {
        return out.write_all(data);
    }

    // IOV_MAX is 1024 on Linux. Batch records to limit writev iovec count.
    const BATCH: usize = 1024;
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(BATCH);
    let mut end = data.len();

    for pos in memchr::memrchr_iter(sep, data) {
        let rec_start = pos + 1;
        if rec_start < end {
            records.push((rec_start, end));
            if records.len() >= BATCH {
                flush_writev(data, &records, out)?;
                records.clear();
            }
        }
        end = rec_start;
    }

    // Remaining prefix before the first separator
    if end > 0 {
        records.push((0, end));
    }

    if !records.is_empty() {
        flush_writev(data, &records, out)?;
    }
    Ok(())
}

/// Before-separator mode: zero-copy writev directly from input buffer.
fn tac_bytes_writev_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    if memchr::memchr(sep, data).is_none() {
        return out.write_all(data);
    }

    const BATCH: usize = 1024;
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(BATCH);
    let mut end = data.len();

    for pos in memchr::memrchr_iter(sep, data) {
        if pos < end {
            records.push((pos, end));
            if records.len() >= BATCH {
                flush_writev(data, &records, out)?;
                records.clear();
            }
        }
        end = pos;
    }

    if end > 0 {
        records.push((0, end));
    }

    if !records.is_empty() {
        flush_writev(data, &records, out)?;
    }
    Ok(())
}

/// Reverse records using a multi-byte string separator.
/// Uses SIMD-accelerated memmem + zero-copy writev output.
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

/// Multi-byte string separator, after mode: zero-copy writev output.
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

    const BATCH: usize = 1024;
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(BATCH);

    let mut end = data.len();
    for &pos in positions.iter().rev() {
        let rec_start = pos + sep_len;
        if rec_start < end {
            records.push((rec_start, end));
            if records.len() >= BATCH {
                flush_writev(data, &records, out)?;
                records.clear();
            }
        }
        end = rec_start;
    }
    if end > 0 {
        records.push((0, end));
    }
    if !records.is_empty() {
        flush_writev(data, &records, out)?;
    }
    Ok(())
}

/// Multi-byte string separator, before mode: zero-copy writev output.
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

    const BATCH: usize = 1024;
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(BATCH);

    let mut end = data.len();
    for &pos in positions.iter().rev() {
        if pos < end {
            records.push((pos, end));
            if records.len() >= BATCH {
                flush_writev(data, &records, out)?;
                records.clear();
            }
        }
        end = pos;
    }
    if end > 0 {
        records.push((0, end));
    }
    if !records.is_empty() {
        flush_writev(data, &records, out)?;
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

    // Build records in reverse order as (start, end) pairs, then flush via writev.
    let mut records: Vec<(usize, usize)> = Vec::with_capacity(matches.len() + 2);

    if !before {
        let last_end = matches.last().unwrap().1;

        if last_end < data.len() {
            records.push((last_end, data.len()));
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            records.push((rec_start, matches[i].1));
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
            records.push((start, end));
        }

        if matches[0].0 > 0 {
            records.push((0, matches[0].0));
        }
    }

    flush_writev(data, &records, out)
}
