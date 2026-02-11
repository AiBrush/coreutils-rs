use std::io::{self, Write};

/// Reverse the records in `data` separated by a single byte `separator` and write to `out`.
/// If `before` is true, the separator is attached before the record instead of after.
/// Uses memrchr for SIMD-accelerated reverse byte scanning with zero allocation.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let mut buf = io::BufWriter::with_capacity(256 * 1024, out);

    if !before {
        // Default mode: separator is AFTER the record (like newline at end of line)
        // Use memrchr to scan backwards — zero allocation, SIMD-accelerated
        let mut end = data.len();

        // Handle trailing content that has no separator
        // GNU tac appends the separator when input lacks a trailing one
        if data[end - 1] != separator {
            match memchr::memrchr(separator, &data[..end.saturating_sub(1)]) {
                Some(last_sep) => {
                    // Write trailing content + separator (GNU tac adds separator)
                    buf.write_all(&data[last_sep + 1..])?;
                    buf.write_all(&[separator])?;
                    end = last_sep + 1;
                }
                None => {
                    // No separator found at all — output data as-is
                    buf.write_all(data)?;
                    buf.flush()?;
                    return Ok(());
                }
            }
        }

        // Now data[end-1] is guaranteed to be a separator byte.
        // Scan backwards to emit each record (content + separator).
        while end > 0 {
            if end >= 2 {
                match memchr::memrchr(separator, &data[..end - 1]) {
                    Some(prev_sep) => {
                        buf.write_all(&data[prev_sep + 1..end])?;
                        end = prev_sep + 1;
                    }
                    None => {
                        // First record — from start of data
                        buf.write_all(&data[..end])?;
                        end = 0;
                    }
                }
            } else {
                // end == 1: data[0] is the separator (empty record)
                buf.write_all(&data[..1])?;
                end = 0;
            }
        }
    } else {
        // Before mode: separator is BEFORE the record
        // Scan backwards with memrchr — zero allocation
        let mut pos = data.len();

        while pos > 0 {
            match memchr::memrchr(separator, &data[..pos]) {
                Some(sep_pos) => {
                    // Write [separator .. pos) as one record
                    buf.write_all(&data[sep_pos..pos])?;
                    pos = sep_pos;
                }
                None => {
                    // No more separators — remaining content is leading data
                    break;
                }
            }
        }

        // Write any leading content (before the first separator)
        if pos > 0 {
            buf.write_all(&data[..pos])?;
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
    let mut buf = io::BufWriter::with_capacity(256 * 1024, out);

    if !before {
        // Default: separator after record
        let last_end = positions.last().unwrap() + sep_len;
        let has_trailing_sep = last_end == data.len();

        // Trailing chunk without separator
        if !has_trailing_sep {
            buf.write_all(&data[last_end..])?;
            buf.write_all(separator)?;
        }

        // Records in reverse
        let mut i = positions.len();
        while i > 0 {
            i -= 1;
            let sep_start = positions[i];
            let rec_start = if i == 0 { 0 } else { positions[i - 1] + sep_len };
            buf.write_all(&data[rec_start..sep_start + sep_len])?;
        }
    } else {
        // Before mode: separator before record
        let has_leading_sep = positions[0] == 0;

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

        if !has_leading_sep {
            buf.write_all(&data[..positions[0]])?;
        }
    }

    buf.flush()?;
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

    let mut buf = io::BufWriter::with_capacity(256 * 1024, out);

    if !before {
        let last_end = matches.last().unwrap().1;
        let has_trailing_sep = last_end == data.len();

        // Trailing content after last separator
        if !has_trailing_sep {
            buf.write_all(&data[last_end..])?;
            // Append the last separator match to close this record
            let last_match = matches.last().unwrap();
            buf.write_all(&data[last_match.0..last_match.1])?;
        }

        // Records in reverse
        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            let rec_end = matches[i].1;
            buf.write_all(&data[rec_start..rec_end])?;
        }
    } else {
        // Before mode
        let has_leading_sep = matches[0].0 == 0;

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let start = matches[i].0;
            let end = if i + 1 < matches.len() {
                matches[i + 1].0
            } else {
                data.len()
            };
            buf.write_all(&data[start..end])?;
        }

        if !has_leading_sep {
            buf.write_all(&data[..matches[0].0])?;
        }
    }

    buf.flush()?;
    Ok(())
}
