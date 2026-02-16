use std::cmp::Ordering;
use std::io::{self, Write};

/// How to handle sort-order checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderCheck {
    /// Default: check, warn once per file, continue, exit 1
    Default,
    /// --check-order: check, error, stop immediately
    Strict,
    /// --nocheck-order: no checking
    None,
}

/// Configuration for the comm command.
pub struct CommConfig {
    pub suppress_col1: bool,
    pub suppress_col2: bool,
    pub suppress_col3: bool,
    pub case_insensitive: bool,
    pub order_check: OrderCheck,
    pub output_delimiter: Option<Vec<u8>>,
    pub total: bool,
    pub zero_terminated: bool,
}

impl Default for CommConfig {
    fn default() -> Self {
        Self {
            suppress_col1: false,
            suppress_col2: false,
            suppress_col3: false,
            case_insensitive: false,
            order_check: OrderCheck::Default,
            output_delimiter: None,
            total: false,
            zero_terminated: false,
        }
    }
}

/// Result of the comm operation.
pub struct CommResult {
    pub count1: usize,
    pub count2: usize,
    pub count3: usize,
    pub had_order_error: bool,
}

/// Compare two byte slices, optionally case-insensitive (ASCII).
#[inline]
fn compare_lines(a: &[u8], b: &[u8], case_insensitive: bool) -> Ordering {
    if case_insensitive {
        for (&ca, &cb) in a.iter().zip(b.iter()) {
            match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                Ordering::Equal => continue,
                other => return other,
            }
        }
        a.len().cmp(&b.len())
    } else {
        a.cmp(b)
    }
}

/// Split data into lines by delimiter, using SIMD-accelerated scanning.
/// Does NOT include a trailing empty line if data ends with the delimiter.
fn split_lines<'a>(data: &'a [u8], delim: u8) -> Vec<&'a [u8]> {
    if data.is_empty() {
        return Vec::new();
    }
    let count = memchr::memchr_iter(delim, data).count();
    let has_trailing = data.last() == Some(&delim);
    let cap = if has_trailing { count } else { count + 1 };
    let mut lines = Vec::with_capacity(cap);
    let mut start = 0;
    for pos in memchr::memchr_iter(delim, data) {
        lines.push(&data[start..pos]);
        start = pos + 1;
    }
    if start < data.len() {
        lines.push(&data[start..]);
    }
    lines
}

/// Run the comm merge algorithm on two sorted inputs.
pub fn comm(
    data1: &[u8],
    data2: &[u8],
    config: &CommConfig,
    tool_name: &str,
    out: &mut impl Write,
) -> io::Result<CommResult> {
    let delim = if config.zero_terminated { b'\0' } else { b'\n' };
    let sep = config.output_delimiter.as_deref().unwrap_or(b"\t");

    // Build column prefixes. Each shown column before the current one
    // contributes one copy of the separator.
    // Column 1: always empty prefix.
    let prefix2: Vec<u8> = if !config.suppress_col1 {
        sep.to_vec()
    } else {
        Vec::new()
    };
    let mut prefix3: Vec<u8> = Vec::new();
    if !config.suppress_col1 {
        prefix3.extend_from_slice(sep);
    }
    if !config.suppress_col2 {
        prefix3.extend_from_slice(sep);
    }

    let lines1 = split_lines(data1, delim);
    let lines2 = split_lines(data2, delim);

    let mut i1 = 0usize;
    let mut i2 = 0usize;
    let mut count1 = 0usize;
    let mut count2 = 0usize;
    let mut count3 = 0usize;
    let mut had_order_error = false;
    let mut warned1 = false;
    let mut warned2 = false;
    let ci = config.case_insensitive;

    let mut buf = Vec::with_capacity(data1.len() + data2.len());

    while i1 < lines1.len() && i2 < lines2.len() {
        // Order checks
        if config.order_check != OrderCheck::None {
            if !warned1 && i1 > 0 && compare_lines(lines1[i1], lines1[i1 - 1], ci) == Ordering::Less
            {
                had_order_error = true;
                warned1 = true;
                eprintln!("{}: file 1 is not in sorted order", tool_name);
                if config.order_check == OrderCheck::Strict {
                    out.write_all(&buf)?;
                    return Ok(CommResult {
                        count1,
                        count2,
                        count3,
                        had_order_error,
                    });
                }
            }
            if !warned2 && i2 > 0 && compare_lines(lines2[i2], lines2[i2 - 1], ci) == Ordering::Less
            {
                had_order_error = true;
                warned2 = true;
                eprintln!("{}: file 2 is not in sorted order", tool_name);
                if config.order_check == OrderCheck::Strict {
                    out.write_all(&buf)?;
                    return Ok(CommResult {
                        count1,
                        count2,
                        count3,
                        had_order_error,
                    });
                }
            }
        }

        match compare_lines(lines1[i1], lines2[i2], ci) {
            Ordering::Less => {
                if !config.suppress_col1 {
                    buf.extend_from_slice(lines1[i1]);
                    buf.push(delim);
                }
                count1 += 1;
                i1 += 1;
            }
            Ordering::Greater => {
                if !config.suppress_col2 {
                    buf.extend_from_slice(&prefix2);
                    buf.extend_from_slice(lines2[i2]);
                    buf.push(delim);
                }
                count2 += 1;
                i2 += 1;
            }
            Ordering::Equal => {
                if !config.suppress_col3 {
                    buf.extend_from_slice(&prefix3);
                    buf.extend_from_slice(lines1[i1]);
                    buf.push(delim);
                }
                count3 += 1;
                i1 += 1;
                i2 += 1;
            }
        }
    }

    // Drain remaining from file 1
    while i1 < lines1.len() {
        if config.order_check != OrderCheck::None
            && !warned1
            && i1 > 0
            && compare_lines(lines1[i1], lines1[i1 - 1], ci) == Ordering::Less
        {
            had_order_error = true;
            warned1 = true;
            eprintln!("{}: file 1 is not in sorted order", tool_name);
            if config.order_check == OrderCheck::Strict {
                out.write_all(&buf)?;
                return Ok(CommResult {
                    count1,
                    count2,
                    count3,
                    had_order_error,
                });
            }
        }
        if !config.suppress_col1 {
            buf.extend_from_slice(lines1[i1]);
            buf.push(delim);
        }
        count1 += 1;
        i1 += 1;
    }

    // Drain remaining from file 2
    while i2 < lines2.len() {
        if config.order_check != OrderCheck::None
            && !warned2
            && i2 > 0
            && compare_lines(lines2[i2], lines2[i2 - 1], ci) == Ordering::Less
        {
            had_order_error = true;
            warned2 = true;
            eprintln!("{}: file 2 is not in sorted order", tool_name);
            if config.order_check == OrderCheck::Strict {
                out.write_all(&buf)?;
                return Ok(CommResult {
                    count1,
                    count2,
                    count3,
                    had_order_error,
                });
            }
        }
        if !config.suppress_col2 {
            buf.extend_from_slice(&prefix2);
            buf.extend_from_slice(lines2[i2]);
            buf.push(delim);
        }
        count2 += 1;
        i2 += 1;
    }

    // Total summary line
    if config.total {
        write!(&mut buf, "{}", count1)?;
        buf.extend_from_slice(sep);
        write!(&mut buf, "{}", count2)?;
        buf.extend_from_slice(sep);
        write!(&mut buf, "{}", count3)?;
        buf.extend_from_slice(sep);
        buf.extend_from_slice(b"total");
        buf.push(delim);
    }

    out.write_all(&buf)?;
    Ok(CommResult {
        count1,
        count2,
        count3,
        had_order_error,
    })
}
