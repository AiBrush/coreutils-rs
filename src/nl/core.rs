use std::io::Write;

/// Line numbering style.
#[derive(Clone)]
pub enum NumberingStyle {
    /// Number all lines.
    All,
    /// Number only non-empty lines (default for body).
    NonEmpty,
    /// Don't number lines.
    None,
    /// Number lines matching a basic regular expression.
    Regex(regex::Regex),
}

/// Number format for line numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumberFormat {
    /// Left-justified, no leading zeros.
    Ln,
    /// Right-justified, no leading zeros (default).
    Rn,
    /// Right-justified, leading zeros.
    Rz,
}

/// Configuration for the nl command.
pub struct NlConfig {
    pub body_style: NumberingStyle,
    pub header_style: NumberingStyle,
    pub footer_style: NumberingStyle,
    pub section_delimiter: Vec<u8>,
    pub line_increment: i64,
    pub join_blank_lines: usize,
    pub number_format: NumberFormat,
    pub no_renumber: bool,
    pub number_separator: Vec<u8>,
    pub starting_line_number: i64,
    pub number_width: usize,
}

impl Default for NlConfig {
    fn default() -> Self {
        Self {
            body_style: NumberingStyle::NonEmpty,
            header_style: NumberingStyle::None,
            footer_style: NumberingStyle::None,
            section_delimiter: vec![b'\\', b':'],
            line_increment: 1,
            join_blank_lines: 1,
            number_format: NumberFormat::Rn,
            no_renumber: false,
            number_separator: vec![b'\t'],
            starting_line_number: 1,
            number_width: 6,
        }
    }
}

/// Parse a numbering style string.
pub fn parse_numbering_style(s: &str) -> Result<NumberingStyle, String> {
    match s {
        "a" => Ok(NumberingStyle::All),
        "t" => Ok(NumberingStyle::NonEmpty),
        "n" => Ok(NumberingStyle::None),
        _ if s.starts_with('p') => {
            let pattern = &s[1..];
            match regex::Regex::new(pattern) {
                Ok(re) => Ok(NumberingStyle::Regex(re)),
                Err(e) => Err(format!("invalid regular expression: {}", e)),
            }
        }
        _ => Err(format!("invalid numbering style: '{}'", s)),
    }
}

/// Parse a number format string.
pub fn parse_number_format(s: &str) -> Result<NumberFormat, String> {
    match s {
        "ln" => Ok(NumberFormat::Ln),
        "rn" => Ok(NumberFormat::Rn),
        "rz" => Ok(NumberFormat::Rz),
        _ => Err(format!("invalid line numbering: '{}'", s)),
    }
}

/// Logical page section types.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    Header,
    Body,
    Footer,
}

/// Check if a line is a section delimiter.
#[inline]
fn check_section_delimiter(line: &[u8], delim: &[u8]) -> Option<Section> {
    if delim.is_empty() {
        return None;
    }
    let dlen = delim.len();

    // Check header (3x)
    if line.len() == dlen * 3 {
        let mut is_header = true;
        for i in 0..3 {
            if &line[i * dlen..(i + 1) * dlen] != delim {
                is_header = false;
                break;
            }
        }
        if is_header {
            return Some(Section::Header);
        }
    }

    // Check body (2x)
    if line.len() == dlen * 2 && &line[..dlen] == delim && &line[dlen..] == delim {
        return Some(Section::Body);
    }

    // Check footer (1x)
    if line.len() == dlen && line == delim {
        return Some(Section::Footer);
    }

    None
}

/// Format a line number according to the format and width.
#[inline]
fn format_number(num: i64, format: NumberFormat, width: usize, buf: &mut Vec<u8>) {
    let mut num_buf = itoa::Buffer::new();
    let num_str = num_buf.format(num);

    match format {
        NumberFormat::Ln => {
            buf.extend_from_slice(num_str.as_bytes());
            let pad = width.saturating_sub(num_str.len());
            buf.resize(buf.len() + pad, b' ');
        }
        NumberFormat::Rn => {
            let pad = width.saturating_sub(num_str.len());
            buf.resize(buf.len() + pad, b' ');
            buf.extend_from_slice(num_str.as_bytes());
        }
        NumberFormat::Rz => {
            if num < 0 {
                buf.push(b'-');
                let abs_str = &num_str[1..];
                let pad = width.saturating_sub(abs_str.len() + 1);
                buf.resize(buf.len() + pad, b'0');
                buf.extend_from_slice(abs_str.as_bytes());
            } else {
                let pad = width.saturating_sub(num_str.len());
                buf.resize(buf.len() + pad, b'0');
                buf.extend_from_slice(num_str.as_bytes());
            }
        }
    }
}

/// Check if a line should be numbered based on the style.
#[inline]
fn should_number(line: &[u8], style: &NumberingStyle) -> bool {
    match style {
        NumberingStyle::All => true,
        NumberingStyle::NonEmpty => !line.is_empty(),
        NumberingStyle::None => false,
        NumberingStyle::Regex(re) => match std::str::from_utf8(line) {
            Ok(s) => re.is_match(s),
            Err(_) => false,
        },
    }
}

/// Build the nl output into a Vec.
pub fn nl_to_vec(data: &[u8], config: &NlConfig) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }

    let estimated_lines = memchr::memchr_iter(b'\n', data).count() + 1;
    let prefix_size = config.number_width + config.number_separator.len() + 2;
    let mut output = Vec::with_capacity(data.len() + estimated_lines * prefix_size);

    let mut line_number = config.starting_line_number;
    let mut current_section = Section::Body;
    let mut consecutive_blanks: usize = 0;

    let mut start = 0;
    let mut line_iter = memchr::memchr_iter(b'\n', data);

    loop {
        let (line, has_newline) = match line_iter.next() {
            Some(pos) => (&data[start..pos], true),
            None => {
                if start < data.len() {
                    (&data[start..], false)
                } else {
                    break;
                }
            }
        };

        // Check for section delimiter
        if let Some(section) = check_section_delimiter(line, &config.section_delimiter) {
            if !config.no_renumber {
                line_number = config.starting_line_number;
            }
            current_section = section;
            consecutive_blanks = 0;
            output.push(b'\n');
            if has_newline {
                start += line.len() + 1;
            } else {
                break;
            }
            continue;
        }

        let style = match current_section {
            Section::Header => &config.header_style,
            Section::Body => &config.body_style,
            Section::Footer => &config.footer_style,
        };

        let is_blank = line.is_empty();

        if is_blank {
            consecutive_blanks += 1;
        } else {
            consecutive_blanks = 0;
        }

        let do_number = if is_blank && config.join_blank_lines > 1 {
            if should_number(line, style) {
                consecutive_blanks >= config.join_blank_lines
            } else {
                false
            }
        } else {
            should_number(line, style)
        };

        if do_number {
            if is_blank && config.join_blank_lines > 1 {
                consecutive_blanks = 0;
            }
            format_number(
                line_number,
                config.number_format,
                config.number_width,
                &mut output,
            );
            output.extend_from_slice(&config.number_separator);
            output.extend_from_slice(line);
            line_number = line_number.wrapping_add(config.line_increment);
        } else {
            // Non-numbered lines: GNU nl outputs width + separator_len total spaces, then content
            let total_pad = config.number_width + config.number_separator.len();
            output.resize(output.len() + total_pad, b' ');
            output.extend_from_slice(line);
        }

        if has_newline {
            output.push(b'\n');
            start += line.len() + 1;
        } else {
            break;
        }
    }

    output
}

/// Number lines and write to the provided writer.
pub fn nl(data: &[u8], config: &NlConfig, out: &mut impl Write) -> std::io::Result<()> {
    let output = nl_to_vec(data, config);
    out.write_all(&output)
}
