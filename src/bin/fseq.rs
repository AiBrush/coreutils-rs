// fseq -- print a sequence of numbers
//
// Usage: seq [OPTION]... LAST
//        seq [OPTION]... FIRST LAST
//        seq [OPTION]... FIRST INCREMENT LAST

use std::io::{BufWriter, Write};
use std::process;

const TOOL_NAME: &str = "seq";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("Usage: {} [OPTION]... LAST", TOOL_NAME);
    println!("  or:  {} [OPTION]... FIRST LAST", TOOL_NAME);
    println!("  or:  {} [OPTION]... FIRST INCREMENT LAST", TOOL_NAME);
    println!("Print numbers from FIRST to LAST, in steps of INCREMENT.");
    println!();
    println!("Mandatory arguments to long options are mandatory for short options too.");
    println!("  -f, --format=FORMAT      use printf style floating-point FORMAT");
    println!("  -s, --separator=STRING   use STRING to separate numbers (default: \\n)");
    println!("  -w, --equal-width        equalize width by padding with leading zeroes");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
    println!();
    println!("If FIRST or INCREMENT is omitted, it defaults to 1.");
    println!("An omitted INCREMENT defaults to 1 even when LAST is smaller than FIRST.");
    println!("The sequence of numbers ends when the sum of the current number and");
    println!("INCREMENT would become greater than LAST.");
    println!("FIRST, INCREMENT, and LAST are interpreted as floating point values.");
    println!("FORMAT must be suitable for printing one argument of type 'double';");
    println!("it defaults to %.PRECf if FIRST, INCREMENT, and LAST are all fixed point");
    println!("decimal numbers with maximum precision PREC, and to %g otherwise.");
}

fn print_version() {
    println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
}

/// Count the number of decimal places in a number string.
fn decimal_places(s: &str) -> usize {
    // Strip leading minus
    let s = s.strip_prefix('-').unwrap_or(s);
    if let Some(pos) = s.find('.') {
        let frac = &s[pos + 1..];
        // Trim trailing zeros for precision determination
        let trimmed = frac.trim_end_matches('0');
        if trimmed.is_empty() { 0 } else { trimmed.len() }
    } else {
        0
    }
}

/// Count total width needed for equal-width formatting.
fn number_width(s: &str) -> usize {
    s.len()
}

/// Determine if a string represents a pure integer.
fn is_integer_str(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    if s.is_empty() {
        return false;
    }
    s.chars().all(|c| c.is_ascii_digit())
}

/// Format a number according to printf-style format string.
/// Supports %e, %f, %g with optional width and precision.
fn format_number(fmt: &str, value: f64) -> String {
    // Parse the format string: %[flags][width][.precision]type
    let bytes = fmt.as_bytes();
    let mut i = 0;

    // Find the % sign
    while i < bytes.len() && bytes[i] != b'%' {
        i += 1;
    }
    let prefix = &fmt[..i];
    if i >= bytes.len() {
        return fmt.to_string();
    }
    i += 1; // skip %

    // Handle %%
    if i < bytes.len() && bytes[i] == b'%' {
        return format!("{prefix}%{}", format_number(&fmt[i + 1..], value));
    }

    // Parse flags
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
            b'#' => {} // we ignore # for now
            _ => break,
        }
        i += 1;
    }

    // Parse width
    let mut width: usize = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        width = width
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as usize);
        i += 1;
    }

    // Parse precision
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

    // Parse type
    if i >= bytes.len() {
        return fmt.to_string();
    }
    let conv_type = bytes[i] as char;
    i += 1;
    let suffix = &fmt[i..];

    let formatted = match conv_type {
        'f' => {
            let prec = precision.unwrap_or(6);
            format!("{:.prec$}", value, prec = prec)
        }
        'e' => {
            let prec = precision.unwrap_or(6);
            format_scientific(value, prec, 'e')
        }
        'E' => {
            let prec = precision.unwrap_or(6);
            format_scientific(value, prec, 'E')
        }
        'g' => {
            let prec = precision.unwrap_or(6);
            format_g(value, prec, false)
        }
        'G' => {
            let prec = precision.unwrap_or(6);
            format_g(value, prec, true)
        }
        _ => {
            // Unknown format, just print the number
            format!("{}", value)
        }
    };

    // Apply width and padding
    let sign_prefix = if value < 0.0 {
        // The number already contains the minus sign
        ""
    } else if plus_sign {
        "+"
    } else if space_sign {
        " "
    } else {
        ""
    };

    // We need to handle the sign separately for zero-padding
    let num_str = if !sign_prefix.is_empty() && !formatted.starts_with('-') {
        format!("{sign_prefix}{formatted}")
    } else {
        formatted
    };

    let padded = if width > 0 && num_str.len() < width {
        let pad_len = width - num_str.len();
        if left_align {
            format!("{num_str}{}", " ".repeat(pad_len))
        } else if zero_pad && !left_align {
            // Zero-pad: put zeros after sign
            if num_str.starts_with('-') || num_str.starts_with('+') || num_str.starts_with(' ') {
                let (sign, rest) = num_str.split_at(1);
                format!("{sign}{}{rest}", "0".repeat(pad_len))
            } else {
                format!("{}{num_str}", "0".repeat(pad_len))
            }
        } else {
            format!("{}{num_str}", " ".repeat(pad_len))
        }
    } else {
        num_str
    };

    format!("{prefix}{padded}{suffix}")
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

    // Round the mantissa to the specified precision
    let factor = 10f64.powi(prec as i32);
    let mantissa = (mantissa * factor).round() / factor;

    // Handle mantissa rounding up to 10
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
        // Use scientific notation
        let sig_prec = prec.saturating_sub(1);
        let s = format_scientific(value, sig_prec, e_char);
        trim_g_trailing_zeros(&s)
    } else {
        // Use fixed notation
        let decimal_prec = if prec as i32 > exp + 1 {
            (prec as i32 - exp - 1) as usize
        } else {
            0
        };
        let s = format!("{value:.decimal_prec$}");
        trim_g_trailing_zeros(&s)
    }
}

/// Trim trailing zeros from %g formatted output (but not from the exponent).
fn trim_g_trailing_zeros(s: &str) -> String {
    // Split at 'e' or 'E' if present
    if let Some(e_pos) = s.find(['e', 'E']) {
        let (mantissa, exponent) = s.split_at(e_pos);
        let trimmed = mantissa.trim_end_matches('0').trim_end_matches('.');
        format!("{trimmed}{exponent}")
    } else {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // Parse options
    let mut format: Option<String> = None;
    let mut separator = "\n".to_string();
    let mut equal_width = false;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    let mut saw_dashdash = false;
    while i < args.len() {
        let arg = &args[i];
        if saw_dashdash {
            positional.push(arg.clone());
            i += 1;
            continue;
        }
        match arg.as_str() {
            "--" => {
                saw_dashdash = true;
            }
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                print_version();
                return;
            }
            "-w" | "--equal-width" => {
                equal_width = true;
            }
            "-f" | "--format" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'f'", TOOL_NAME);
                    process::exit(1);
                }
                format = Some(args[i].clone());
            }
            "-s" | "--separator" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 's'", TOOL_NAME);
                    process::exit(1);
                }
                separator = args[i].clone();
            }
            _ => {
                if let Some(rest) = arg.strip_prefix("--format=") {
                    format = Some(rest.to_string());
                } else if let Some(rest) = arg.strip_prefix("--separator=") {
                    separator = rest.to_string();
                } else if let Some(rest) = arg.strip_prefix("-f") {
                    format = Some(rest.to_string());
                } else if let Some(rest) = arg.strip_prefix("-s") {
                    separator = rest.to_string();
                } else {
                    // Could be a negative number or positional arg
                    positional.push(arg.clone());
                }
            }
        }
        i += 1;
    }

    if positional.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        process::exit(1);
    }

    let (first_str, increment_str, last_str) = match positional.len() {
        1 => ("1".to_string(), "1".to_string(), positional[0].clone()),
        2 => (
            positional[0].clone(),
            "1".to_string(),
            positional[1].clone(),
        ),
        3 => (
            positional[0].clone(),
            positional[1].clone(),
            positional[2].clone(),
        ),
        _ => {
            eprintln!("{}: extra operand '{}'", TOOL_NAME, positional[3]);
            process::exit(1);
        }
    };

    let first: f64 = match first_str.parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "{}: invalid floating point argument: '{}'",
                TOOL_NAME, first_str
            );
            process::exit(1);
        }
    };
    let increment: f64 = match increment_str.parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "{}: invalid floating point argument: '{}'",
                TOOL_NAME, increment_str
            );
            process::exit(1);
        }
    };
    let last: f64 = match last_str.parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "{}: invalid floating point argument: '{}'",
                TOOL_NAME, last_str
            );
            process::exit(1);
        }
    };

    if increment == 0.0 {
        eprintln!(
            "{}: invalid Zero increment value: '{}'",
            TOOL_NAME, increment_str
        );
        process::exit(1);
    }

    // Determine precision from input
    let prec = decimal_places(&first_str)
        .max(decimal_places(&increment_str))
        .max(decimal_places(&last_str));

    // Determine if we should use integer mode
    let use_int = prec == 0
        && is_integer_str(&first_str)
        && is_integer_str(&increment_str)
        && is_integer_str(&last_str)
        && format.is_none();

    // Determine format string
    let fmt = if let Some(ref f) = format {
        if equal_width {
            eprintln!(
                "{}: format string may not be specified when printing equal width strings",
                TOOL_NAME
            );
            process::exit(1);
        }
        f.clone()
    } else if equal_width {
        // Determine the width needed
        let first_width = if use_int {
            format_int_value(first as i64).len()
        } else {
            format_fixed(first, prec).len()
        };
        let last_width = if use_int {
            format_int_value(last as i64).len()
        } else {
            format_fixed(last, prec).len()
        };
        let w = first_width.max(last_width);
        if use_int {
            format!("%0{w}g")
        } else {
            format!("%0{w}.{prec}f")
        }
    } else if prec > 0 {
        format!("%.{prec}f")
    } else {
        String::new() // Will use integer or default formatting
    };

    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let mut is_first = true;

    if use_int && fmt.is_empty() {
        // Fast integer path
        let first_i = first as i64;
        let inc_i = increment as i64;
        let last_i = last as i64;

        let mut current = first_i;
        if inc_i > 0 {
            while current <= last_i {
                if !is_first {
                    let _ = out.write_all(separator.as_bytes());
                }
                is_first = false;
                let _ = write!(out, "{current}");
                current += inc_i;
            }
        } else {
            while current >= last_i {
                if !is_first {
                    let _ = out.write_all(separator.as_bytes());
                }
                is_first = false;
                let _ = write!(out, "{current}");
                current += inc_i;
            }
        }
    } else if use_int && !fmt.is_empty() {
        // Integer values with format string (e.g., equal-width)
        let first_i = first as i64;
        let inc_i = increment as i64;
        let last_i = last as i64;

        let mut current = first_i;
        if inc_i > 0 {
            while current <= last_i {
                if !is_first {
                    let _ = out.write_all(separator.as_bytes());
                }
                is_first = false;
                let s = format_number(&fmt, current as f64);
                let _ = out.write_all(s.as_bytes());
                current += inc_i;
            }
        } else {
            while current >= last_i {
                if !is_first {
                    let _ = out.write_all(separator.as_bytes());
                }
                is_first = false;
                let s = format_number(&fmt, current as f64);
                let _ = out.write_all(s.as_bytes());
                current += inc_i;
            }
        }
    } else {
        // Float path
        // Use a step counter to avoid accumulation errors
        let mut step: u64 = 0;
        if increment > 0.0 {
            loop {
                let val = first + step as f64 * increment;
                if val > last {
                    break;
                }
                if !is_first {
                    let _ = out.write_all(separator.as_bytes());
                }
                is_first = false;
                if fmt.is_empty() {
                    let s = format_fixed(val, prec);
                    let _ = out.write_all(s.as_bytes());
                } else {
                    let s = format_number(&fmt, val);
                    let _ = out.write_all(s.as_bytes());
                }
                step += 1;
            }
        } else {
            loop {
                let val = first + step as f64 * increment;
                if val < last {
                    break;
                }
                if !is_first {
                    let _ = out.write_all(separator.as_bytes());
                }
                is_first = false;
                if fmt.is_empty() {
                    let s = format_fixed(val, prec);
                    let _ = out.write_all(s.as_bytes());
                } else {
                    let s = format_number(&fmt, val);
                    let _ = out.write_all(s.as_bytes());
                }
                step += 1;
            }
        }
    }

    if !is_first {
        let _ = out.write_all(b"\n");
    }
    let _ = out.flush();
}

fn format_int_value(v: i64) -> String {
    format!("{v}")
}

fn format_fixed(value: f64, prec: usize) -> String {
    if prec == 0 {
        format!("{}", value as i64)
    } else {
        format!("{value:.prec$}", prec = prec)
    }
}

/// Return the width needed for equal-width display of a number string.
#[allow(dead_code)]
fn display_width(s: &str) -> usize {
    number_width(s)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fseq");
        Command::new(path)
    }

    #[test]
    fn test_basic_1_to_10() {
        let output = cmd().arg("10").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let expected = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
        assert_eq!(stdout, expected);
    }

    #[test]
    fn test_first_and_last() {
        let output = cmd().args(["3", "7"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "3\n4\n5\n6\n7\n");
    }

    #[test]
    fn test_first_increment_last() {
        let output = cmd().args(["1", "2", "10"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "1\n3\n5\n7\n9\n");
    }

    #[test]
    fn test_format_f() {
        let output = cmd().args(["-f", "%03g", "5"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "001\n002\n003\n004\n005\n");
    }

    #[test]
    fn test_separator() {
        let output = cmd().args(["-s", ", ", "5"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "1, 2, 3, 4, 5\n");
    }

    #[test]
    fn test_equal_width() {
        let output = cmd().args(["-w", "1", "10"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "01\n02\n03\n04\n05\n06\n07\n08\n09\n10\n");
    }

    #[test]
    fn test_negative_numbers() {
        let output = cmd().args(["-3", "3"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "-3\n-2\n-1\n0\n1\n2\n3\n");
    }

    #[test]
    fn test_floating_point() {
        let output = cmd().args(["0.5", "0.5", "2.5"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "0.5\n1.0\n1.5\n2.0\n2.5\n");
    }

    #[test]
    fn test_countdown() {
        let output = cmd().args(["5", "-1", "1"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "5\n4\n3\n2\n1\n");
    }

    #[test]
    fn test_single_number() {
        let output = cmd().arg("1").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "1\n");
    }

    #[test]
    fn test_large_range() {
        let output = cmd().arg("10000").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 10000);
        assert_eq!(lines[0], "1");
        assert_eq!(lines[9999], "10000");
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("seq"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("seq"));
        assert!(stdout.contains("fcoreutils"));
    }

    /// Check if system seq is GNU seq (BSD seq on macOS behaves differently)
    fn is_gnu_seq() -> bool {
        Command::new("seq")
            .arg("--version")
            .output()
            .map(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                stdout.contains("GNU") || stderr.contains("GNU")
            })
            .unwrap_or(false)
    }

    #[test]
    fn test_match_gnu_basic() {
        if !is_gnu_seq() {
            return;
        }
        let gnu = Command::new("seq").arg("10").output().unwrap();
        let ours = cmd().arg("10").output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&ours.stdout),
            String::from_utf8_lossy(&gnu.stdout),
            "Output mismatch with GNU seq for 'seq 10'"
        );
    }

    #[test]
    fn test_match_gnu_first_last() {
        if !is_gnu_seq() {
            return;
        }
        let gnu = Command::new("seq").args(["5", "15"]).output().unwrap();
        let ours = cmd().args(["5", "15"]).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&ours.stdout),
            String::from_utf8_lossy(&gnu.stdout),
            "Output mismatch with GNU seq for 'seq 5 15'"
        );
    }

    #[test]
    fn test_match_gnu_increment() {
        if !is_gnu_seq() {
            return;
        }
        let gnu = Command::new("seq").args(["1", "3", "20"]).output().unwrap();
        let ours = cmd().args(["1", "3", "20"]).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&ours.stdout),
            String::from_utf8_lossy(&gnu.stdout),
            "Output mismatch with GNU seq for 'seq 1 3 20'"
        );
    }

    #[test]
    fn test_match_gnu_separator() {
        if !is_gnu_seq() {
            return;
        }
        let gnu = Command::new("seq").args(["-s", ":", "5"]).output().unwrap();
        let ours = cmd().args(["-s", ":", "5"]).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&ours.stdout),
            String::from_utf8_lossy(&gnu.stdout),
            "Output mismatch with GNU seq for 'seq -s : 5'"
        );
    }

    #[test]
    fn test_match_gnu_equal_width() {
        if !is_gnu_seq() {
            return;
        }
        let gnu = Command::new("seq")
            .args(["-w", "1", "100"])
            .output()
            .unwrap();
        let ours = cmd().args(["-w", "1", "100"]).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&ours.stdout),
            String::from_utf8_lossy(&gnu.stdout),
            "Output mismatch with GNU seq for 'seq -w 1 100'"
        );
    }

    #[test]
    fn test_zero_increment() {
        let output = cmd().args(["1", "0", "5"]).output().unwrap();
        assert!(!output.status.success());
    }

    #[test]
    fn test_empty_sequence() {
        // When first > last with positive increment, output nothing
        let output = cmd().args(["5", "1"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "");
    }

    #[test]
    fn test_equal_width_negative() {
        let output = cmd().args(["-w", "-5", "5"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Should zero-pad to match widths
        assert!(stdout.contains("-5\n"));
        assert!(stdout.contains("05\n") || stdout.contains("5\n"));
    }
}
