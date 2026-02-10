use memchr::memchr_iter;
use std::io::{self, BufRead, Write};

/// Configuration for cut operations.
pub struct CutConfig<'a> {
    pub mode: CutMode,
    pub ranges: &'a [Range],
    pub complement: bool,
    pub delim: u8,
    pub output_delim: &'a [u8],
    pub suppress_no_delim: bool,
    pub line_delim: u8,
}

/// A range specification like 1, 3-5, -3, 4-
#[derive(Debug, Clone)]
pub struct Range {
    pub start: usize, // 1-based, 0 means "from beginning"
    pub end: usize,   // 1-based, usize::MAX means "to end"
}

/// Parse a LIST specification like "1,3-5,7-" into ranges.
/// Each range is 1-based. Returns sorted, merged ranges.
pub fn parse_ranges(spec: &str) -> Result<Vec<Range>, String> {
    let mut ranges = Vec::new();

    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some(idx) = part.find('-') {
            let left = &part[..idx];
            let right = &part[idx + 1..];

            let start = if left.is_empty() {
                1
            } else {
                left.parse::<usize>()
                    .map_err(|_| format!("invalid range: '{}'", part))?
            };

            let end = if right.is_empty() {
                usize::MAX
            } else {
                right
                    .parse::<usize>()
                    .map_err(|_| format!("invalid range: '{}'", part))?
            };

            if start == 0 {
                return Err("fields and positions are numbered from 1".to_string());
            }
            if start > end {
                return Err(format!("invalid decreasing range: '{}'", part));
            }

            ranges.push(Range { start, end });
        } else {
            let n = part
                .parse::<usize>()
                .map_err(|_| format!("invalid field: '{}'", part))?;
            if n == 0 {
                return Err("fields and positions are numbered from 1".to_string());
            }
            ranges.push(Range { start: n, end: n });
        }
    }

    if ranges.is_empty() {
        return Err("you must specify a list of bytes, characters, or fields".to_string());
    }

    // Sort and merge overlapping ranges
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut merged = vec![ranges[0].clone()];
    for r in &ranges[1..] {
        let last = merged.last_mut().unwrap();
        if r.start <= last.end.saturating_add(1) {
            last.end = last.end.max(r.end);
        } else {
            merged.push(r.clone());
        }
    }

    Ok(merged)
}

/// Check if a 1-based position is in any range.
#[inline(always)]
fn in_ranges(ranges: &[Range], pos: usize) -> bool {
    for r in ranges {
        if pos < r.start {
            return false; // ranges are sorted, no point checking further
        }
        if pos <= r.end {
            return true;
        }
    }
    false
}

/// Cut fields from a line using a delimiter. Writes to `out`.
/// Uses memchr for SIMD-accelerated delimiter scanning.
#[inline]
pub fn cut_fields(
    line: &[u8],
    delim: u8,
    ranges: &[Range],
    complement: bool,
    output_delim: &[u8],
    suppress_no_delim: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    // Check if line contains delimiter at all
    if memchr::memchr(delim, line).is_none() {
        if !suppress_no_delim {
            out.write_all(line)?;
        }
        return Ok(());
    }

    // Walk through fields using memchr, output selected ones
    let mut field_num: usize = 1;
    let mut field_start: usize = 0;
    let mut first_output = true;

    for delim_pos in memchr_iter(delim, line) {
        let selected = in_ranges(ranges, field_num) != complement;
        if selected {
            if !first_output {
                out.write_all(output_delim)?;
            }
            out.write_all(&line[field_start..delim_pos])?;
            first_output = false;
        }
        field_start = delim_pos + 1;
        field_num += 1;
    }

    // Last field (after last delimiter)
    let selected = in_ranges(ranges, field_num) != complement;
    if selected {
        if !first_output {
            out.write_all(output_delim)?;
        }
        out.write_all(&line[field_start..])?;
    }

    Ok(())
}

/// Cut bytes/chars from a line. Writes selected bytes to `out`.
#[inline]
pub fn cut_bytes(
    line: &[u8],
    ranges: &[Range],
    complement: bool,
    output_delim: &[u8],
    out: &mut impl Write,
) -> io::Result<()> {
    let mut first_range = true;

    if complement {
        // For complement, output bytes NOT in any range
        let mut in_excluded = false;
        for (i, &b) in line.iter().enumerate() {
            let pos = i + 1;
            if in_ranges(ranges, pos) {
                if in_excluded {
                    first_range = false;
                }
                in_excluded = false;
            } else {
                if !in_excluded && !first_range && !output_delim.is_empty() {
                    out.write_all(output_delim)?;
                }
                out.write_all(&[b])?;
                in_excluded = true;
            }
        }
    } else {
        // Output bytes in ranges. Ranges are sorted and merged.
        for r in ranges {
            let start = r.start.saturating_sub(1); // convert to 0-based
            let end = r.end.min(line.len()); // clamp to line length
            if start >= line.len() {
                break;
            }
            if !first_range && !output_delim.is_empty() {
                out.write_all(output_delim)?;
            }
            out.write_all(&line[start..end])?;
            first_range = false;
        }
    }
    Ok(())
}

/// Process a full data buffer (from mmap or read) with cut operation.
/// Processes line-by-line by scanning for line_delim.
pub fn process_cut_data(data: &[u8], cfg: &CutConfig, out: &mut impl Write) -> io::Result<()> {
    let mut start = 0;

    for end_pos in memchr_iter(cfg.line_delim, data) {
        let line = &data[start..end_pos];
        process_one_line(line, cfg, out)?;
        out.write_all(&[cfg.line_delim])?;
        start = end_pos + 1;
    }

    // Handle last line without terminator
    if start < data.len() {
        let line = &data[start..];
        process_one_line(line, cfg, out)?;
        out.write_all(b"\n")?;
    }

    Ok(())
}

/// Process input from a reader (for stdin).
pub fn process_cut_reader<R: BufRead>(
    mut reader: R,
    cfg: &CutConfig,
    out: &mut impl Write,
) -> io::Result<()> {
    let mut buf = Vec::new();

    loop {
        buf.clear();
        let n = reader.read_until(cfg.line_delim, &mut buf)?;
        if n == 0 {
            break;
        }

        let has_delim = buf.last() == Some(&cfg.line_delim);
        let line = if has_delim {
            &buf[..buf.len() - 1]
        } else {
            &buf[..]
        };

        process_one_line(line, cfg, out)?;

        if has_delim {
            out.write_all(&[cfg.line_delim])?;
        } else if !line.is_empty() {
            out.write_all(b"\n")?;
        }
    }

    Ok(())
}

#[inline]
fn process_one_line(line: &[u8], cfg: &CutConfig, out: &mut impl Write) -> io::Result<()> {
    match cfg.mode {
        CutMode::Fields => cut_fields(
            line,
            cfg.delim,
            cfg.ranges,
            cfg.complement,
            cfg.output_delim,
            cfg.suppress_no_delim,
            out,
        ),
        CutMode::Bytes | CutMode::Characters => {
            cut_bytes(line, cfg.ranges, cfg.complement, cfg.output_delim, out)
        }
    }
}

/// Cut operation mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CutMode {
    Bytes,
    Characters,
    Fields,
}
