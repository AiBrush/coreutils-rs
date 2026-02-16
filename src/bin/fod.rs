// fod -- octal/hex dump
//
// Usage: od [OPTION]... [FILE]...
//        od [-abcdfilosx] [FILE] [[+]OFFSET[.][b]]

use std::fs::File;
use std::io::{self, Read};
use std::process;

use coreutils_rs::common::reset_sigpipe;
use coreutils_rs::od::{AddressRadix, OdConfig, OutputFormat, od_process, parse_format_type};

const TOOL_NAME: &str = "od";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut address_radix = None;
    let mut formats: Vec<OutputFormat> = Vec::new();
    let mut skip_bytes: u64 = 0;
    let mut read_bytes: Option<u64> = None;
    let mut width: Option<usize> = None;
    let mut show_duplicates = false;
    let mut operands: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if saw_dashdash {
            operands.push(arg.clone());
            i += 1;
            continue;
        }
        match arg.as_str() {
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--" => saw_dashdash = true,
            "-v" | "--output-duplicates" => show_duplicates = true,

            "--traditional" => { /* accepted, ignored */ }

            // Traditional format shortcuts
            "-a" => formats.push(OutputFormat::NamedChar),
            "-b" => formats.push(OutputFormat::Octal(1)),
            "-c" => formats.push(OutputFormat::PrintableChar),
            "-d" => formats.push(OutputFormat::UnsignedDec(2)),
            "-e" => formats.push(OutputFormat::Float(8)),
            "-f" => formats.push(OutputFormat::Float(4)),
            "-h" => formats.push(OutputFormat::Hex(2)),
            "-i" => formats.push(OutputFormat::SignedDec(4)),
            "-l" => formats.push(OutputFormat::SignedDec(8)),
            "-o" => formats.push(OutputFormat::Octal(2)),
            "-s" => formats.push(OutputFormat::SignedDec(2)),
            "-x" => formats.push(OutputFormat::Hex(2)),

            _ if arg.starts_with("--address-radix=") => {
                address_radix = Some(parse_radix(&arg["--address-radix=".len()..]));
            }
            _ if arg.starts_with("--format=") => {
                let fmt_str = &arg["--format=".len()..];
                match parse_format_type(fmt_str) {
                    Ok(f) => formats.push(f),
                    Err(e) => {
                        eprintln!("{}: {}", TOOL_NAME, e);
                        process::exit(1);
                    }
                }
            }
            _ if arg.starts_with("--skip-bytes=") => {
                skip_bytes = parse_offset(&arg["--skip-bytes=".len()..]);
            }
            _ if arg.starts_with("--read-bytes=") => {
                read_bytes = Some(parse_offset(&arg["--read-bytes=".len()..]));
            }
            _ if arg == "--width" => {
                // --width without =
                i += 1;
                if i < args.len() {
                    width = Some(args[i].parse().unwrap_or(16));
                }
            }
            _ if arg.starts_with("--width=") => {
                width = Some(arg["--width=".len()..].parse().unwrap_or(16));
            }
            _ if arg.starts_with("--width") => {
                // --width with no argument means default 32
                width = Some(32);
            }
            _ if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") => {
                // Short options
                let bytes = arg.as_bytes();
                let mut j = 1;
                while j < bytes.len() {
                    match bytes[j] {
                        b'A' => {
                            j += 1;
                            if j < bytes.len() {
                                address_radix =
                                    Some(parse_radix(&String::from_utf8_lossy(&bytes[j..j + 1])));
                                j += 1;
                            } else {
                                i += 1;
                                if i < args.len() {
                                    address_radix = Some(parse_radix(&args[i]));
                                }
                            }
                            continue;
                        }
                        b'j' => {
                            let rest: String =
                                String::from_utf8_lossy(&bytes[j + 1..]).into_owned();
                            if rest.is_empty() {
                                i += 1;
                                if i < args.len() {
                                    skip_bytes = parse_offset(&args[i]);
                                }
                            } else {
                                skip_bytes = parse_offset(&rest);
                            }
                            j = bytes.len(); // consumed rest
                            continue;
                        }
                        b'N' => {
                            let rest: String =
                                String::from_utf8_lossy(&bytes[j + 1..]).into_owned();
                            if rest.is_empty() {
                                i += 1;
                                if i < args.len() {
                                    read_bytes = Some(parse_offset(&args[i]));
                                }
                            } else {
                                read_bytes = Some(parse_offset(&rest));
                            }
                            j = bytes.len();
                            continue;
                        }
                        b't' => {
                            let rest: String =
                                String::from_utf8_lossy(&bytes[j + 1..]).into_owned();
                            let fmt_str = if rest.is_empty() {
                                i += 1;
                                if i < args.len() {
                                    args[i].clone()
                                } else {
                                    eprintln!("{}: option requires an argument -- 't'", TOOL_NAME);
                                    process::exit(1);
                                }
                            } else {
                                rest
                            };
                            match parse_format_type(&fmt_str) {
                                Ok(f) => formats.push(f),
                                Err(e) => {
                                    eprintln!("{}: {}", TOOL_NAME, e);
                                    process::exit(1);
                                }
                            }
                            j = bytes.len();
                            continue;
                        }
                        b'w' => {
                            let rest: String =
                                String::from_utf8_lossy(&bytes[j + 1..]).into_owned();
                            if rest.is_empty() {
                                // -w alone means default 32
                                width = Some(32);
                            } else {
                                width = Some(rest.parse().unwrap_or(16));
                            }
                            j = bytes.len();
                            continue;
                        }
                        b'v' => show_duplicates = true,
                        b'a' => formats.push(OutputFormat::NamedChar),
                        b'b' => formats.push(OutputFormat::Octal(1)),
                        b'c' => formats.push(OutputFormat::PrintableChar),
                        b'd' => formats.push(OutputFormat::UnsignedDec(2)),
                        b'e' => formats.push(OutputFormat::Float(8)),
                        b'f' => formats.push(OutputFormat::Float(4)),
                        b'h' => formats.push(OutputFormat::Hex(2)),
                        b'i' => formats.push(OutputFormat::SignedDec(4)),
                        b'l' => formats.push(OutputFormat::SignedDec(8)),
                        b'o' => formats.push(OutputFormat::Octal(2)),
                        b's' => formats.push(OutputFormat::SignedDec(2)),
                        b'x' => formats.push(OutputFormat::Hex(2)),
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, bytes[j] as char);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    j += 1;
                }
            }
            _ if arg.starts_with('+') => {
                // Traditional offset: +OFFSET[.][b]
                skip_bytes = parse_offset(&arg[1..]);
            }
            _ => operands.push(arg.clone()),
        }
        i += 1;
    }

    let config = OdConfig {
        address_radix: address_radix.unwrap_or(AddressRadix::Octal),
        formats: if formats.is_empty() {
            vec![OutputFormat::Octal(2)]
        } else {
            formats
        },
        skip_bytes,
        read_bytes,
        width: width.unwrap_or(16),
        show_duplicates,
    };

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());

    if operands.is_empty() || (operands.len() == 1 && operands[0] == "-") {
        let stdin = io::stdin();
        let reader = stdin.lock();
        if let Err(e) = od_process(reader, &mut out, &config) {
            eprintln!("{}: {}", TOOL_NAME, e);
            process::exit(1);
        }
    } else {
        // Concatenate all files
        let mut combined = Vec::new();
        for path in &operands {
            if path == "-" {
                let mut buf = Vec::new();
                io::stdin()
                    .lock()
                    .read_to_end(&mut buf)
                    .unwrap_or_else(|e| {
                        eprintln!("{}: standard input: {}", TOOL_NAME, e);
                        process::exit(1);
                    });
                combined.extend_from_slice(&buf);
            } else {
                match File::open(path) {
                    Ok(mut f) => {
                        let mut buf = Vec::new();
                        f.read_to_end(&mut buf).unwrap_or_else(|e| {
                            eprintln!("{}: {}: {}", TOOL_NAME, path, e);
                            process::exit(1);
                        });
                        combined.extend_from_slice(&buf);
                    }
                    Err(e) => {
                        eprintln!("{}: {}: {}", TOOL_NAME, path, e);
                        process::exit(1);
                    }
                }
            }
        }
        if let Err(e) = od_process(combined.as_slice(), &mut out, &config) {
            eprintln!("{}: {}", TOOL_NAME, e);
            process::exit(1);
        }
    }
}

fn parse_radix(s: &str) -> AddressRadix {
    match s {
        "o" => AddressRadix::Octal,
        "d" => AddressRadix::Decimal,
        "x" => AddressRadix::Hex,
        "n" => AddressRadix::None,
        _ => {
            eprintln!(
                "{}: invalid address radix '{}'; it must be one character from [doxn]",
                TOOL_NAME, s
            );
            process::exit(1);
        }
    }
}

fn parse_offset(s: &str) -> u64 {
    let s = s.trim();
    let (num_str, multiplier) = if let Some(n) = s.strip_suffix("GiB") {
        (n, 1_073_741_824u64)
    } else if let Some(n) = s.strip_suffix("GB") {
        (n, 1_000_000_000u64)
    } else if let Some(n) = s.strip_suffix('G') {
        (n, 1_073_741_824u64)
    } else if let Some(n) = s.strip_suffix("MiB") {
        (n, 1_048_576u64)
    } else if let Some(n) = s.strip_suffix("MB") {
        (n, 1_000_000u64)
    } else if let Some(n) = s.strip_suffix('M') {
        (n, 1_048_576u64)
    } else if let Some(n) = s.strip_suffix("KiB") {
        (n, 1_024u64)
    } else if let Some(n) = s.strip_suffix("KB") {
        (n, 1_000u64)
    } else if let Some(n) = s.strip_suffix("kB") {
        (n, 1_000u64)
    } else if let Some(n) = s.strip_suffix('K') {
        (n, 1_024u64)
    } else if let Some(n) = s.strip_suffix('k') {
        (n, 1_024u64)
    } else if let Some(n) = s.strip_suffix('b') {
        (n, 512u64)
    } else {
        (s, 1u64)
    };

    let base_val = if num_str.starts_with("0x") || num_str.starts_with("0X") {
        u64::from_str_radix(&num_str[2..], 16).unwrap_or(0)
    } else if num_str.starts_with('0') && num_str.len() > 1 {
        u64::from_str_radix(&num_str[1..], 8).unwrap_or(0)
    } else {
        num_str.parse::<u64>().unwrap_or(0)
    };

    base_val * multiplier
}

fn print_help() {
    println!("Usage: {} [OPTION]... [FILE]...", TOOL_NAME);
    println!("  or:  {} -C [OPTION]... [FILE]...", TOOL_NAME);
    println!("Write an unambiguous representation, octal bytes by default,");
    println!("of FILE to standard output.");
    println!();
    println!("  -A, --address-radix=RADIX   output format for file offsets; RADIX is one");
    println!("                                of [doxn], for Decimal, Octal, Hex or None");
    println!("  -j, --skip-bytes=BYTES      skip BYTES input bytes first");
    println!("  -N, --read-bytes=BYTES      limit dump to BYTES input bytes");
    println!("  -t, --format=TYPE           select output format or formats");
    println!("  -v, --output-duplicates     do not use * to mark line suppression");
    println!("  -w[BYTES], --width[=BYTES]  output BYTES bytes per output line;");
    println!("                                32 is implied when BYTES is not specified");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
    println!();
    println!("Traditional format specifications may be intermixed:");
    println!("  -b   same as -t o1");
    println!("  -c   same as -t c");
    println!("  -d   same as -t u2");
    println!("  -o   same as -t o2");
    println!("  -s   same as -t d2");
    println!("  -x   same as -t x2");
}
