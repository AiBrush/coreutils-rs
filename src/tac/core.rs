use std::io::{self, Write};

/// Reverse records separated by a single byte.
/// Uses forward SIMD scan (memchr_iter) to collect separator positions,
/// then writes records in reverse order directly from mmap'd data.
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
        let last_sep = *positions.last().unwrap();

        // Trailing content without separator — output first
        if last_sep + 1 < data.len() {
            out.write_all(&data[last_sep + 1..])?;
        }

        // Records in reverse: each record is from (prev_sep+1) to (cur_sep+1)
        // Write directly from mmap'd data — zero copy
        for i in (0..positions.len()).rev() {
            let start = if i == 0 { 0 } else { positions[i - 1] + 1 };
            let end = positions[i] + 1;
            out.write_all(&data[start..end])?;
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
            out.write_all(&data[start..end])?;
        }

        // Leading content before first separator
        if positions[0] > 0 {
            out.write_all(&data[..positions[0]])?;
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
        return out.write_all(data);
    }

    let sep_len = separator.len();

    if !before {
        let last_end = positions.last().unwrap() + sep_len;
        if last_end < data.len() {
            out.write_all(&data[last_end..])?;
        }
        for i in (0..positions.len()).rev() {
            let rec_start = if i == 0 {
                0
            } else {
                positions[i - 1] + sep_len
            };
            out.write_all(&data[rec_start..positions[i] + sep_len])?;
        }
    } else {
        for i in (0..positions.len()).rev() {
            let start = positions[i];
            let end = if i + 1 < positions.len() {
                positions[i + 1]
            } else {
                data.len()
            };
            out.write_all(&data[start..end])?;
        }
        if positions[0] > 0 {
            out.write_all(&data[..positions[0]])?;
        }
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

    if !before {
        let last_end = matches.last().unwrap().1;

        if last_end < data.len() {
            out.write_all(&data[last_end..])?;
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            out.write_all(&data[rec_start..matches[i].1])?;
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
            out.write_all(&data[start..end])?;
        }

        if matches[0].0 > 0 {
            out.write_all(&data[..matches[0].0])?;
        }
    }

    Ok(())
}
