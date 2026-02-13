use std::io::{self, Write};

/// Output buffer: 16MB minimizes write() syscall count.
const OUT_BUF: usize = 16 * 1024 * 1024;

/// Copy a slice into the output buffer, flushing to the writer when full.
#[inline]
fn emit_slice(out: &mut impl Write, buf: &mut [u8], wp: &mut usize, s: &[u8]) -> io::Result<()> {
    let cap = buf.len();
    if *wp + s.len() <= cap {
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), buf.as_mut_ptr().add(*wp), s.len());
        }
        *wp += s.len();
        Ok(())
    } else if s.len() > cap {
        if *wp > 0 {
            out.write_all(&buf[..*wp])?;
            *wp = 0;
        }
        out.write_all(s)
    } else {
        out.write_all(&buf[..*wp])?;
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), buf.as_mut_ptr(), s.len());
        }
        *wp = s.len();
        Ok(())
    }
}

/// Reverse records separated by a single byte.
/// Uses backward SIMD scan (memrchr) — zero Vec allocation, single pass.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if !before {
        tac_bytes_backward_after(data, separator, out)
    } else {
        tac_bytes_backward_before(data, separator, out)
    }
}

/// After-separator mode: backward scan with memrchr.
/// Each record includes its trailing separator byte.
/// Scans from end to start, emitting records directly — no position Vec needed.
fn tac_bytes_backward_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let buf_size = data.len().min(OUT_BUF);
    let mut buf = vec![0u8; buf_size];
    let mut wp = 0;

    let mut end = data.len();

    let Some(mut pos) = memchr::memrchr(sep, data) else {
        return out.write_all(data);
    };

    // Trailing content after last separator
    if pos + 1 < end {
        emit_slice(out, &mut buf, &mut wp, &data[pos + 1..end])?;
    }
    end = pos + 1;

    // Scan backward for remaining separators
    while pos > 0 {
        match memchr::memrchr(sep, &data[..pos]) {
            Some(prev) => {
                emit_slice(out, &mut buf, &mut wp, &data[prev + 1..end])?;
                end = prev + 1;
                pos = prev;
            }
            None => break,
        }
    }

    // First record (from start of data)
    emit_slice(out, &mut buf, &mut wp, &data[0..end])?;

    if wp > 0 {
        out.write_all(&buf[..wp])?;
    }
    Ok(())
}

/// Before-separator mode: backward scan with memrchr.
/// Each record starts with its separator byte.
fn tac_bytes_backward_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let buf_size = data.len().min(OUT_BUF);
    let mut buf = vec![0u8; buf_size];
    let mut wp = 0;

    let mut end = data.len();

    let Some(pos) = memchr::memrchr(sep, data) else {
        return out.write_all(data);
    };

    // Last record: from last separator to end
    emit_slice(out, &mut buf, &mut wp, &data[pos..end])?;
    end = pos;

    // Scan backward
    while end > 0 {
        match memchr::memrchr(sep, &data[..end]) {
            Some(prev) => {
                emit_slice(out, &mut buf, &mut wp, &data[prev..end])?;
                end = prev;
            }
            None => break,
        }
    }

    // Leading content before first separator
    if end > 0 {
        emit_slice(out, &mut buf, &mut wp, &data[0..end])?;
    }

    if wp > 0 {
        out.write_all(&buf[..wp])?;
    }
    Ok(())
}

/// Reverse records using a multi-byte string separator.
/// Uses backward SIMD-accelerated memmem (FinderRev) + buffered output.
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
    let finder = memchr::memmem::FinderRev::new(separator);

    let buf_size = data.len().min(OUT_BUF);
    let mut buf = vec![0u8; buf_size];
    let mut wp = 0;

    if !before {
        let mut end = data.len();

        let Some(mut pos) = finder.rfind(data) else {
            return out.write_all(data);
        };

        // Trailing content after last separator
        if pos + sep_len < end {
            emit_slice(out, &mut buf, &mut wp, &data[pos + sep_len..end])?;
        }
        end = pos + sep_len;

        // Scan backward
        while pos > 0 {
            match finder.rfind(&data[..pos]) {
                Some(prev) => {
                    emit_slice(out, &mut buf, &mut wp, &data[prev + sep_len..end])?;
                    end = prev + sep_len;
                    pos = prev;
                }
                None => break,
            }
        }

        // First record
        emit_slice(out, &mut buf, &mut wp, &data[0..end])?;
    } else {
        let mut end = data.len();

        let Some(pos) = finder.rfind(data) else {
            return out.write_all(data);
        };

        // Last record: from last separator to end
        emit_slice(out, &mut buf, &mut wp, &data[pos..end])?;
        end = pos;

        // Scan backward
        while end > 0 {
            match finder.rfind(&data[..end]) {
                Some(prev) => {
                    emit_slice(out, &mut buf, &mut wp, &data[prev..end])?;
                    end = prev;
                }
                None => break,
            }
        }

        // Leading content before first separator
        if end > 0 {
            emit_slice(out, &mut buf, &mut wp, &data[0..end])?;
        }
    }

    if wp > 0 {
        out.write_all(&buf[..wp])?;
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

    let buf_size = data.len().min(OUT_BUF);
    let mut buf = vec![0u8; buf_size];
    let mut wp = 0;

    if !before {
        let last_end = matches.last().unwrap().1;

        if last_end < data.len() {
            emit_slice(out, &mut buf, &mut wp, &data[last_end..])?;
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            emit_slice(out, &mut buf, &mut wp, &data[rec_start..matches[i].1])?;
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
            emit_slice(out, &mut buf, &mut wp, &data[start..end])?;
        }

        if matches[0].0 > 0 {
            emit_slice(out, &mut buf, &mut wp, &data[..matches[0].0])?;
        }
    }

    if wp > 0 {
        out.write_all(&buf[..wp])?;
    }
    Ok(())
}
