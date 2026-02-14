use std::io::{self, Write};

/// Reverse records separated by a single byte.
/// Reverse-copy approach: collects separator positions with SIMD memchr,
/// then copies records in reverse order into a contiguous output buffer
/// for a single write_all syscall.
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

/// Reverse records of an owned Vec. Delegates to tac_bytes which uses
/// reverse-copy into a contiguous buffer + single write_all.
pub fn tac_bytes_owned(
    data: &mut [u8],
    separator: u8,
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    tac_bytes(data, separator, before, out)
}

/// After-separator mode: reverse-copy records into a contiguous buffer, single write_all.
/// Uses memrchr_iter to scan backwards (SIMD-accelerated), finding separators from the end.
/// This avoids the Vec<usize> positions allocation entirely (~1MB for 130K lines)
/// and processes records in natural reverse order. Single pass: scan + copy interleaved.
fn tac_bytes_zerocopy_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    // Quick check: if no separators exist, output as-is without allocating output buffer.
    if memchr::memchr(sep, data).is_none() {
        return out.write_all(data);
    }

    // Allocate output buffer. For after-separator mode, output size <= input size.
    let mut outbuf: Vec<u8> = Vec::with_capacity(data.len());
    #[allow(clippy::uninit_vec)]
    unsafe {
        outbuf.set_len(data.len())
    };
    let dst = outbuf.as_mut_ptr();
    let src = data.as_ptr();
    let mut wp = 0;

    // Scan backwards with SIMD memrchr: yields separator positions from end to start.
    // For each separator found, copy the record between it and the previous end point.
    let mut end = data.len();

    for pos in memchr::memrchr_iter(sep, data) {
        let rec_start = pos + 1;
        let rec_len = end - rec_start;
        if rec_len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(src.add(rec_start), dst.add(wp), rec_len);
            }
            wp += rec_len;
        }
        end = rec_start;
    }

    // Remaining prefix before the first separator
    if end > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst.add(wp), end);
        }
        wp += end;
    }

    out.write_all(&outbuf[..wp])
}

/// Before-separator mode: reverse-copy records into a contiguous buffer, single write_all.
/// Uses memrchr_iter for zero-allocation reverse scanning.
fn tac_bytes_zerocopy_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    if memchr::memchr(sep, data).is_none() {
        return out.write_all(data);
    }

    let mut outbuf: Vec<u8> = Vec::with_capacity(data.len());
    #[allow(clippy::uninit_vec)]
    unsafe {
        outbuf.set_len(data.len())
    };
    let dst = outbuf.as_mut_ptr();
    let src = data.as_ptr();
    let mut wp = 0;

    let mut end = data.len();

    for pos in memchr::memrchr_iter(sep, data) {
        let rec_len = end - pos;
        if rec_len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(src.add(pos), dst.add(wp), rec_len);
            }
            wp += rec_len;
        }
        end = pos;
    }

    if end > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst.add(wp), end);
        }
        wp += end;
    }

    out.write_all(&outbuf[..wp])
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

/// Multi-byte string separator, after mode: reverse-copy records, single write_all.
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

    let mut outbuf: Vec<u8> = Vec::with_capacity(data.len());
    #[allow(clippy::uninit_vec)]
    unsafe {
        outbuf.set_len(data.len())
    };
    let dst = outbuf.as_mut_ptr();
    let src = data.as_ptr();
    let mut wp = 0;

    let mut end = data.len();
    for &pos in positions.iter().rev() {
        let rec_start = pos + sep_len;
        let rec_len = end - rec_start;
        if rec_len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(src.add(rec_start), dst.add(wp), rec_len);
            }
            wp += rec_len;
        }
        end = rec_start;
    }
    if end > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst.add(wp), end);
        }
        wp += end;
    }

    out.write_all(&outbuf[..wp])
}

/// Multi-byte string separator, before mode: reverse-copy records, single write_all.
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

    let mut outbuf: Vec<u8> = Vec::with_capacity(data.len());
    #[allow(clippy::uninit_vec)]
    unsafe {
        outbuf.set_len(data.len())
    };
    let dst = outbuf.as_mut_ptr();
    let src = data.as_ptr();
    let mut wp = 0;

    let mut end = data.len();
    for &pos in positions.iter().rev() {
        let rec_len = end - pos;
        if rec_len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(src.add(pos), dst.add(wp), rec_len);
            }
            wp += rec_len;
        }
        end = pos;
    }
    if end > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst.add(wp), end);
        }
        wp += end;
    }

    out.write_all(&outbuf[..wp])
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

    // Reverse-copy records into contiguous buffer for single write_all.
    let mut outbuf: Vec<u8> = Vec::with_capacity(data.len());
    #[allow(clippy::uninit_vec)]
    unsafe {
        outbuf.set_len(data.len())
    };
    let dst = outbuf.as_mut_ptr();
    let src_ptr = data.as_ptr();
    let mut wp = 0;

    for &(start, len) in &records {
        if len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(src_ptr.add(start), dst.add(wp), len);
            }
            wp += len;
        }
    }

    out.write_all(&outbuf[..wp])
}
