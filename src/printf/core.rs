/// GNU coreutils-compatible printf implementation.
///
/// Processes a printf format string with the given arguments, returning the
/// raw output bytes. The format string is reused if there are more arguments
/// than a single pass consumes.

/// Sentinel returned inside `process_format_string` when `\c` is encountered.
const STOP_OUTPUT: u8 = 0xFF;

/// Process a printf format string with the given arguments, returning raw bytes.
///
/// The format string repeats if there are more arguments than one pass consumes.
/// Processing stops immediately when `\c` is encountered (in the format string
/// itself or inside a `%b` argument).
pub fn process_format_string(format: &str, args: &[&str]) -> Vec<u8> {
    let mut output = Vec::with_capacity(256);
    let fmt_bytes = format.as_bytes();

    if args.is_empty() {
        // Single pass with no arguments
        let stop = format_one_pass(fmt_bytes, args, &mut 0, &mut output);
        if stop {
            // remove trailing STOP_OUTPUT sentinel if present
            if output.last() == Some(&STOP_OUTPUT) {
                output.pop();
            }
        }
        return output;
    }

    let mut arg_idx: usize = 0;
    loop {
        let start_idx = arg_idx;
        let stop = format_one_pass(fmt_bytes, args, &mut arg_idx, &mut output);
        if stop {
            if output.last() == Some(&STOP_OUTPUT) {
                output.pop();
            }
            break;
        }
        // If no arguments were consumed, or we've used them all, stop
        if arg_idx == start_idx || arg_idx >= args.len() {
            break;
        }
    }

    output
}

/// Run one pass of the format string. Returns `true` if output should stop (`\c`).
/// `arg_idx` is advanced as arguments are consumed.
fn format_one_pass(fmt: &[u8], args: &[&str], arg_idx: &mut usize, output: &mut Vec<u8>) -> bool {
    let mut i = 0;
    while i < fmt.len() {
        match fmt[i] {
            b'%' => {
                i += 1;
                if i >= fmt.len() {
                    output.push(b'%');
                    break;
                }
                if fmt[i] == b'%' {
                    output.push(b'%');
                    i += 1;
                    continue;
                }
                let stop = process_conversion(fmt, &mut i, args, arg_idx, output);
                if stop {
                    return true;
                }
            }
            b'\\' => {
                i += 1;
                let stop = process_format_escape(fmt, &mut i, output);
                if stop {
                    return true;
                }
            }
            ch => {
                output.push(ch);
                i += 1;
            }
        }
    }
    false
}

/// Process a conversion specifier (the part after `%`).
/// `i` points to the first character after `%`. Returns true if `\c` stop was hit.
fn process_conversion(
    fmt: &[u8],
    i: &mut usize,
    args: &[&str],
    arg_idx: &mut usize,
    output: &mut Vec<u8>,
) -> bool {
    // Parse flags
    let mut flags = FormatFlags::default();
    while *i < fmt.len() {
        match fmt[*i] {
            b'-' => flags.left_align = true,
            b'+' => flags.plus_sign = true,
            b' ' => flags.space_sign = true,
            b'0' => flags.zero_pad = true,
            b'#' => flags.alternate = true,
            _ => break,
        }
        *i += 1;
    }

    // Parse width
    let width = parse_decimal(fmt, i);

    // Parse precision
    let precision = if *i < fmt.len() && fmt[*i] == b'.' {
        *i += 1;
        Some(parse_decimal(fmt, i))
    } else {
        None
    };

    // Parse conversion character
    if *i >= fmt.len() {
        return false;
    }
    let conv = fmt[*i];
    *i += 1;

    let arg = consume_arg(args, arg_idx);

    match conv {
        b's' => {
            let s = arg;
            let formatted = apply_string_format(s, &flags, width, precision);
            output.extend_from_slice(&formatted);
        }
        b'b' => {
            let (bytes, stop) = process_b_argument(arg);
            let formatted = apply_string_format_bytes(&bytes, &flags, width, precision);
            output.extend_from_slice(&formatted);
            if stop {
                return true;
            }
        }
        b'c' => {
            if let Some(ch) = arg.chars().next() {
                let mut buf = [0u8; 4];
                let encoded = ch.encode_utf8(&mut buf);
                let formatted = apply_string_format(encoded, &flags, width, precision);
                output.extend_from_slice(&formatted);
            } else {
                // empty arg: output a NUL byte (GNU compat)
                let formatted = apply_string_format_bytes(&[0], &flags, width, precision);
                output.extend_from_slice(&formatted);
            }
        }
        b'd' | b'i' => {
            let val = parse_integer(arg);
            let s = format!("{}", val);
            let formatted = apply_numeric_format(&s, val < 0, &flags, width, precision);
            output.extend_from_slice(formatted.as_bytes());
        }
        b'u' => {
            let val = parse_unsigned(arg);
            let s = format!("{}", val);
            let formatted = apply_numeric_format(&s, false, &flags, width, precision);
            output.extend_from_slice(formatted.as_bytes());
        }
        b'o' => {
            let val = parse_unsigned(arg);
            let s = format!("{:o}", val);
            let prefix = if flags.alternate && !s.starts_with('0') {
                "0"
            } else {
                ""
            };
            let full = format!("{}{}", prefix, s);
            let formatted = apply_numeric_format(&full, false, &flags, width, precision);
            output.extend_from_slice(formatted.as_bytes());
        }
        b'x' => {
            let val = parse_unsigned(arg);
            let s = format!("{:x}", val);
            let prefix = if flags.alternate && val != 0 {
                "0x"
            } else {
                ""
            };
            let full = format!("{}{}", prefix, s);
            let formatted = apply_numeric_format(&full, false, &flags, width, precision);
            output.extend_from_slice(formatted.as_bytes());
        }
        b'X' => {
            let val = parse_unsigned(arg);
            let s = format!("{:X}", val);
            let prefix = if flags.alternate && val != 0 {
                "0X"
            } else {
                ""
            };
            let full = format!("{}{}", prefix, s);
            let formatted = apply_numeric_format(&full, false, &flags, width, precision);
            output.extend_from_slice(formatted.as_bytes());
        }
        b'f' => {
            let val = parse_float(arg);
            let prec = precision.unwrap_or(6);
            let s = format!("{:.prec$}", val, prec = prec);
            let formatted = apply_float_format(&s, val < 0.0, &flags, width);
            output.extend_from_slice(formatted.as_bytes());
        }
        b'e' => {
            let val = parse_float(arg);
            let prec = precision.unwrap_or(6);
            let s = format_scientific(val, prec, 'e');
            let formatted = apply_float_format(&s, val < 0.0, &flags, width);
            output.extend_from_slice(formatted.as_bytes());
        }
        b'E' => {
            let val = parse_float(arg);
            let prec = precision.unwrap_or(6);
            let s = format_scientific(val, prec, 'E');
            let formatted = apply_float_format(&s, val < 0.0, &flags, width);
            output.extend_from_slice(formatted.as_bytes());
        }
        b'g' => {
            let val = parse_float(arg);
            let prec = precision.unwrap_or(6);
            let s = format_g(val, prec, false);
            let formatted = apply_float_format(&s, val < 0.0, &flags, width);
            output.extend_from_slice(formatted.as_bytes());
        }
        b'G' => {
            let val = parse_float(arg);
            let prec = precision.unwrap_or(6);
            let s = format_g(val, prec, true);
            let formatted = apply_float_format(&s, val < 0.0, &flags, width);
            output.extend_from_slice(formatted.as_bytes());
        }
        b'q' => {
            let s = arg;
            let quoted = shell_quote(s);
            let formatted = apply_string_format(&quoted, &flags, width, precision);
            output.extend_from_slice(&formatted);
        }
        _ => {
            // Unknown conversion: output literally
            output.push(b'%');
            output.push(conv);
        }
    }
    false
}

/// Consume the next argument, returning "" if exhausted.
fn consume_arg<'a>(args: &[&'a str], arg_idx: &mut usize) -> &'a str {
    if *arg_idx < args.len() {
        let val = args[*arg_idx];
        *arg_idx += 1;
        val
    } else {
        ""
    }
}

/// Process an escape sequence in the format string.
/// `i` points to the character after `\`. Returns true if `\c` was encountered.
fn process_format_escape(fmt: &[u8], i: &mut usize, output: &mut Vec<u8>) -> bool {
    if *i >= fmt.len() {
        output.push(b'\\');
        return false;
    }
    match fmt[*i] {
        b'\\' => {
            output.push(b'\\');
            *i += 1;
        }
        b'"' => {
            output.push(b'"');
            *i += 1;
        }
        b'a' => {
            output.push(0x07);
            *i += 1;
        }
        b'b' => {
            output.push(0x08);
            *i += 1;
        }
        b'c' => {
            return true;
        }
        b'e' | b'E' => {
            output.push(0x1B);
            *i += 1;
        }
        b'f' => {
            output.push(0x0C);
            *i += 1;
        }
        b'n' => {
            output.push(b'\n');
            *i += 1;
        }
        b'r' => {
            output.push(b'\r');
            *i += 1;
        }
        b't' => {
            output.push(b'\t');
            *i += 1;
        }
        b'v' => {
            output.push(0x0B);
            *i += 1;
        }
        b'0' => {
            // Octal: \0NNN (up to 3 octal digits after the leading 0)
            *i += 1;
            let val = parse_octal_digits(fmt, i, 3);
            output.push(val);
        }
        b'1'..=b'7' => {
            // Octal: \NNN (up to 3 octal digits)
            let val = parse_octal_digits(fmt, i, 3);
            output.push(val);
        }
        b'x' => {
            *i += 1;
            let val = parse_hex_digits(fmt, i, 2);
            output.push(val as u8);
        }
        b'u' => {
            *i += 1;
            let val = parse_hex_digits(fmt, i, 4);
            if let Some(ch) = char::from_u32(val) {
                let mut buf = [0u8; 4];
                let encoded = ch.encode_utf8(&mut buf);
                output.extend_from_slice(encoded.as_bytes());
            }
        }
        b'U' => {
            *i += 1;
            let val = parse_hex_digits(fmt, i, 8);
            if let Some(ch) = char::from_u32(val) {
                let mut buf = [0u8; 4];
                let encoded = ch.encode_utf8(&mut buf);
                output.extend_from_slice(encoded.as_bytes());
            }
        }
        _ => {
            // Unknown escape: output backslash and the character
            output.push(b'\\');
            output.push(fmt[*i]);
            *i += 1;
        }
    }
    false
}

/// Process backslash escapes in a %b argument string.
/// Returns (bytes, stop) where stop is true if \c was found.
fn process_b_argument(arg: &str) -> (Vec<u8>, bool) {
    let bytes = arg.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 1;
            if i >= bytes.len() {
                output.push(b'\\');
                break;
            }
            match bytes[i] {
                b'\\' => {
                    output.push(b'\\');
                    i += 1;
                }
                b'a' => {
                    output.push(0x07);
                    i += 1;
                }
                b'b' => {
                    output.push(0x08);
                    i += 1;
                }
                b'c' => {
                    return (output, true);
                }
                b'e' | b'E' => {
                    output.push(0x1B);
                    i += 1;
                }
                b'f' => {
                    output.push(0x0C);
                    i += 1;
                }
                b'n' => {
                    output.push(b'\n');
                    i += 1;
                }
                b'r' => {
                    output.push(b'\r');
                    i += 1;
                }
                b't' => {
                    output.push(b'\t');
                    i += 1;
                }
                b'v' => {
                    output.push(0x0B);
                    i += 1;
                }
                b'0' => {
                    i += 1;
                    let val = parse_octal_digits(bytes, &mut i, 3);
                    output.push(val);
                }
                b'1'..=b'7' => {
                    let val = parse_octal_digits(bytes, &mut i, 3);
                    output.push(val);
                }
                b'x' => {
                    i += 1;
                    let val = parse_hex_digits(bytes, &mut i, 2);
                    output.push(val as u8);
                }
                _ => {
                    // In %b, unknown escapes pass through literally
                    output.push(b'\\');
                    output.push(bytes[i]);
                    i += 1;
                }
            }
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    (output, false)
}

/// Parse up to `max_digits` octal digits from `data` starting at `*i`.
fn parse_octal_digits(data: &[u8], i: &mut usize, max_digits: usize) -> u8 {
    let mut val: u32 = 0;
    let mut count = 0;
    while *i < data.len() && count < max_digits {
        let ch = data[*i];
        if ch >= b'0' && ch <= b'7' {
            val = val * 8 + (ch - b'0') as u32;
            *i += 1;
            count += 1;
        } else {
            break;
        }
    }
    (val & 0xFF) as u8
}

/// Parse up to `max_digits` hex digits from `data` starting at `*i`.
fn parse_hex_digits(data: &[u8], i: &mut usize, max_digits: usize) -> u32 {
    let mut val: u32 = 0;
    let mut count = 0;
    while *i < data.len() && count < max_digits {
        let ch = data[*i];
        if ch.is_ascii_hexdigit() {
            val = val * 16 + hex_digit_value(ch) as u32;
            *i += 1;
            count += 1;
        } else {
            break;
        }
    }
    val
}

fn hex_digit_value(ch: u8) -> u8 {
    match ch {
        b'0'..=b'9' => ch - b'0',
        b'a'..=b'f' => ch - b'a' + 10,
        b'A'..=b'F' => ch - b'A' + 10,
        _ => 0,
    }
}

/// Parse a decimal integer from `data` at position `*i`.
fn parse_decimal(data: &[u8], i: &mut usize) -> usize {
    let mut val: usize = 0;
    while *i < data.len() && data[*i].is_ascii_digit() {
        val = val
            .saturating_mul(10)
            .saturating_add((data[*i] - b'0') as usize);
        *i += 1;
    }
    val
}

/// Parse an integer argument. Supports decimal, octal (0-prefix), hex (0x-prefix),
/// and single-character constants ('c' or "c").
fn parse_integer(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }

    // Character constants: 'X or "X
    if (s.starts_with('\'') || s.starts_with('"')) && s.len() >= 2 {
        return s[1..].chars().next().map_or(0, |c| c as i64);
    }

    // Try to detect sign
    let (negative, digits) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };

    let magnitude = if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).unwrap_or(0)
    } else if let Some(oct) = digits.strip_prefix('0') {
        if oct.is_empty() {
            0
        } else {
            u64::from_str_radix(oct, 8).unwrap_or(0)
        }
    } else {
        digits.parse::<u64>().unwrap_or(0)
    };

    if negative {
        -(magnitude as i64)
    } else {
        magnitude as i64
    }
}

/// Parse an unsigned integer argument.
fn parse_unsigned(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }

    // Character constants
    if (s.starts_with('\'') || s.starts_with('"')) && s.len() >= 2 {
        return s[1..].chars().next().map_or(0, |c| c as u64);
    }

    // Negative values wrap around like C unsigned
    let (negative, digits) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };

    let magnitude = if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).unwrap_or(0)
    } else if let Some(oct) = digits.strip_prefix('0') {
        if oct.is_empty() {
            0
        } else {
            u64::from_str_radix(oct, 8).unwrap_or(0)
        }
    } else {
        digits.parse::<u64>().unwrap_or(0)
    };

    if negative {
        magnitude.wrapping_neg()
    } else {
        magnitude
    }
}

/// Parse a floating-point argument.
fn parse_float(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }

    // Character constants
    if (s.starts_with('\'') || s.starts_with('"')) && s.len() >= 2 {
        return s[1..].chars().next().map_or(0.0, |c| c as u32 as f64);
    }

    // Handle hex float prefix for parsing
    if s.starts_with("0x") || s.starts_with("0X") || s.starts_with("-0x") || s.starts_with("-0X") {
        // Rust doesn't parse hex floats natively; parse as integer
        return parse_integer(s) as f64;
    }

    s.parse::<f64>().unwrap_or(0.0)
}

#[derive(Default)]
struct FormatFlags {
    left_align: bool,
    plus_sign: bool,
    space_sign: bool,
    zero_pad: bool,
    alternate: bool,
}

/// Apply string formatting with width and precision (for %s, %b, %c).
fn apply_string_format(
    s: &str,
    flags: &FormatFlags,
    width: usize,
    precision: Option<usize>,
) -> Vec<u8> {
    let truncated: &str;
    let owned: String;
    if let Some(prec) = precision {
        if s.len() > prec {
            // Truncate to prec bytes, but respect UTF-8 boundaries
            owned = s.chars().take(prec).collect();
            truncated = &owned;
        } else {
            truncated = s;
        }
    } else {
        truncated = s;
    }

    apply_padding(truncated.as_bytes(), flags, width)
}

/// Apply string formatting for raw bytes.
fn apply_string_format_bytes(
    s: &[u8],
    flags: &FormatFlags,
    width: usize,
    precision: Option<usize>,
) -> Vec<u8> {
    let data = if let Some(prec) = precision {
        if s.len() > prec { &s[..prec] } else { s }
    } else {
        s
    };

    apply_padding(data, flags, width)
}

/// Apply padding (left or right) to reach the desired width.
fn apply_padding(data: &[u8], flags: &FormatFlags, width: usize) -> Vec<u8> {
    if width == 0 || data.len() >= width {
        return data.to_vec();
    }
    let pad_len = width - data.len();
    let mut result = Vec::with_capacity(width);
    if flags.left_align {
        result.extend_from_slice(data);
        result.resize(result.len() + pad_len, b' ');
    } else {
        result.resize(pad_len, b' ');
        result.extend_from_slice(data);
    }
    result
}

/// Apply numeric formatting with width, flags, and optional precision for integers.
fn apply_numeric_format(
    num_str: &str,
    is_negative: bool,
    flags: &FormatFlags,
    width: usize,
    precision: Option<usize>,
) -> String {
    // For integers, precision specifies minimum number of digits
    let digits = if is_negative {
        &num_str[1..] // strip the minus
    } else {
        num_str
    };

    let digits = if let Some(prec) = precision {
        if prec > 0 && digits.len() < prec {
            let padding = "0".repeat(prec - digits.len());
            format!("{}{}", padding, digits)
        } else if prec == 0 && digits == "0" {
            String::new()
        } else {
            digits.to_string()
        }
    } else {
        digits.to_string()
    };

    let sign = if is_negative {
        "-".to_string()
    } else if flags.plus_sign {
        "+".to_string()
    } else if flags.space_sign {
        " ".to_string()
    } else {
        String::new()
    };

    let content = format!("{}{}", sign, digits);

    if width > 0 && content.len() < width {
        let pad_len = width - content.len();
        if flags.left_align {
            format!("{}{}", content, " ".repeat(pad_len))
        } else if flags.zero_pad && precision.is_none() {
            format!("{}{}{}", sign, "0".repeat(pad_len), digits)
        } else {
            format!("{}{}", " ".repeat(pad_len), content)
        }
    } else {
        content
    }
}

/// Apply float formatting with width and flags.
fn apply_float_format(
    num_str: &str,
    _is_negative: bool,
    flags: &FormatFlags,
    width: usize,
) -> String {
    let (sign_prefix, abs_str) = if num_str.starts_with('-') {
        ("-", &num_str[1..])
    } else if flags.plus_sign {
        ("+", num_str)
    } else if flags.space_sign {
        (" ", num_str)
    } else {
        ("", num_str)
    };

    let content = format!("{}{}", sign_prefix, abs_str);

    if width > 0 && content.len() < width {
        let pad_len = width - content.len();
        if flags.left_align {
            format!("{}{}", content, " ".repeat(pad_len))
        } else if flags.zero_pad {
            format!("{}{}{}", sign_prefix, "0".repeat(pad_len), abs_str)
        } else {
            format!("{}{}", " ".repeat(pad_len), content)
        }
    } else {
        content
    }
}

/// Format in scientific notation matching C's %e.
fn format_scientific(value: f64, prec: usize, e_char: char) -> String {
    if value == 0.0 {
        let sign = if value.is_sign_negative() { "-" } else { "" };
        if prec == 0 {
            return format!("{sign}0{e_char}+00");
        }
        return format!("{sign}0.{:0>prec$}{e_char}+00", "", prec = prec);
    }

    let abs = value.abs();
    let sign = if value < 0.0 { "-" } else { "" };
    let exp = abs.log10().floor() as i32;
    let mantissa = abs / 10f64.powi(exp);

    let factor = 10f64.powi(prec as i32);
    let mantissa = (mantissa * factor).round() / factor;

    let (mantissa, exp) = if mantissa >= 10.0 {
        (mantissa / 10.0, exp + 1)
    } else {
        (mantissa, exp)
    };

    let exp_sign = if exp >= 0 { '+' } else { '-' };
    let exp_abs = exp.unsigned_abs();

    if prec == 0 {
        format!("{sign}{mantissa:.0}{e_char}{exp_sign}{exp_abs:02}")
    } else {
        format!(
            "{sign}{mantissa:.prec$}{e_char}{exp_sign}{exp_abs:02}",
            prec = prec
        )
    }
}

/// Format using %g - shortest representation.
fn format_g(value: f64, prec: usize, upper: bool) -> String {
    let prec = if prec == 0 { 1 } else { prec };

    if value == 0.0 {
        let sign = if value.is_sign_negative() { "-" } else { "" };
        return format!("{sign}0");
    }

    let abs = value.abs();
    let exp = abs.log10().floor() as i32;
    let e_char = if upper { 'E' } else { 'e' };

    if exp < -4 || exp >= prec as i32 {
        let sig_prec = prec.saturating_sub(1);
        let s = format_scientific(value, sig_prec, e_char);
        trim_g_trailing_zeros(&s)
    } else {
        let decimal_prec = if prec as i32 > exp + 1 {
            (prec as i32 - exp - 1) as usize
        } else {
            0
        };
        let s = format!("{value:.decimal_prec$}");
        trim_g_trailing_zeros(&s)
    }
}

/// Shell-quote a string for %q format specifier (GNU printf compat).
/// Uses backslash escaping for special characters, or $'...' for control chars.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }

    // Check if the string needs quoting at all.
    let needs_quoting = s.bytes().any(|b| {
        !b.is_ascii_alphanumeric()
            && b != b'_'
            && b != b'/'
            && b != b'.'
            && b != b'-'
            && b != b':'
            && b != b','
            && b != b'+'
            && b != b'@'
            && b != b'%'
    });

    if !needs_quoting {
        return s.to_string();
    }

    // Check if we have control characters that need $'...' quoting.
    let has_control = s.bytes().any(|b| b < 0x20 || b == 0x7f || b >= 0x80);

    if has_control {
        let mut result = String::from("$'");
        for byte in s.bytes() {
            match byte {
                b'\'' => result.push_str("\\'"),
                b'\\' => result.push_str("\\\\"),
                b'\n' => result.push_str("\\n"),
                b'\t' => result.push_str("\\t"),
                b'\r' => result.push_str("\\r"),
                0x07 => result.push_str("\\a"),
                0x08 => result.push_str("\\b"),
                0x0c => result.push_str("\\f"),
                0x0b => result.push_str("\\v"),
                0x1b => result.push_str("\\E"),
                b if b < 0x20 || b == 0x7f => {
                    result.push_str(&format!("\\{:03o}", b));
                }
                b if b >= 0x80 => {
                    result.push_str(&format!("\\{:03o}", b));
                }
                _ => result.push(byte as char),
            }
        }
        result.push('\'');
        result
    } else {
        // Use backslash escaping for simple special chars.
        let mut result = String::with_capacity(s.len() * 2);
        for ch in s.chars() {
            match ch {
                ' ' | '\'' | '"' | '\\' | '$' | '`' | '!' | '(' | ')' | '[' | ']' | '{' | '}'
                | '<' | '>' | '|' | '&' | ';' | '#' | '~' | '?' | '*' | '=' | '^' => {
                    result.push('\\');
                    result.push(ch);
                }
                _ => result.push(ch),
            }
        }
        result
    }
}

/// Trim trailing zeros from %g formatted output.
/// Only trims after a decimal point to avoid turning "100000" into "1".
fn trim_g_trailing_zeros(s: &str) -> String {
    if let Some(e_pos) = s.find(['e', 'E']) {
        let (mantissa, exponent) = s.split_at(e_pos);
        if mantissa.contains('.') {
            let trimmed = mantissa.trim_end_matches('0').trim_end_matches('.');
            format!("{trimmed}{exponent}")
        } else {
            s.to_string()
        }
    } else if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}
