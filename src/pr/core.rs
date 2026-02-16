use std::io::{self, BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default page length in lines.
pub const DEFAULT_PAGE_LENGTH: usize = 66;
/// Default page width in columns.
pub const DEFAULT_PAGE_WIDTH: usize = 72;
/// Number of header lines (2 blank + 1 header + 2 blank).
pub const HEADER_LINES: usize = 5;
/// Number of footer lines (5 blank).
pub const FOOTER_LINES: usize = 5;

/// Configuration for the pr command.
#[derive(Clone)]
pub struct PrConfig {
    /// First page to print (1-indexed).
    pub first_page: usize,
    /// Last page to print (0 = no limit).
    pub last_page: usize,
    /// Number of columns.
    pub columns: usize,
    /// Print columns across rather than down.
    pub across: bool,
    /// Show control characters in hat notation (^X).
    pub show_control_chars: bool,
    /// Double-space output.
    pub double_space: bool,
    /// Date format string for header.
    pub date_format: String,
    /// Expand input tabs to spaces (char, width).
    pub expand_tabs: Option<(char, usize)>,
    /// Use form feeds instead of newlines for page breaks.
    pub form_feed: bool,
    /// Custom header string (replaces filename).
    pub header: Option<String>,
    /// Replace spaces with tabs in output (char, width).
    pub output_tabs: Option<(char, usize)>,
    /// Join lines (do not truncate lines when using columns).
    pub join_lines: bool,
    /// Page length in lines (including header/footer).
    pub page_length: usize,
    /// Merge multiple files side by side.
    pub merge: bool,
    /// Number lines: (separator_char, digits).
    pub number_lines: Option<(char, usize)>,
    /// First line number.
    pub first_line_number: usize,
    /// Indent (offset) each line by this many spaces.
    pub indent: usize,
    /// Suppress file-not-found warnings.
    pub no_file_warnings: bool,
    /// Column separator character.
    pub separator: Option<char>,
    /// Column separator string.
    pub sep_string: Option<String>,
    /// Omit header and trailer.
    pub omit_header: bool,
    /// Omit header, trailer, and form feeds.
    pub omit_pagination: bool,
    /// Show nonprinting characters.
    pub show_nonprinting: bool,
    /// Page width.
    pub page_width: usize,
    /// Truncate lines to page width (-W).
    pub truncate_lines: bool,
}

impl Default for PrConfig {
    fn default() -> Self {
        Self {
            first_page: 1,
            last_page: 0,
            columns: 1,
            across: false,
            show_control_chars: false,
            double_space: false,
            date_format: "%Y-%m-%d %H:%M".to_string(),
            expand_tabs: None,
            form_feed: false,
            header: None,
            output_tabs: None,
            join_lines: false,
            page_length: DEFAULT_PAGE_LENGTH,
            merge: false,
            number_lines: None,
            first_line_number: 1,
            indent: 0,
            no_file_warnings: false,
            separator: None,
            sep_string: None,
            omit_header: false,
            omit_pagination: false,
            show_nonprinting: false,
            page_width: DEFAULT_PAGE_WIDTH,
            truncate_lines: false,
        }
    }
}

/// Format a SystemTime as a date string using libc strftime.
fn format_header_date(time: &SystemTime, format: &str) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&secs, &mut tm);
    }

    // Use strftime via libc
    let c_format = std::ffi::CString::new(format).unwrap_or_default();
    let mut buf = vec![0u8; 256];
    let len = unsafe {
        libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            c_format.as_ptr(),
            &tm,
        )
    };
    if len == 0 {
        return String::new();
    }
    buf.truncate(len);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Expand tabs in a line to spaces.
fn expand_tabs_in_line(line: &str, tab_char: char, tab_width: usize) -> String {
    if tab_width == 0 {
        return line.replace(tab_char, "");
    }
    let mut result = String::with_capacity(line.len());
    let mut col = 0;
    for ch in line.chars() {
        if ch == tab_char {
            let spaces = tab_width - (col % tab_width);
            for _ in 0..spaces {
                result.push(' ');
            }
            col += spaces;
        } else {
            result.push(ch);
            col += 1;
        }
    }
    result
}

/// Convert a character to hat notation (^X) for control characters.
fn to_hat_notation(ch: char) -> String {
    let b = ch as u32;
    if b < 32 {
        format!("^{}", (b as u8 + b'@') as char)
    } else if b == 127 {
        "^?".to_string()
    } else {
        ch.to_string()
    }
}

/// Convert a character using -v notation (like cat -v).
fn to_nonprinting(ch: char) -> String {
    let b = ch as u32;
    if b < 32 && b != 9 && b != 10 {
        // Control chars except TAB and LF
        format!("^{}", (b as u8 + b'@') as char)
    } else if b == 127 {
        "^?".to_string()
    } else if b >= 128 && b < 160 {
        format!("M-^{}", (b as u8 - 128 + b'@') as char)
    } else if b >= 160 && b < 255 {
        format!("M-{}", (b as u8 - 128) as char)
    } else if b == 255 {
        "M-^?".to_string()
    } else {
        ch.to_string()
    }
}

/// Process a line for control char display.
fn process_control_chars(line: &str, show_control: bool, show_nonprinting: bool) -> String {
    if !show_control && !show_nonprinting {
        return line.to_string();
    }
    let mut result = String::with_capacity(line.len());
    for ch in line.chars() {
        if show_nonprinting {
            result.push_str(&to_nonprinting(ch));
        } else if show_control {
            result.push_str(&to_hat_notation(ch));
        } else {
            result.push(ch);
        }
    }
    result
}

/// Get the column separator to use.
fn get_column_separator(config: &PrConfig) -> String {
    if let Some(ref s) = config.sep_string {
        s.clone()
    } else if let Some(c) = config.separator {
        c.to_string()
    } else {
        " ".to_string()
    }
}

/// Paginate a single file and write output.
pub fn pr_file<R: BufRead, W: Write>(
    input: R,
    output: &mut W,
    config: &PrConfig,
    filename: &str,
    file_date: Option<SystemTime>,
) -> io::Result<()> {
    let date = file_date.unwrap_or_else(SystemTime::now);

    // Read all lines
    let mut all_lines: Vec<String> = Vec::new();
    for line_result in input.lines() {
        let line = line_result?;
        let mut line = line;

        // Expand tabs if requested
        if let Some((tab_char, tab_width)) = config.expand_tabs {
            line = expand_tabs_in_line(&line, tab_char, tab_width);
        }

        // Process control characters
        line = process_control_chars(&line, config.show_control_chars, config.show_nonprinting);

        all_lines.push(line);
    }

    let header_str = config.header.as_deref().unwrap_or(filename);
    let date_str = format_header_date(&date, &config.date_format);

    // Calculate body lines per page
    let body_lines_per_page = if config.omit_header || config.omit_pagination {
        if config.page_length > 0 {
            config.page_length
        } else {
            DEFAULT_PAGE_LENGTH
        }
    } else if config.page_length <= HEADER_LINES + FOOTER_LINES {
        // If page is too small for header+footer, just use 1 body line
        1
    } else {
        config.page_length - HEADER_LINES - FOOTER_LINES
    };

    // Account for double spacing: each input line takes 2 output lines
    let input_lines_per_page = if config.double_space {
        (body_lines_per_page + 1) / 2
    } else {
        body_lines_per_page
    };

    // Handle multi-column mode
    let columns = config.columns.max(1);
    let lines_per_column = if columns > 1 && !config.across {
        input_lines_per_page / columns
    } else {
        input_lines_per_page
    };

    let lines_consumed_per_page = if columns > 1 && !config.across {
        lines_per_column * columns
    } else {
        input_lines_per_page
    };

    // Split into pages
    let total_lines = all_lines.len();
    let mut line_number = config.first_line_number;
    let mut page_num = 1usize;
    let mut line_idx = 0;

    while line_idx < total_lines || (line_idx == 0 && total_lines == 0) {
        // For empty input, output one empty page (matching GNU behavior)
        if total_lines == 0 && line_idx == 0 {
            if page_num >= config.first_page
                && (config.last_page == 0 || page_num <= config.last_page)
            {
                if !config.omit_header && !config.omit_pagination {
                    write_header(output, &date_str, header_str, page_num, config)?;
                }
                if !config.omit_header && !config.omit_pagination {
                    write_footer(output, config)?;
                }
            }
            break;
        }

        let page_end = (line_idx + lines_consumed_per_page).min(total_lines);

        if page_num >= config.first_page && (config.last_page == 0 || page_num <= config.last_page)
        {
            // Write header
            if !config.omit_header && !config.omit_pagination {
                write_header(output, &date_str, header_str, page_num, config)?;
            }

            // Write body
            if columns > 1 {
                write_multicolumn_body(
                    output,
                    &all_lines[line_idx..page_end],
                    config,
                    columns,
                    lines_per_column,
                    &mut line_number,
                    body_lines_per_page,
                )?;
            } else {
                write_single_column_body(
                    output,
                    &all_lines[line_idx..page_end],
                    config,
                    &mut line_number,
                    body_lines_per_page,
                )?;
            }

            // Write footer
            if !config.omit_header && !config.omit_pagination {
                write_footer(output, config)?;
            }
        }

        line_idx = page_end;
        page_num += 1;

        // Break if we've consumed all lines
        if line_idx >= total_lines {
            break;
        }
    }

    Ok(())
}

/// Paginate multiple files merged side by side (-m mode).
pub fn pr_merge<W: Write>(
    inputs: &[Vec<String>],
    output: &mut W,
    config: &PrConfig,
    filenames: &[&str],
    file_dates: &[SystemTime],
) -> io::Result<()> {
    let date = file_dates.first().copied().unwrap_or_else(SystemTime::now);
    let date_str = format_header_date(&date, &config.date_format);
    let header_str = config
        .header
        .as_deref()
        .unwrap_or_else(|| filenames.first().copied().unwrap_or(""));

    let body_lines_per_page = if config.omit_header || config.omit_pagination {
        if config.page_length > 0 {
            config.page_length
        } else {
            DEFAULT_PAGE_LENGTH
        }
    } else if config.page_length <= HEADER_LINES + FOOTER_LINES {
        1
    } else {
        config.page_length - HEADER_LINES - FOOTER_LINES
    };

    let input_lines_per_page = if config.double_space {
        (body_lines_per_page + 1) / 2
    } else {
        body_lines_per_page
    };

    let num_files = inputs.len();
    let col_sep = get_column_separator(config);
    let col_width = if num_files > 1 {
        (config
            .page_width
            .saturating_sub(col_sep.len() * (num_files - 1)))
            / num_files
    } else {
        config.page_width
    };

    let max_lines = inputs.iter().map(|f| f.len()).max().unwrap_or(0);
    let mut page_num = 1usize;
    let mut line_idx = 0;
    let mut line_number = config.first_line_number;

    while line_idx < max_lines {
        let page_end = (line_idx + input_lines_per_page).min(max_lines);

        if page_num >= config.first_page && (config.last_page == 0 || page_num <= config.last_page)
        {
            if !config.omit_header && !config.omit_pagination {
                write_header(output, &date_str, header_str, page_num, config)?;
            }

            let mut body_lines_written = 0;
            for i in line_idx..page_end {
                if config.double_space && body_lines_written > 0 {
                    writeln!(output)?;
                    body_lines_written += 1;
                }

                let indent_str = " ".repeat(config.indent);
                write!(output, "{}", indent_str)?;

                if let Some((sep, digits)) = config.number_lines {
                    write!(output, "{:>width$}{}", line_number, sep, width = digits)?;
                    line_number += 1;
                }

                for (fi, file_lines) in inputs.iter().enumerate() {
                    let content = if i < file_lines.len() {
                        &file_lines[i]
                    } else {
                        ""
                    };
                    let truncated = if config.truncate_lines && content.len() > col_width {
                        &content[..col_width]
                    } else {
                        content
                    };
                    if fi > 0 {
                        write!(output, "{}", col_sep)?;
                    }
                    write!(output, "{:<width$}", truncated, width = col_width)?;
                }
                writeln!(output)?;
                body_lines_written += 1;
            }

            // Pad remaining body lines
            while body_lines_written < body_lines_per_page {
                writeln!(output)?;
                body_lines_written += 1;
            }

            if !config.omit_header && !config.omit_pagination {
                write_footer(output, config)?;
            }
        }

        line_idx = page_end;
        page_num += 1;
    }

    Ok(())
}

/// Write page header: 2 blank lines, date/header/page line, 2 blank lines.
fn write_header<W: Write>(
    output: &mut W,
    date_str: &str,
    header: &str,
    page_num: usize,
    config: &PrConfig,
) -> io::Result<()> {
    // 2 blank lines
    writeln!(output)?;
    writeln!(output)?;

    // Header line: date is left-aligned, header is centered, Page N is right-aligned.
    // Total width is page_width (default 72).
    let page_str = format!("Page {}", page_num);
    let line_width = config.page_width;

    let left = date_str;
    let right = &page_str;
    let center = header;

    // Available space for center text between left and right.
    let left_len = left.len();
    let right_len = right.len();
    let center_len = center.len();

    // GNU pr centers the header title within the line.
    // The layout is: LEFT + spaces + CENTER + spaces + RIGHT
    // where the total is exactly line_width characters.
    if left_len + center_len + right_len + 2 >= line_width {
        // Not enough space to center; just concatenate.
        writeln!(output, "{} {} {}", left, center, right)?;
    } else {
        let total_spaces = line_width - left_len - center_len - right_len;
        // Distribute spaces evenly around the center text.
        let left_spaces = total_spaces / 2;
        let right_spaces = total_spaces - left_spaces;
        writeln!(
            output,
            "{}{}{}{}{}",
            left,
            " ".repeat(left_spaces),
            center,
            " ".repeat(right_spaces),
            right
        )?;
    }

    // 2 blank lines
    writeln!(output)?;
    writeln!(output)?;

    Ok(())
}

/// Write page footer: 5 blank lines (or form feed).
fn write_footer<W: Write>(output: &mut W, config: &PrConfig) -> io::Result<()> {
    if config.form_feed {
        write!(output, "\x0c")?;
    } else {
        for _ in 0..FOOTER_LINES {
            writeln!(output)?;
        }
    }
    Ok(())
}

/// Write body for single column mode.
fn write_single_column_body<W: Write>(
    output: &mut W,
    lines: &[String],
    config: &PrConfig,
    line_number: &mut usize,
    body_lines_per_page: usize,
) -> io::Result<()> {
    let indent_str = " ".repeat(config.indent);
    let mut body_lines_written = 0;

    for (i, line) in lines.iter().enumerate() {
        if config.double_space && i > 0 {
            writeln!(output)?;
            body_lines_written += 1;
            if body_lines_written >= body_lines_per_page {
                break;
            }
        }

        write!(output, "{}", indent_str)?;

        if let Some((sep, digits)) = config.number_lines {
            write!(output, "{:>width$}{}", line_number, sep, width = digits)?;
            *line_number += 1;
        }

        let content = if config.truncate_lines {
            let max_w = compute_content_width(config);
            if line.len() > max_w {
                &line[..max_w]
            } else {
                line.as_str()
            }
        } else {
            line.as_str()
        };

        writeln!(output, "{}", content)?;
        body_lines_written += 1;
    }

    // Pad remaining body lines if not omitting headers
    if !config.omit_header && !config.omit_pagination {
        while body_lines_written < body_lines_per_page {
            writeln!(output)?;
            body_lines_written += 1;
        }
    }

    Ok(())
}

/// Compute available content width after accounting for numbering and indent.
fn compute_content_width(config: &PrConfig) -> usize {
    let mut w = config.page_width;
    w = w.saturating_sub(config.indent);
    if let Some((_, digits)) = config.number_lines {
        w = w.saturating_sub(digits + 1); // digits + separator
    }
    w
}

/// Write body for multi-column mode.
fn write_multicolumn_body<W: Write>(
    output: &mut W,
    lines: &[String],
    config: &PrConfig,
    columns: usize,
    lines_per_column: usize,
    line_number: &mut usize,
    body_lines_per_page: usize,
) -> io::Result<()> {
    let col_sep = get_column_separator(config);
    let col_width = if columns > 1 {
        (config
            .page_width
            .saturating_sub(col_sep.len() * (columns - 1)))
            / columns
    } else {
        config.page_width
    };

    let indent_str = " ".repeat(config.indent);
    let mut body_lines_written = 0;

    if config.across {
        // Print columns across: line 0 fills col0, line 1 fills col1, etc.
        let mut i = 0;
        while i < lines.len() {
            if config.double_space && body_lines_written > 0 {
                writeln!(output)?;
                body_lines_written += 1;
                if body_lines_written >= body_lines_per_page {
                    break;
                }
            }

            write!(output, "{}", indent_str)?;

            for col in 0..columns {
                let li = i + col;
                if col > 0 {
                    write!(output, "{}", col_sep)?;
                }
                if li < lines.len() {
                    if let Some((sep, digits)) = config.number_lines {
                        write!(output, "{:>width$}{}", line_number, sep, width = digits)?;
                        *line_number += 1;
                    }
                    let content = &lines[li];
                    let truncated = if config.truncate_lines && content.len() > col_width {
                        &content[..col_width]
                    } else {
                        content.as_str()
                    };
                    if col < columns - 1 {
                        write!(output, "{:<width$}", truncated, width = col_width)?;
                    } else {
                        write!(output, "{}", truncated)?;
                    }
                }
            }
            writeln!(output)?;
            body_lines_written += 1;
            i += columns;
        }
    } else {
        // Print columns down: first lines_per_column lines in col0, next in col1, etc.
        for row in 0..lines_per_column {
            if config.double_space && row > 0 {
                writeln!(output)?;
                body_lines_written += 1;
                if body_lines_written >= body_lines_per_page {
                    break;
                }
            }

            write!(output, "{}", indent_str)?;

            for col in 0..columns {
                let li = col * lines_per_column + row;
                if col > 0 {
                    write!(output, "{}", col_sep)?;
                }
                if li < lines.len() {
                    if let Some((sep, digits)) = config.number_lines {
                        let num = config.first_line_number + li;
                        write!(output, "{:>width$}{}", num, sep, width = digits)?;
                    }
                    let content = &lines[li];
                    let truncated = if config.truncate_lines && content.len() > col_width {
                        &content[..col_width]
                    } else {
                        content.as_str()
                    };
                    if col < columns - 1 {
                        write!(output, "{:<width$}", truncated, width = col_width)?;
                    } else {
                        write!(output, "{}", truncated)?;
                    }
                } else if col < columns - 1 {
                    write!(output, "{:<width$}", "", width = col_width)?;
                }
            }
            writeln!(output)?;
            body_lines_written += 1;
        }
        // Update line_number for the lines we processed
        if config.number_lines.is_some() {
            *line_number += lines.len();
        }
    }

    // Pad remaining body lines
    if !config.omit_header && !config.omit_pagination {
        while body_lines_written < body_lines_per_page {
            writeln!(output)?;
            body_lines_written += 1;
        }
    }

    Ok(())
}
