use std::process;

use coreutils_rs::common::reset_sigpipe;
use coreutils_rs::split::{self, SplitConfig, SplitMode, SuffixType};

struct Cli {
    config: SplitConfig,
    input: String,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        config: SplitConfig::default(),
        input: "-".to_string(),
    };

    let mut args = std::env::args_os().skip(1);
    let mut positional_count = 0;

    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            // Remaining args are positional
            for a in args {
                let s = a.to_string_lossy().into_owned();
                match positional_count {
                    0 => cli.input = s,
                    1 => cli.config.prefix = s,
                    _ => {
                        eprintln!("split: extra operand '{}'", s);
                        eprintln!("Try 'split --help' for more information.");
                        process::exit(1);
                    }
                }
                positional_count += 1;
            }
            break;
        }
        if bytes.starts_with(b"--") {
            let arg_str = arg.to_string_lossy();
            let arg_ref: &str = &arg_str;
            if let Some(val) = arg_ref.strip_prefix("--suffix-length=") {
                cli.config.suffix_length = val.parse().unwrap_or_else(|_| {
                    eprintln!("split: invalid suffix length: '{}'", val);
                    process::exit(1);
                });
            } else if let Some(val) = arg_ref.strip_prefix("--bytes=") {
                let size = split::parse_size(val).unwrap_or_else(|e| {
                    eprintln!("split: invalid number of bytes: '{}'", e);
                    process::exit(1);
                });
                cli.config.mode = SplitMode::Bytes(size);
            } else if let Some(val) = arg_ref.strip_prefix("--line-bytes=") {
                let size = split::parse_size(val).unwrap_or_else(|e| {
                    eprintln!("split: invalid number of bytes: '{}'", e);
                    process::exit(1);
                });
                cli.config.mode = SplitMode::LineBytes(size);
            } else if let Some(val) = arg_ref.strip_prefix("--lines=") {
                let n: u64 = val.parse().unwrap_or_else(|_| {
                    eprintln!("split: invalid number of lines: '{}'", val);
                    process::exit(1);
                });
                cli.config.mode = SplitMode::Lines(n);
            } else if let Some(val) = arg_ref.strip_prefix("--number=") {
                let n: u64 = val.parse().unwrap_or_else(|_| {
                    eprintln!("split: invalid number of chunks: '{}'", val);
                    process::exit(1);
                });
                cli.config.mode = SplitMode::Number(n);
            } else if let Some(val) = arg_ref.strip_prefix("--additional-suffix=") {
                cli.config.additional_suffix = val.to_string();
            } else if let Some(val) = arg_ref.strip_prefix("--numeric-suffixes=") {
                let from: u64 = val.parse().unwrap_or_else(|_| {
                    eprintln!("split: invalid start value: '{}'", val);
                    process::exit(1);
                });
                cli.config.suffix_type = SuffixType::Numeric(from);
            } else if arg_ref == "--numeric-suffixes" {
                cli.config.suffix_type = SuffixType::Numeric(0);
            } else if let Some(val) = arg_ref.strip_prefix("--hex-suffixes=") {
                let from: u64 = val.parse().unwrap_or_else(|_| {
                    eprintln!("split: invalid start value: '{}'", val);
                    process::exit(1);
                });
                cli.config.suffix_type = SuffixType::Hex(from);
            } else if arg_ref == "--hex-suffixes" {
                cli.config.suffix_type = SuffixType::Hex(0);
            } else if let Some(val) = arg_ref.strip_prefix("--filter=") {
                cli.config.filter = Some(val.to_string());
            } else if let Some(val) = arg_ref.strip_prefix("--separator=") {
                if val.len() == 1 {
                    cli.config.separator = val.as_bytes()[0];
                } else if val.is_empty() {
                    cli.config.separator = b'\0';
                } else {
                    eprintln!("split: multi-character separator '{}'", val);
                    process::exit(1);
                }
            } else if arg_ref == "--elide-empty-files" {
                cli.config.elide_empty = true;
            } else if arg_ref == "--verbose" {
                cli.config.verbose = true;
            } else if arg_ref == "--help" {
                print_help();
                process::exit(0);
            } else if arg_ref == "--version" {
                println!("split (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                process::exit(0);
            } else {
                eprintln!("split: unrecognized option '{}'", arg_str);
                eprintln!("Try 'split --help' for more information.");
                process::exit(1);
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            // Short options: may be clustered
            let arg_str = arg.to_string_lossy();
            let chars: Vec<char> = arg_str[1..].chars().collect();
            let mut i = 0;
            while i < chars.len() {
                match chars[i] {
                    'a' => {
                        // -a N: suffix length
                        let val = if i + 1 < chars.len() {
                            chars[i + 1..].iter().collect::<String>()
                        } else {
                            args.next()
                                .unwrap_or_else(|| {
                                    eprintln!("split: option requires an argument -- 'a'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned()
                        };
                        cli.config.suffix_length = val.parse().unwrap_or_else(|_| {
                            eprintln!("split: invalid suffix length: '{}'", val);
                            process::exit(1);
                        });
                        break; // consumed rest of cluster
                    }
                    'b' => {
                        let val = if i + 1 < chars.len() {
                            chars[i + 1..].iter().collect::<String>()
                        } else {
                            args.next()
                                .unwrap_or_else(|| {
                                    eprintln!("split: option requires an argument -- 'b'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned()
                        };
                        let size = split::parse_size(&val).unwrap_or_else(|e| {
                            eprintln!("split: invalid number of bytes: '{}'", e);
                            process::exit(1);
                        });
                        cli.config.mode = SplitMode::Bytes(size);
                        break;
                    }
                    'C' => {
                        let val = if i + 1 < chars.len() {
                            chars[i + 1..].iter().collect::<String>()
                        } else {
                            args.next()
                                .unwrap_or_else(|| {
                                    eprintln!("split: option requires an argument -- 'C'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned()
                        };
                        let size = split::parse_size(&val).unwrap_or_else(|e| {
                            eprintln!("split: invalid number of bytes: '{}'", e);
                            process::exit(1);
                        });
                        cli.config.mode = SplitMode::LineBytes(size);
                        break;
                    }
                    'l' => {
                        let val = if i + 1 < chars.len() {
                            chars[i + 1..].iter().collect::<String>()
                        } else {
                            args.next()
                                .unwrap_or_else(|| {
                                    eprintln!("split: option requires an argument -- 'l'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned()
                        };
                        let n: u64 = val.parse().unwrap_or_else(|_| {
                            eprintln!("split: invalid number of lines: '{}'", val);
                            process::exit(1);
                        });
                        cli.config.mode = SplitMode::Lines(n);
                        break;
                    }
                    'n' => {
                        let val = if i + 1 < chars.len() {
                            chars[i + 1..].iter().collect::<String>()
                        } else {
                            args.next()
                                .unwrap_or_else(|| {
                                    eprintln!("split: option requires an argument -- 'n'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned()
                        };
                        let n: u64 = val.parse().unwrap_or_else(|_| {
                            eprintln!("split: invalid number of chunks: '{}'", val);
                            process::exit(1);
                        });
                        cli.config.mode = SplitMode::Number(n);
                        break;
                    }
                    't' => {
                        let val = if i + 1 < chars.len() {
                            chars[i + 1..].iter().collect::<String>()
                        } else {
                            args.next()
                                .unwrap_or_else(|| {
                                    eprintln!("split: option requires an argument -- 't'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned()
                        };
                        if val.len() == 1 {
                            cli.config.separator = val.as_bytes()[0];
                        } else if val.is_empty() {
                            cli.config.separator = b'\0';
                        } else {
                            eprintln!("split: multi-character separator '{}'", val);
                            process::exit(1);
                        }
                        break;
                    }
                    'd' => {
                        cli.config.suffix_type = SuffixType::Numeric(0);
                    }
                    'x' => {
                        cli.config.suffix_type = SuffixType::Hex(0);
                    }
                    'e' => {
                        cli.config.elide_empty = true;
                    }
                    _ => {
                        eprintln!("split: invalid option -- '{}'", chars[i]);
                        eprintln!("Try 'split --help' for more information.");
                        process::exit(1);
                    }
                }
                i += 1;
            }
        } else {
            // Positional argument
            let s = arg.to_string_lossy().into_owned();
            match positional_count {
                0 => cli.input = s,
                1 => cli.config.prefix = s,
                _ => {
                    eprintln!("split: extra operand '{}'", s);
                    eprintln!("Try 'split --help' for more information.");
                    process::exit(1);
                }
            }
            positional_count += 1;
        }
    }

    cli
}

fn print_help() {
    print!(
        "Usage: split [OPTION]... [FILE [PREFIX]]\n\
         Output pieces of FILE to PREFIXaa, PREFIXab, ...;\n\
         default size is 1000 lines, and default PREFIX is 'x'.\n\n\
         With no FILE, or when FILE is -, read standard input.\n\n\
         \x20 -a, --suffix-length=N   generate suffixes of length N (default 2)\n\
         \x20 --additional-suffix=SUFF  append an additional SUFFIX to file names\n\
         \x20 -b, --bytes=SIZE        put SIZE bytes per output file\n\
         \x20 -C, --line-bytes=SIZE   put at most SIZE bytes of records per output file\n\
         \x20 -d                      use numeric suffixes starting at 0\n\
         \x20 --numeric-suffixes[=FROM]  same as -d, but allow setting the start value\n\
         \x20 -x                      use hex suffixes starting at 0\n\
         \x20 --hex-suffixes[=FROM]   same as -x, but allow setting the start value\n\
         \x20 -e, --elide-empty-files  do not generate empty output files with '-n'\n\
         \x20 --filter=COMMAND        write to shell COMMAND; file name is $FILE\n\
         \x20 -l, --lines=NUMBER      put NUMBER lines/records per output file\n\
         \x20 -n, --number=CHUNKS     generate CHUNKS output files\n\
         \x20 -t, --separator=SEP     use SEP instead of newline as the record separator\n\
         \x20 --verbose               print a diagnostic just before each output file is opened\n\
         \x20 --help                  display this help and exit\n\
         \x20 --version               output version information and exit\n\n\
         The SIZE argument is an integer and optional unit (example: 10K is 10*1024).\n\
         Units are K, M, G, T, P, E (powers of 1024) or KB, MB, ... (powers of 1000).\n"
    );
}

fn main() {
    reset_sigpipe();

    let cli = parse_args();

    if let Err(e) = split::split_file(&cli.input, &cli.config) {
        eprintln!("split: {}", e);
        process::exit(1);
    }
}
