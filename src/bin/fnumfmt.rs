// fnumfmt -- convert numbers to/from human-readable form
//
// Usage: numfmt [OPTION]... [NUMBER]...
//
// Converts numbers from/to human-readable strings.
// Numbers can be given on the command line or read from standard input.

use std::io::{self, BufWriter, Write};
use std::process;

use coreutils_rs::numfmt::{self, InvalidMode, NumfmtConfig};

const TOOL_NAME: &str = "numfmt";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("Usage: {} [OPTION]... [NUMBER]...", TOOL_NAME);
    println!("Reformat NUMBER(s), or the numbers from standard input if none are specified.");
    println!();
    println!("Mandatory arguments to long options are mandatory for short options too.");
    println!("  -d, --delimiter=X    use X instead of whitespace for field delimiter");
    println!("      --field=FIELDS   replace the numbers in these input fields (default=1);");
    println!("                         see FIELDS below");
    println!("      --format=FORMAT  use printf style floating-point FORMAT;");
    println!("                         see FORMAT below for details");
    println!("      --from=UNIT      auto-scale input numbers to UNITs; default is 'none';");
    println!("                         see UNIT below");
    println!("      --from-unit=N    specify the input unit size (instead of the default 1)");
    println!("      --grouping       use locale-defined grouping of digits, e.g. 1,000,000");
    println!("                         (which means it has no effect in the C/POSIX locale)");
    println!("      --header[=N]     print (without converting) the first N header lines;");
    println!("                         N defaults to 1 if not specified");
    println!("      --invalid=MODE   failure mode for invalid numbers: MODE can be:");
    println!("                         abort (default), fail, warn, ignore");
    println!("      --padding=N      pad the output to N characters; positive N will");
    println!("                         right-align; negative N will left-align;");
    println!("                         padding is ignored if the output is wider than N");
    println!("      --round=METHOD   use METHOD for rounding when scaling; METHOD can be:");
    println!("                         up, down, from-zero, towards-zero, nearest (default)");
    println!("      --suffix=SUFFIX  add SUFFIX to output numbers, and accept optional");
    println!("                         SUFFIX in input numbers");
    println!("      --to=UNIT        auto-scale output numbers to UNITs; see UNIT below");
    println!("      --to-unit=N      the output unit size (instead of the default 1)");
    println!("  -z, --zero-terminated  line delimiter is NUL, not newline");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
    println!();
    println!("UNIT options:");
    println!("  none       no auto-scaling is done; suffixes will trigger an error");
    println!("  auto       accept optional single/two letter suffix:");
    println!("               1K = 1000, 1Ki = 1024, 1M = 1000000, 1Mi = 1048576, ...");
    println!("  si         accept optional single letter suffix:");
    println!("               1K = 1000, 1M = 1000000, ...");
    println!("  iec        accept optional single letter suffix:");
    println!("               1K = 1024, 1M = 1048576, ...");
    println!("  iec-i      accept optional two-letter suffix:");
    println!("               1Ki = 1024, 1Mi = 1048576, ...");
}

fn print_version() {
    println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
}

fn parse_args() -> (NumfmtConfig, Vec<String>) {
    let mut config = NumfmtConfig::default();
    let mut positional: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" => {
                print_help();
                process::exit(0);
            }
            "--version" => {
                print_version();
                process::exit(0);
            }
            "--" => {
                // Remaining args are positional.
                for a in args.by_ref() {
                    positional.push(a);
                }
                break;
            }
            "-z" | "--zero-terminated" => {
                config.zero_terminated = true;
            }
            "--grouping" => {
                config.grouping = true;
            }
            _ => {
                if let Some(val) = arg.strip_prefix("--from=") {
                    match numfmt::parse_scale_unit(val) {
                        Ok(u) => config.from = u,
                        Err(e) => {
                            eprintln!("{}: {}", TOOL_NAME, e);
                            process::exit(1);
                        }
                    }
                } else if let Some(val) = arg.strip_prefix("--to=") {
                    match numfmt::parse_scale_unit(val) {
                        Ok(u) => config.to = u,
                        Err(e) => {
                            eprintln!("{}: {}", TOOL_NAME, e);
                            process::exit(1);
                        }
                    }
                } else if let Some(val) = arg.strip_prefix("--from-unit=") {
                    match val.parse::<f64>() {
                        Ok(n) if n > 0.0 => config.from_unit = n,
                        _ => {
                            eprintln!("{}: invalid unit size: '{}'", TOOL_NAME, val);
                            process::exit(1);
                        }
                    }
                } else if let Some(val) = arg.strip_prefix("--to-unit=") {
                    match val.parse::<f64>() {
                        Ok(n) if n > 0.0 => config.to_unit = n,
                        _ => {
                            eprintln!("{}: invalid unit size: '{}'", TOOL_NAME, val);
                            process::exit(1);
                        }
                    }
                } else if let Some(val) = arg.strip_prefix("--padding=") {
                    match val.parse::<i32>() {
                        Ok(n) if n != 0 => config.padding = Some(n),
                        _ => {
                            eprintln!("{}: invalid padding value: '{}'", TOOL_NAME, val);
                            process::exit(1);
                        }
                    }
                } else if let Some(val) = arg.strip_prefix("--round=") {
                    match numfmt::parse_round_method(val) {
                        Ok(m) => config.round = m,
                        Err(e) => {
                            eprintln!("{}: {}", TOOL_NAME, e);
                            process::exit(1);
                        }
                    }
                } else if let Some(val) = arg.strip_prefix("--suffix=") {
                    config.suffix = Some(val.to_string());
                } else if let Some(val) = arg.strip_prefix("--format=") {
                    config.format = Some(val.to_string());
                } else if let Some(val) = arg.strip_prefix("--field=") {
                    match numfmt::parse_fields(val) {
                        Ok(f) => config.field = f,
                        Err(e) => {
                            eprintln!("{}: {}", TOOL_NAME, e);
                            process::exit(1);
                        }
                    }
                } else if let Some(val) = arg.strip_prefix("--invalid=") {
                    match numfmt::parse_invalid_mode(val) {
                        Ok(m) => config.invalid = m,
                        Err(e) => {
                            eprintln!("{}: {}", TOOL_NAME, e);
                            process::exit(1);
                        }
                    }
                } else if arg == "--header" {
                    config.header = 1;
                } else if let Some(val) = arg.strip_prefix("--header=") {
                    match val.parse::<usize>() {
                        Ok(n) => config.header = n,
                        Err(_) => {
                            eprintln!("{}: invalid header value: '{}'", TOOL_NAME, val);
                            process::exit(1);
                        }
                    }
                } else if arg == "-d" || arg == "--delimiter" {
                    match args.next() {
                        Some(val) => {
                            if val.len() != 1 {
                                eprintln!(
                                    "{}: the delimiter must be a single character",
                                    TOOL_NAME
                                );
                                process::exit(1);
                            }
                            config.delimiter = val.chars().next();
                        }
                        None => {
                            eprintln!(
                                "{}: option requires an argument -- 'd'",
                                TOOL_NAME
                            );
                            process::exit(1);
                        }
                    }
                } else if let Some(val) = arg.strip_prefix("--delimiter=") {
                    if val.len() != 1 {
                        eprintln!(
                            "{}: the delimiter must be a single character",
                            TOOL_NAME
                        );
                        process::exit(1);
                    }
                    config.delimiter = val.chars().next();
                } else if let Some(val) = arg.strip_prefix("-d") {
                    if val.len() != 1 {
                        eprintln!(
                            "{}: the delimiter must be a single character",
                            TOOL_NAME
                        );
                        process::exit(1);
                    }
                    config.delimiter = val.chars().next();
                } else if arg.starts_with('-') && arg.len() > 1 {
                    // Could be a negative number.
                    if arg.chars().nth(1).is_some_and(|c| c.is_ascii_digit() || c == '.') {
                        positional.push(arg);
                    } else {
                        eprintln!("{}: unrecognized option '{}'", TOOL_NAME, arg);
                        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                        process::exit(1);
                    }
                } else {
                    positional.push(arg);
                }
            }
        }
    }

    (config, positional)
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let (config, positional) = parse_args();

    if positional.is_empty() {
        // Read from stdin.
        let stdin = io::stdin();
        let reader = stdin.lock();
        let stdout = io::stdout();
        let writer = BufWriter::new(stdout.lock());

        match numfmt::run_numfmt(reader, writer, &config) {
            Ok(()) => {}
            Err(_) => process::exit(2),
        }
    } else {
        // Process command-line arguments as numbers.
        let stdout = io::stdout();
        let mut writer = BufWriter::new(stdout.lock());
        let terminator = if config.zero_terminated { '\0' } else { '\n' };
        let mut had_error = false;

        for number in &positional {
            match numfmt::process_line(number, &config) {
                Ok(result) => {
                    let _ = write!(writer, "{}{}", result, terminator);
                }
                Err(e) => {
                    match config.invalid {
                        InvalidMode::Abort => {
                            eprintln!("{}: {}", TOOL_NAME, e);
                            process::exit(2);
                        }
                        InvalidMode::Fail => {
                            eprintln!("{}: {}", TOOL_NAME, e);
                            let _ = write!(writer, "{}{}", number, terminator);
                            had_error = true;
                        }
                        InvalidMode::Warn => {
                            eprintln!("{}: {}", TOOL_NAME, e);
                            let _ = write!(writer, "{}{}", number, terminator);
                        }
                        InvalidMode::Ignore => {
                            let _ = write!(writer, "{}{}", number, terminator);
                        }
                    }
                }
            }
        }

        let _ = writer.flush();
        if had_error {
            process::exit(2);
        }
    }
}
