use std::io::Write;

/// Unit scale for input/output conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleUnit {
    /// No scaling.
    None,
    /// SI: K=1000, M=10^6, G=10^9, T=10^12, P=10^15, E=10^18, Z=10^21, Y=10^24.
    Si,
    /// IEC: K=1024, M=1048576, G=2^30, T=2^40, P=2^50, E=2^60.
    Iec,
    /// IEC with 'i' suffix: Ki=1024, Mi=1048576, etc.
    IecI,
    /// Auto-detect from suffix (for --from=auto).
    Auto,
}

/// Rounding method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundMethod {
    /// Round up (toward +infinity).
    Up,
    /// Round down (toward -infinity).
    Down,
    /// Round away from zero.
    FromZero,
    /// Round toward zero.
    TowardsZero,
    /// Round to nearest, half away from zero (default).
    Nearest,
}

/// How to handle invalid input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidMode {
    /// Print error and exit immediately.
    Abort,
    /// Print error but continue processing.
    Fail,
    /// Print warning but continue processing.
    Warn,
    /// Silently ignore invalid input.
    Ignore,
}

/// Configuration for the numfmt command.
pub struct NumfmtConfig {
    pub from: ScaleUnit,
    pub to: ScaleUnit,
    pub from_unit: f64,
    pub to_unit: f64,
    pub padding: Option<i32>,
    pub round: RoundMethod,
    pub suffix: Option<String>,
    pub format: Option<String>,
    pub field: Vec<usize>,
    pub delimiter: Option<char>,
    pub header: usize,
    pub invalid: InvalidMode,
    pub grouping: bool,
    pub zero_terminated: bool,
}

impl Default for NumfmtConfig {
    fn default() -> Self {
        Self {
            from: ScaleUnit::None,
            to: ScaleUnit::None,
            from_unit: 1.0,
            to_unit: 1.0,
            padding: None,
            round: RoundMethod::Nearest,
            suffix: None,
            format: None,
            field: vec![1],
            delimiter: None,
            header: 0,
            invalid: InvalidMode::Abort,
            grouping: false,
            zero_terminated: false,
        }
    }
}

/// SI suffix table: suffix char -> multiplier.
const SI_SUFFIXES: &[(char, f64)] = &[
    ('K', 1e3),
    ('M', 1e6),
    ('G', 1e9),
    ('T', 1e12),
    ('P', 1e15),
    ('E', 1e18),
    ('Z', 1e21),
    ('Y', 1e24),
];

/// IEC suffix table: suffix char -> multiplier (powers of 1024).
const IEC_SUFFIXES: &[(char, f64)] = &[
    ('K', 1024.0),
    ('M', 1_048_576.0),
    ('G', 1_073_741_824.0),
    ('T', 1_099_511_627_776.0),
    ('P', 1_125_899_906_842_624.0),
    ('E', 1_152_921_504_606_846_976.0),
    ('Z', 1_180_591_620_717_411_303_424.0),
    ('Y', 1_208_925_819_614_629_174_706_176.0),
];

/// Parse a scale unit string.
pub fn parse_scale_unit(s: &str) -> Result<ScaleUnit, String> {
    match s {
        "none" => Ok(ScaleUnit::None),
        "si" => Ok(ScaleUnit::Si),
        "iec" => Ok(ScaleUnit::Iec),
        "iec-i" => Ok(ScaleUnit::IecI),
        "auto" => Ok(ScaleUnit::Auto),
        _ => Err(format!("invalid unit: '{}'", s)),
    }
}

/// Parse a round method string.
pub fn parse_round_method(s: &str) -> Result<RoundMethod, String> {
    match s {
        "up" => Ok(RoundMethod::Up),
        "down" => Ok(RoundMethod::Down),
        "from-zero" => Ok(RoundMethod::FromZero),
        "towards-zero" => Ok(RoundMethod::TowardsZero),
        "nearest" => Ok(RoundMethod::Nearest),
        _ => Err(format!("invalid rounding method: '{}'", s)),
    }
}

/// Parse an invalid mode string.
pub fn parse_invalid_mode(s: &str) -> Result<InvalidMode, String> {
    match s {
        "abort" => Ok(InvalidMode::Abort),
        "fail" => Ok(InvalidMode::Fail),
        "warn" => Ok(InvalidMode::Warn),
        "ignore" => Ok(InvalidMode::Ignore),
        _ => Err(format!("invalid mode: '{}'", s)),
    }
}

/// Parse a field specification string like "1", "1,3", "1-5", or "-".
/// Returns 1-based field indices.
pub fn parse_fields(s: &str) -> Result<Vec<usize>, String> {
    if s == "-" {
        // All fields - we represent this as an empty vec and handle it specially.
        return Ok(vec![]);
    }
    let mut fields = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if let Some(dash_pos) = part.find('-') {
            let start_str = &part[..dash_pos];
            let end_str = &part[dash_pos + 1..];
            // Handle open ranges like "-5" or "3-"
            if start_str.is_empty() && end_str.is_empty() {
                return Ok(vec![]);
            }
            let start: usize = if start_str.is_empty() {
                1
            } else {
                start_str
                    .parse()
                    .map_err(|_| format!("invalid field value '{}'", part))?
            };
            let end: usize = if end_str.is_empty() {
                // Open-ended range: we use 0 as sentinel for "all remaining"
                // For simplicity, return a large upper bound.
                9999
            } else {
                end_str
                    .parse()
                    .map_err(|_| format!("invalid field value '{}'", part))?
            };
            if start == 0 {
                return Err(format!("fields are numbered from 1: '{}'", part));
            }
            for i in start..=end {
                if !fields.contains(&i) {
                    fields.push(i);
                }
            }
        } else {
            let n: usize = part
                .parse()
                .map_err(|_| format!("invalid field value '{}'", part))?;
            if n == 0 {
                return Err("fields are numbered from 1".to_string());
            }
            if !fields.contains(&n) {
                fields.push(n);
            }
        }
    }
    fields.sort();
    Ok(fields)
}

/// Parse a number with optional suffix, returning the raw numeric value.
/// Handles suffixes like K, M, G, T, P, E, Z, Y (and Ki, Mi, etc. for iec-i).
fn parse_number_with_suffix(s: &str, unit: ScaleUnit) -> Result<f64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("invalid number: ''".to_string());
    }

    // Find where the numeric part ends and the suffix begins.
    let mut num_end = s.len();
    let bytes = s.as_bytes();
    let len = s.len();

    // Check for trailing scale suffix characters.
    if len > 0 {
        let last_char = bytes[len - 1] as char;

        match unit {
            ScaleUnit::Auto | ScaleUnit::IecI => {
                // Check for 'i' suffix (e.g., Ki, Mi).
                if last_char == 'i' && len >= 2 {
                    let prefix_char = (bytes[len - 2] as char).to_ascii_uppercase();
                    if is_scale_suffix(prefix_char) {
                        num_end = len - 2;
                    }
                } else {
                    let upper = last_char.to_ascii_uppercase();
                    if is_scale_suffix(upper) {
                        num_end = len - 1;
                    }
                }
            }
            ScaleUnit::Si | ScaleUnit::Iec => {
                let upper = last_char.to_ascii_uppercase();
                if is_scale_suffix(upper) {
                    num_end = len - 1;
                }
            }
            ScaleUnit::None => {}
        }
    }

    let num_str = &s[..num_end];
    let suffix_str = &s[num_end..];

    // Parse the numeric part.
    let value: f64 = num_str
        .parse()
        .map_err(|_| format!("invalid number: '{}'", s))?;

    // Apply suffix multiplier.
    let multiplier = if suffix_str.is_empty() {
        1.0
    } else {
        let suffix_upper = suffix_str.chars().next().unwrap().to_ascii_uppercase();
        match unit {
            ScaleUnit::Auto => {
                // Auto-detect: if suffix ends with 'i', use IEC; otherwise SI.
                if suffix_str.len() >= 2 && suffix_str.ends_with('i') {
                    find_iec_multiplier(suffix_upper)?
                } else {
                    find_si_multiplier(suffix_upper)?
                }
            }
            ScaleUnit::Si => find_si_multiplier(suffix_upper)?,
            ScaleUnit::Iec | ScaleUnit::IecI => find_iec_multiplier(suffix_upper)?,
            ScaleUnit::None => {
                return Err(format!("invalid number: '{}'", s));
            }
        }
    };

    Ok(value * multiplier)
}

fn is_scale_suffix(c: char) -> bool {
    matches!(c, 'K' | 'M' | 'G' | 'T' | 'P' | 'E' | 'Z' | 'Y')
}

fn find_si_multiplier(c: char) -> Result<f64, String> {
    for &(suffix, mult) in SI_SUFFIXES {
        if suffix == c {
            return Ok(mult);
        }
    }
    Err(format!("invalid suffix: '{}'", c))
}

fn find_iec_multiplier(c: char) -> Result<f64, String> {
    for &(suffix, mult) in IEC_SUFFIXES {
        if suffix == c {
            return Ok(mult);
        }
    }
    Err(format!("invalid suffix: '{}'", c))
}

/// Apply rounding according to the specified method.
fn apply_round(value: f64, method: RoundMethod) -> f64 {
    match method {
        RoundMethod::Up => value.ceil(),
        RoundMethod::Down => value.floor(),
        RoundMethod::FromZero => {
            if value >= 0.0 {
                value.ceil()
            } else {
                value.floor()
            }
        }
        RoundMethod::TowardsZero => {
            if value >= 0.0 {
                value.floor()
            } else {
                value.ceil()
            }
        }
        RoundMethod::Nearest => value.round(),
    }
}

/// Format a number with scale suffix for output.
fn format_scaled(value: f64, unit: ScaleUnit, round: RoundMethod) -> String {
    match unit {
        ScaleUnit::None => {
            // Output as plain number.
            format_plain_number(value)
        }
        ScaleUnit::Si => format_with_scale(value, SI_SUFFIXES, "", round),
        ScaleUnit::Iec => format_with_scale(value, IEC_SUFFIXES, "", round),
        ScaleUnit::IecI => format_with_scale(value, IEC_SUFFIXES, "i", round),
        ScaleUnit::Auto => {
            // For --to=auto, behave like SI.
            format_with_scale(value, SI_SUFFIXES, "", round)
        }
    }
}

/// Format a plain number, removing unnecessary trailing zeros and decimal point.
fn format_plain_number(value: f64) -> String {
    let int_val = value as i64;
    if value == (int_val as f64) {
        format!("{}", int_val)
    } else {
        // Use enough precision to avoid loss.
        format!("{:.1}", value)
    }
}

/// Format a number with appropriate scale suffix.
fn format_with_scale(
    value: f64,
    suffixes: &[(char, f64)],
    i_suffix: &str,
    round: RoundMethod,
) -> String {
    let abs_value = value.abs();
    let sign = if value < 0.0 { "-" } else { "" };

    // Find the largest suffix that applies.
    let mut chosen_suffix = None;
    let mut chosen_mult = 1.0;

    for &(suffix, mult) in suffixes.iter().rev() {
        if abs_value >= mult {
            chosen_suffix = Some(suffix);
            chosen_mult = mult;
            break;
        }
    }

    if let Some(suffix) = chosen_suffix {
        let scaled = value / chosen_mult;
        let scaled = apply_round_for_display(scaled, round);

        format!("{sign}{:.1}{}{}", scaled.abs(), suffix, i_suffix)
    } else {
        // Value is smaller than the smallest suffix, output as-is.
        format_plain_number(value)
    }
}

/// Apply rounding for display purposes (when formatting scaled output).
fn apply_round_for_display(value: f64, method: RoundMethod) -> f64 {
    // For display, we round to 1 decimal place.
    let factor = 10.0;
    let shifted = value * factor;
    let rounded = match method {
        RoundMethod::Up => shifted.ceil(),
        RoundMethod::Down => shifted.floor(),
        RoundMethod::FromZero => {
            if shifted >= 0.0 {
                shifted.ceil()
            } else {
                shifted.floor()
            }
        }
        RoundMethod::TowardsZero => {
            if shifted >= 0.0 {
                shifted.floor()
            } else {
                shifted.ceil()
            }
        }
        RoundMethod::Nearest => shifted.round(),
    };
    rounded / factor
}

/// Insert thousands grouping separators.
fn group_thousands(s: &str) -> String {
    // Find the integer part (before any decimal point).
    let (integer_part, rest) = if let Some(dot_pos) = s.find('.') {
        (&s[..dot_pos], &s[dot_pos..])
    } else {
        (s, "")
    };

    // Handle sign.
    let (sign, digits) = if integer_part.starts_with('-') {
        ("-", &integer_part[1..])
    } else {
        ("", integer_part)
    };

    if digits.len() <= 3 {
        return format!("{}{}{}", sign, digits, rest);
    }

    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    let remainder = digits.len() % 3;
    if remainder > 0 {
        result.push_str(&digits[..remainder]);
    }
    for (i, chunk) in digits.as_bytes()[remainder..].chunks(3).enumerate() {
        if i > 0 || remainder > 0 {
            result.push(',');
        }
        result.push_str(std::str::from_utf8(chunk).unwrap());
    }

    format!("{}{}{}", sign, result, rest)
}

/// Apply width/padding from a printf-style format string to an already-scaled string.
/// Used when both --to and --format are specified.
fn apply_format_padding(scaled: &str, fmt: &str) -> String {
    let bytes = fmt.as_bytes();
    let mut i = 0;

    // Find '%'.
    while i < bytes.len() && bytes[i] != b'%' {
        i += 1;
    }
    let prefix = &fmt[..i];
    if i >= bytes.len() {
        return format!("{}{}", prefix, scaled);
    }
    i += 1; // skip '%'

    // Parse flags.
    let mut left_align = false;
    while i < bytes.len() {
        match bytes[i] {
            b'0' | b'+' | b' ' | b'#' | b'\'' => {}
            b'-' => left_align = true,
            _ => break,
        }
        i += 1;
    }

    // Parse width.
    let mut width: usize = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        width = width
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as usize);
        i += 1;
    }

    // Skip precision and conversion char.
    while i < bytes.len() && (bytes[i] == b'.' || bytes[i].is_ascii_digit()) {
        i += 1;
    }
    if i < bytes.len() {
        i += 1; // skip conversion char
    }
    let suffix = &fmt[i..];

    let padded = if width > 0 && scaled.len() < width {
        let pad_len = width - scaled.len();
        if left_align {
            format!("{}{}", scaled, " ".repeat(pad_len))
        } else {
            format!("{}{}", " ".repeat(pad_len), scaled)
        }
    } else {
        scaled.to_string()
    };

    format!("{}{}{}", prefix, padded, suffix)
}

/// Apply printf-style format to a number.
fn apply_format(value: f64, fmt: &str) -> Result<String, String> {
    // Parse format: %[flags][width][.precision]f
    let bytes = fmt.as_bytes();
    let mut i = 0;

    // Find '%'.
    while i < bytes.len() && bytes[i] != b'%' {
        i += 1;
    }
    let prefix = &fmt[..i];
    if i >= bytes.len() {
        return Err(format!("invalid format: '{}'", fmt));
    }
    i += 1; // skip '%'

    if i >= bytes.len() {
        return Err(format!("invalid format: '{}'", fmt));
    }

    // Handle %%
    if bytes[i] == b'%' {
        return Ok(format!("{}%", prefix));
    }

    // Parse flags.
    let mut zero_pad = false;
    let mut left_align = false;
    let mut plus_sign = false;
    let mut space_sign = false;
    while i < bytes.len() {
        match bytes[i] {
            b'0' => zero_pad = true,
            b'-' => left_align = true,
            b'+' => plus_sign = true,
            b' ' => space_sign = true,
            b'#' => {}
            b'\'' => {} // grouping flag, handled separately
            _ => break,
        }
        i += 1;
    }

    // Parse width.
    let mut width: usize = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        width = width
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as usize);
        i += 1;
    }

    // Parse precision.
    let mut precision: Option<usize> = None;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let mut prec: usize = 0;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            prec = prec
                .saturating_mul(10)
                .saturating_add((bytes[i] - b'0') as usize);
            i += 1;
        }
        precision = Some(prec);
    }

    // Parse conversion type.
    if i >= bytes.len() {
        return Err(format!("invalid format: '{}'", fmt));
    }
    let conv = bytes[i] as char;
    i += 1;
    let suffix = &fmt[i..];

    let prec = precision.unwrap_or(6);
    let formatted = match conv {
        'f' => format!("{:.prec$}", value, prec = prec),
        'e' => format_scientific(value, prec, 'e'),
        'E' => format_scientific(value, prec, 'E'),
        'g' => format_g(value, prec, false),
        'G' => format_g(value, prec, true),
        _ => return Err(format!("invalid format character: '{}'", conv)),
    };

    // Apply sign prefix.
    let sign_str = if value < 0.0 {
        ""
    } else if plus_sign {
        "+"
    } else if space_sign {
        " "
    } else {
        ""
    };

    let num_str = if !sign_str.is_empty() && !formatted.starts_with('-') {
        format!("{}{}", sign_str, formatted)
    } else {
        formatted
    };

    // Apply width and padding.
    let padded = if width > 0 && num_str.len() < width {
        let pad_len = width - num_str.len();
        if left_align {
            format!("{}{}", num_str, " ".repeat(pad_len))
        } else if zero_pad {
            if num_str.starts_with('-') || num_str.starts_with('+') || num_str.starts_with(' ') {
                let (sign, rest) = num_str.split_at(1);
                format!("{}{}{}", sign, "0".repeat(pad_len), rest)
            } else {
                format!("{}{}", "0".repeat(pad_len), num_str)
            }
        } else {
            format!("{}{}", " ".repeat(pad_len), num_str)
        }
    } else {
        num_str
    };

    Ok(format!("{}{}{}", prefix, padded, suffix))
}

/// Format in scientific notation.
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
        trim_g_zeros(&s)
    } else {
        let decimal_prec = if prec as i32 > exp + 1 {
            (prec as i32 - exp - 1) as usize
        } else {
            0
        };
        let s = format!("{value:.decimal_prec$}");
        trim_g_zeros(&s)
    }
}

fn trim_g_zeros(s: &str) -> String {
    if let Some(e_pos) = s.find(['e', 'E']) {
        let (mantissa, exponent) = s.split_at(e_pos);
        let trimmed = mantissa.trim_end_matches('0').trim_end_matches('.');
        format!("{trimmed}{exponent}")
    } else {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Convert a single numeric token according to the config.
fn convert_number(token: &str, config: &NumfmtConfig) -> Result<String, String> {
    // Parse the input number (with optional suffix).
    let raw_value = parse_number_with_suffix(token, config.from)?;

    // Apply from-unit scaling.
    let value = raw_value * config.from_unit;

    // Apply to-unit scaling.
    let value = value / config.to_unit;

    // Format the output.
    let mut result = if let Some(ref fmt) = config.format {
        // If --to is also specified, first scale, then apply format padding.
        if config.to != ScaleUnit::None {
            let scaled = format_scaled(value, config.to, config.round);
            // Extract width from the format string and apply padding.
            apply_format_padding(&scaled, fmt)
        } else {
            let rounded = apply_round(value, config.round);
            apply_format(rounded, fmt)?
        }
    } else if config.to != ScaleUnit::None {
        format_scaled(value, config.to, config.round)
    } else {
        let rounded = apply_round(value, config.round);
        format_plain_number(rounded)
    };

    // Apply grouping.
    if config.grouping {
        result = group_thousands(&result);
    }

    // Apply suffix.
    if let Some(ref suffix) = config.suffix {
        result.push_str(suffix);
    }

    // Apply padding.
    if let Some(pad) = config.padding {
        let pad_width = pad.unsigned_abs() as usize;
        if result.len() < pad_width {
            let deficit = pad_width - result.len();
            if pad < 0 {
                // Left-align (pad on right).
                result = format!("{}{}", result, " ".repeat(deficit));
            } else {
                // Right-align (pad on left).
                result = format!("{}{}", " ".repeat(deficit), result);
            }
        }
    }

    Ok(result)
}

/// Split a line into fields based on the delimiter.
fn split_fields<'a>(line: &'a str, delimiter: Option<char>) -> Vec<&'a str> {
    match delimiter {
        Some(delim) => line.split(delim).collect(),
        None => {
            // Whitespace splitting: split on runs of whitespace, but preserve
            // leading whitespace as empty fields.
            let mut fields = Vec::new();
            let mut chars = line.char_indices().peekable();
            let mut field_start = 0;
            let mut in_space = true;
            let mut first = true;

            while let Some(&(i, c)) = chars.peek() {
                if c.is_whitespace() {
                    if !in_space && !first {
                        fields.push(&line[field_start..i]);
                    }
                    in_space = true;
                    chars.next();
                } else {
                    if in_space {
                        field_start = i;
                        in_space = false;
                        first = false;
                    }
                    chars.next();
                }
            }
            if !in_space {
                fields.push(&line[field_start..]);
            }

            if fields.is_empty() {
                vec![line]
            } else {
                fields
            }
        }
    }
}

/// Reassemble fields into a line with proper spacing.
fn reassemble_fields(
    original: &str,
    fields: &[&str],
    converted: &[String],
    delimiter: Option<char>,
) -> String {
    match delimiter {
        Some(delim) => converted.join(&delim.to_string()),
        None => {
            // For whitespace-delimited input, reconstruct preserving original spacing.
            let mut result = String::with_capacity(original.len());
            let mut field_idx = 0;
            let mut in_space = true;
            let mut i = 0;
            let bytes = original.as_bytes();

            while i < bytes.len() {
                let c = bytes[i] as char;
                if c.is_ascii_whitespace() {
                    if !in_space && field_idx > 0 {
                        // We just finished a field.
                    }
                    result.push(c);
                    in_space = true;
                    i += 1;
                } else {
                    if in_space {
                        in_space = false;
                        // Output the converted field instead of the original.
                        if field_idx < converted.len() {
                            result.push_str(&converted[field_idx]);
                        } else if field_idx < fields.len() {
                            result.push_str(fields[field_idx]);
                        }
                        field_idx += 1;
                        // Skip past the original field characters.
                        while i < bytes.len() && !(bytes[i] as char).is_ascii_whitespace() {
                            i += 1;
                        }
                        continue;
                    }
                    i += 1;
                }
            }

            result
        }
    }
}

/// Process a single line according to the numfmt configuration.
pub fn process_line(line: &str, config: &NumfmtConfig) -> Result<String, String> {
    let fields = split_fields(line, config.delimiter);

    if fields.is_empty() {
        return Ok(line.to_string());
    }

    let all_fields = config.field.is_empty();

    let mut converted: Vec<String> = Vec::with_capacity(fields.len());
    for (i, field) in fields.iter().enumerate() {
        let field_num = i + 1; // 1-based
        let should_convert = all_fields || config.field.contains(&field_num);

        if should_convert {
            match convert_number(field, config) {
                Ok(s) => converted.push(s),
                Err(e) => match config.invalid {
                    InvalidMode::Abort => return Err(e),
                    InvalidMode::Fail => {
                        eprintln!("numfmt: {}", e);
                        converted.push(field.to_string());
                    }
                    InvalidMode::Warn => {
                        eprintln!("numfmt: {}", e);
                        converted.push(field.to_string());
                    }
                    InvalidMode::Ignore => {
                        converted.push(field.to_string());
                    }
                },
            }
        } else {
            converted.push(field.to_string());
        }
    }

    Ok(reassemble_fields(
        line,
        &fields,
        &converted,
        config.delimiter,
    ))
}

/// Run the numfmt command with the given configuration and input.
pub fn run_numfmt<R: std::io::BufRead, W: Write>(
    input: R,
    mut output: W,
    config: &NumfmtConfig,
) -> Result<(), String> {
    let terminator = if config.zero_terminated { b'\0' } else { b'\n' };
    let mut header_remaining = config.header;
    let mut buf = Vec::new();
    let mut reader = input;
    let mut had_error = false;

    loop {
        buf.clear();
        let bytes_read = reader
            .read_until(terminator, &mut buf)
            .map_err(|e| format!("read error: {}", e))?;
        if bytes_read == 0 {
            break;
        }

        // Remove the terminator for processing.
        let line = if buf.last() == Some(&terminator) {
            &buf[..buf.len() - 1]
        } else {
            &buf[..]
        };
        let line_str = String::from_utf8_lossy(line);

        if header_remaining > 0 {
            header_remaining -= 1;
            output
                .write_all(line_str.as_bytes())
                .map_err(|e| format!("write error: {}", e))?;
            output
                .write_all(&[terminator])
                .map_err(|e| format!("write error: {}", e))?;
            continue;
        }

        match process_line(&line_str, config) {
            Ok(result) => {
                output
                    .write_all(result.as_bytes())
                    .map_err(|e| format!("write error: {}", e))?;
                output
                    .write_all(&[terminator])
                    .map_err(|e| format!("write error: {}", e))?;
            }
            Err(e) => {
                match config.invalid {
                    InvalidMode::Abort => {
                        eprintln!("numfmt: {}", e);
                        return Err(e);
                    }
                    InvalidMode::Fail => {
                        eprintln!("numfmt: {}", e);
                        // Output original line.
                        output
                            .write_all(line_str.as_bytes())
                            .map_err(|e| format!("write error: {}", e))?;
                        output
                            .write_all(&[terminator])
                            .map_err(|e| format!("write error: {}", e))?;
                        had_error = true;
                    }
                    InvalidMode::Warn => {
                        eprintln!("numfmt: {}", e);
                        output
                            .write_all(line_str.as_bytes())
                            .map_err(|e| format!("write error: {}", e))?;
                        output
                            .write_all(&[terminator])
                            .map_err(|e| format!("write error: {}", e))?;
                    }
                    InvalidMode::Ignore => {
                        output
                            .write_all(line_str.as_bytes())
                            .map_err(|e| format!("write error: {}", e))?;
                        output
                            .write_all(&[terminator])
                            .map_err(|e| format!("write error: {}", e))?;
                    }
                }
            }
        }
    }

    output.flush().map_err(|e| format!("flush error: {}", e))?;

    if had_error {
        Err("conversion errors occurred".to_string())
    } else {
        Ok(())
    }
}
