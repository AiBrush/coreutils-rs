use std::io::{self, Read, Write};

/// Address radix for the offset column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressRadix {
    Octal,
    Decimal,
    Hex,
    None,
}

/// Output format specifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Named character (a): nul, soh, stx, ...
    NamedChar,
    /// Printable character or backslash escape (c): \0, \a, \b, \t, \n, ...
    PrintableChar,
    /// Signed decimal integer of given byte size (d1, d2, d4, d8)
    SignedDec(usize),
    /// Floating point of given byte size (f4, f8)
    Float(usize),
    /// Octal integer of given byte size (o1, o2, o4)
    Octal(usize),
    /// Unsigned decimal integer of given byte size (u1, u2, u4, u8)
    UnsignedDec(usize),
    /// Hexadecimal integer of given byte size (x1, x2, x4, x8)
    Hex(usize),
}

/// Configuration for the od command.
#[derive(Debug, Clone)]
pub struct OdConfig {
    pub address_radix: AddressRadix,
    pub formats: Vec<OutputFormat>,
    pub skip_bytes: u64,
    pub read_bytes: Option<u64>,
    pub width: usize,
    pub show_duplicates: bool,
}

impl Default for OdConfig {
    fn default() -> Self {
        Self {
            address_radix: AddressRadix::Octal,
            formats: vec![OutputFormat::Octal(2)],
            skip_bytes: 0,
            read_bytes: None,
            width: 16,
            show_duplicates: false,
        }
    }
}

/// Named characters for -t a format (ASCII named characters).
/// Index 0..=127 maps to the name for that byte value.
const NAMED_CHARS: [&str; 128] = [
    "nul", "soh", "stx", "etx", "eot", "enq", "ack", "bel", " bs", " ht", " nl", " vt", " ff",
    " cr", " so", " si", "dle", "dc1", "dc2", "dc3", "dc4", "nak", "syn", "etb", "can", " em",
    "sub", "esc", " fs", " gs", " rs", " us", " sp", "!", "\"", "#", "$", "%", "&", "'", "(", ")",
    "*", "+", ",", "-", ".", "/", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", ":", ";", "<",
    "=", ">", "?", "@", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O",
    "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z", "[", "\\", "]", "^", "_", "`", "a", "b",
    "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u",
    "v", "w", "x", "y", "z", "{", "|", "}", "~", "del",
];

/// Format an address according to the radix.
fn format_address(offset: u64, radix: AddressRadix) -> String {
    match radix {
        AddressRadix::Octal => format!("{:07o}", offset),
        AddressRadix::Decimal => format!("{:07}", offset),
        AddressRadix::Hex => format!("{:06x}", offset),
        AddressRadix::None => String::new(),
    }
}

/// Return the field width for a single value of the given format.
/// This matches GNU od's column widths.
fn field_width(fmt: OutputFormat) -> usize {
    match fmt {
        OutputFormat::NamedChar => 4, // 3 chars + leading space => " nul" = 4 wide
        OutputFormat::PrintableChar => 4, // 3 chars + leading space => "  \\n" = 4 wide
        OutputFormat::Octal(1) => 4,  // " 377"
        OutputFormat::Octal(2) => 7,  // " 177777"
        OutputFormat::Octal(4) => 12, // " 37777777777"
        OutputFormat::Octal(8) => 23, // " 1777777777777777777777"
        OutputFormat::Hex(1) => 3,    // " ff"
        OutputFormat::Hex(2) => 5,    // " ffff"
        OutputFormat::Hex(4) => 9,    // " ffffffff"
        OutputFormat::Hex(8) => 17,   // " ffffffffffffffff"
        OutputFormat::UnsignedDec(1) => 4, // " 255"
        OutputFormat::UnsignedDec(2) => 6, // " 65535"
        OutputFormat::UnsignedDec(4) => 11, // " 4294967295"
        OutputFormat::UnsignedDec(8) => 21, // " 18446744073709551615"
        OutputFormat::SignedDec(1) => 5, // " -128"
        OutputFormat::SignedDec(2) => 7, // " -32768"
        OutputFormat::SignedDec(4) => 12, // " -2147483648"
        OutputFormat::SignedDec(8) => 21, // " -9223372036854775808"
        OutputFormat::Float(4) => 15, // " -x.xxxxxxxe+xx"
        OutputFormat::Float(8) => 25, // " -x.xxxxxxxxxxxxxxe+xxx"
        _ => 4,
    }
}

/// Get the byte size of a format element.
fn element_size(fmt: OutputFormat) -> usize {
    match fmt {
        OutputFormat::NamedChar | OutputFormat::PrintableChar => 1,
        OutputFormat::SignedDec(s)
        | OutputFormat::Float(s)
        | OutputFormat::Octal(s)
        | OutputFormat::UnsignedDec(s)
        | OutputFormat::Hex(s) => s,
    }
}

/// Format a single value for the given format.
fn format_value(bytes: &[u8], fmt: OutputFormat, width: usize) -> String {
    match fmt {
        OutputFormat::NamedChar => {
            let b = bytes[0];
            if b < 128 {
                format!("{:>w$}", NAMED_CHARS[b as usize], w = width)
            } else {
                format!("{:>w$}", format!("{:03o}", b), w = width)
            }
        }
        OutputFormat::PrintableChar => {
            let b = bytes[0];
            let s = match b {
                0x00 => "\\0".to_string(),
                0x07 => "\\a".to_string(),
                0x08 => "\\b".to_string(),
                0x09 => "\\t".to_string(),
                0x0a => "\\n".to_string(),
                0x0b => "\\v".to_string(),
                0x0c => "\\f".to_string(),
                0x0d => "\\r".to_string(),
                0x20..=0x7e => format!("{}", b as char),
                _ => format!("{:03o}", b),
            };
            format!("{:>w$}", s, w = width)
        }
        OutputFormat::Octal(size) => match size {
            1 => format!("{:>w$}", format!("{:03o}", bytes[0]), w = width),
            2 => {
                let v = u16::from_le_bytes(bytes[..2].try_into().unwrap());
                format!("{:>w$}", format!("{:06o}", v), w = width)
            }
            4 => {
                let v = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                format!("{:>w$}", format!("{:011o}", v), w = width)
            }
            8 => {
                let v = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                format!("{:>w$}", format!("{:022o}", v), w = width)
            }
            _ => String::new(),
        },
        OutputFormat::Hex(size) => match size {
            1 => format!("{:>w$}", format!("{:02x}", bytes[0]), w = width),
            2 => {
                let v = u16::from_le_bytes(bytes[..2].try_into().unwrap());
                format!("{:>w$}", format!("{:04x}", v), w = width)
            }
            4 => {
                let v = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                format!("{:>w$}", format!("{:08x}", v), w = width)
            }
            8 => {
                let v = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                format!("{:>w$}", format!("{:016x}", v), w = width)
            }
            _ => String::new(),
        },
        OutputFormat::UnsignedDec(size) => match size {
            1 => format!("{:>w$}", bytes[0], w = width),
            2 => {
                let v = u16::from_le_bytes(bytes[..2].try_into().unwrap());
                format!("{:>w$}", v, w = width)
            }
            4 => {
                let v = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                format!("{:>w$}", v, w = width)
            }
            8 => {
                let v = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                format!("{:>w$}", v, w = width)
            }
            _ => String::new(),
        },
        OutputFormat::SignedDec(size) => match size {
            1 => format!("{:>w$}", bytes[0] as i8, w = width),
            2 => {
                let v = i16::from_le_bytes(bytes[..2].try_into().unwrap());
                format!("{:>w$}", v, w = width)
            }
            4 => {
                let v = i32::from_le_bytes(bytes[..4].try_into().unwrap());
                format!("{:>w$}", v, w = width)
            }
            8 => {
                let v = i64::from_le_bytes(bytes[..8].try_into().unwrap());
                format!("{:>w$}", v, w = width)
            }
            _ => String::new(),
        },
        OutputFormat::Float(size) => match size {
            4 => {
                let v = f32::from_le_bytes(bytes[..4].try_into().unwrap());
                format!("{:>w$}", format_float_f32(v), w = width)
            }
            8 => {
                let v = f64::from_le_bytes(bytes[..8].try_into().unwrap());
                format!("{:>w$}", format_float_f64(v), w = width)
            }
            _ => String::new(),
        },
    }
}

/// Format a float using C's %g format.
/// Uses libc snprintf on Unix and Rust formatting on Windows.
fn snprintf_g(v: f64, precision: usize) -> String {
    let precision = precision.min(50);
    #[cfg(unix)]
    {
        let mut buf = [0u8; 64];
        let fmt = std::ffi::CString::new(format!("%.{}g", precision)).unwrap();
        let len = unsafe {
            libc::snprintf(
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                fmt.as_ptr(),
                v,
            )
        };
        if len > 0 && (len as usize) < buf.len() {
            return String::from_utf8_lossy(&buf[..len as usize]).into_owned();
        }
    }
    // Fallback / Windows: use Rust formatting with %g-like behavior
    let s = format!("{:.prec$e}", v, prec = precision.saturating_sub(1));
    // Convert scientific notation to shortest form like %g
    if let Some(e_pos) = s.find('e') {
        let exp: i32 = s[e_pos + 1..].parse().unwrap_or(0);
        if exp >= -(precision as i32) && exp < precision as i32 {
            // Use fixed notation
            let fixed = format!(
                "{:.prec$}",
                v,
                prec = (precision as i32 - 1 - exp).max(0) as usize
            );
            // Trim trailing zeros after decimal point
            if fixed.contains('.') {
                let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
                return trimmed.to_string();
            }
            return fixed;
        }
    }
    format!("{:.*e}", precision.saturating_sub(1), v)
}

/// Format f32 like GNU od: uses %.7g formatting.
fn format_float_f32(v: f32) -> String {
    snprintf_g(v as f64, 7)
}

/// Format f64 like GNU od: uses %.17g formatting.
fn format_float_f64(v: f64) -> String {
    snprintf_g(v, 17)
}

/// Format one line of output for a given format type.
fn format_line(
    chunk: &[u8],
    fmt: OutputFormat,
    line_width: usize,
    is_first_format: bool,
    radix: AddressRadix,
    offset: u64,
) -> String {
    let mut line = String::new();

    // Address prefix
    if is_first_format {
        line.push_str(&format_address(offset, radix));
    } else if radix != AddressRadix::None {
        // Continuation lines: pad with spaces to match address width
        let addr_width = match radix {
            AddressRadix::Octal => 7,
            AddressRadix::Decimal => 7,
            AddressRadix::Hex => 6,
            AddressRadix::None => 0,
        };
        for _ in 0..addr_width {
            line.push(' ');
        }
    }

    let elem_sz = element_size(fmt);
    let fw = field_width(fmt);

    // Number of full elements in this chunk
    let num_elems = line_width / elem_sz;

    // How many elements we can actually format from this (possibly short) chunk
    let actual_full = chunk.len() / elem_sz;
    let remainder = chunk.len() % elem_sz;

    for i in 0..num_elems {
        if i < actual_full {
            let start = i * elem_sz;
            let end = start + elem_sz;
            line.push_str(&format_value(&chunk[start..end], fmt, fw));
        } else if i == actual_full && remainder > 0 {
            // Partial element at the end: pad with zeros
            let start = i * elem_sz;
            let mut padded = vec![0u8; elem_sz];
            padded[..remainder].copy_from_slice(&chunk[start..]);
            line.push_str(&format_value(&padded, fmt, fw));
        }
    }

    line
}

/// Parse a format type string (the TYPE argument of -t).
pub fn parse_format_type(s: &str) -> Result<OutputFormat, String> {
    if s.is_empty() {
        return Err("empty format string".to_string());
    }

    let mut chars = s.chars();
    let type_char = chars.next().unwrap();
    let size_str: String = chars.collect();

    match type_char {
        'a' => Ok(OutputFormat::NamedChar),
        'c' => Ok(OutputFormat::PrintableChar),
        'd' => {
            let size = if size_str.is_empty() {
                4
            } else {
                parse_size_spec(&size_str, "d")?
            };
            Ok(OutputFormat::SignedDec(size))
        }
        'f' => {
            let size = if size_str.is_empty() {
                4
            } else {
                parse_float_size(&size_str)?
            };
            Ok(OutputFormat::Float(size))
        }
        'o' => {
            let size = if size_str.is_empty() {
                2
            } else {
                parse_size_spec(&size_str, "o")?
            };
            Ok(OutputFormat::Octal(size))
        }
        'u' => {
            let size = if size_str.is_empty() {
                4
            } else {
                parse_size_spec(&size_str, "u")?
            };
            Ok(OutputFormat::UnsignedDec(size))
        }
        'x' => {
            let size = if size_str.is_empty() {
                2
            } else {
                parse_size_spec(&size_str, "x")?
            };
            Ok(OutputFormat::Hex(size))
        }
        _ => Err(format!("invalid type string '{}'", s)),
    }
}

fn parse_size_spec(s: &str, type_name: &str) -> Result<usize, String> {
    // Accept C, S, I, L or a number
    match s {
        "C" => Ok(1),
        "S" => Ok(2),
        "I" => Ok(4),
        "L" => Ok(8),
        _ => {
            let n: usize = s
                .parse()
                .map_err(|_| format!("invalid type string '{}{}': invalid size", type_name, s))?;
            match n {
                1 | 2 | 4 | 8 => Ok(n),
                _ => Err(format!(
                    "invalid type string '{}{}': invalid size",
                    type_name, s
                )),
            }
        }
    }
}

fn parse_float_size(s: &str) -> Result<usize, String> {
    match s {
        "F" | "4" => Ok(4),
        "D" | "8" => Ok(8),
        "L" | "16" => Err("16-byte float not supported".to_string()),
        _ => {
            let n: usize = s
                .parse()
                .map_err(|_| format!("invalid float size '{}'", s))?;
            match n {
                4 | 8 => Ok(n),
                _ => Err(format!("invalid float size '{}'", s)),
            }
        }
    }
}

/// Process input and produce od output.
pub fn od_process<R: Read, W: Write>(
    mut input: R,
    output: &mut W,
    config: &OdConfig,
) -> io::Result<()> {
    // Skip bytes
    if config.skip_bytes > 0 {
        let mut to_skip = config.skip_bytes;
        let mut skip_buf = [0u8; 8192];
        while to_skip > 0 {
            let chunk_size = std::cmp::min(to_skip, skip_buf.len() as u64) as usize;
            let n = input.read(&mut skip_buf[..chunk_size])?;
            if n == 0 {
                break;
            }
            to_skip -= n as u64;
        }
    }

    // Read all data (respecting read_bytes limit)
    let data = match config.read_bytes {
        Some(limit) => {
            let mut buf = Vec::new();
            let mut limited = input.take(limit);
            limited.read_to_end(&mut buf)?;
            buf
        }
        None => {
            let mut buf = Vec::new();
            input.read_to_end(&mut buf)?;
            buf
        }
    };

    let width = config.width;
    let mut offset = config.skip_bytes;
    let mut prev_chunk: Option<Vec<u8>> = None;
    let mut star_printed = false;

    let mut pos = 0;
    while pos < data.len() {
        let end = std::cmp::min(pos + width, data.len());
        let chunk = &data[pos..end];

        // Duplicate suppression
        if !config.show_duplicates && chunk.len() == width {
            if let Some(ref prev) = prev_chunk {
                if prev.as_slice() == chunk {
                    if !star_printed {
                        writeln!(output, "*")?;
                        star_printed = true;
                    }
                    pos += width;
                    offset += width as u64;
                    continue;
                }
            }
        }

        star_printed = false;

        for (i, fmt) in config.formats.iter().enumerate() {
            let line = format_line(chunk, *fmt, width, i == 0, config.address_radix, offset);
            writeln!(output, "{}", line)?;
        }

        prev_chunk = Some(chunk.to_vec());
        pos += width;
        offset += width as u64;
    }

    // Final address line
    if config.address_radix != AddressRadix::None {
        // The final offset is skip_bytes + actual data length
        let final_offset = config.skip_bytes + data.len() as u64;
        writeln!(
            output,
            "{}",
            format_address(final_offset, config.address_radix)
        )?;
    }

    Ok(())
}
