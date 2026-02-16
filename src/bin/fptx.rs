use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process;

use coreutils_rs::ptx::{self, OutputFormat, PtxConfig};

struct Cli {
    config: PtxConfig,
    files: Vec<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        config: PtxConfig::default(),
        files: Vec::new(),
    };

    let mut args = std::env::args_os().skip(1);

    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            for a in args {
                cli.files.push(a.to_string_lossy().into_owned());
            }
            break;
        }
        if bytes.starts_with(b"--") {
            let s = arg.to_string_lossy();
            if let Some(val) = s.strip_prefix("--width=") {
                cli.config.width = val.parse().unwrap_or_else(|_| {
                    eprintln!("ptx: invalid width: '{}'", val);
                    process::exit(1);
                });
            } else if let Some(val) = s.strip_prefix("--gap-size=") {
                cli.config.gap_size = val.parse().unwrap_or_else(|_| {
                    eprintln!("ptx: invalid gap size: '{}'", val);
                    process::exit(1);
                });
            } else if let Some(val) = s.strip_prefix("--ignore-file=") {
                match ptx::read_word_file(val) {
                    Ok(words) => cli.config.ignore_words = words,
                    Err(e) => {
                        eprintln!("ptx: {}: {}", val, e);
                        process::exit(1);
                    }
                }
            } else if let Some(val) = s.strip_prefix("--only-file=") {
                match ptx::read_word_file(val) {
                    Ok(words) => cli.config.only_words = Some(words),
                    Err(e) => {
                        eprintln!("ptx: {}: {}", val, e);
                        process::exit(1);
                    }
                }
            } else if let Some(val) = s.strip_prefix("--break-file=") {
                cli.config.word_regexp = Some(val.to_string());
            } else if let Some(val) = s.strip_prefix("--sentence-regexp=") {
                cli.config.sentence_regexp = Some(val.to_string());
            } else if let Some(val) = s.strip_prefix("--word-regexp=") {
                cli.config.word_regexp = Some(val.to_string());
            } else {
                match bytes {
                    b"--ignore-case" => cli.config.ignore_case = true,
                    b"--auto-reference" => cli.config.auto_reference = true,
                    b"--traditional" => cli.config.traditional = true,
                    b"--references" => cli.config.references = true,
                    b"--right-side-refs" => cli.config.right_reference = true,
                    b"--format=roff" => cli.config.format = OutputFormat::Roff,
                    b"--format=tex" => cli.config.format = OutputFormat::Tex,
                    b"--width" => {
                        let val = args.next().unwrap_or_else(|| {
                            eprintln!("ptx: option '--width' requires an argument");
                            process::exit(1);
                        });
                        cli.config.width = val.to_string_lossy().parse().unwrap_or_else(|_| {
                            eprintln!("ptx: invalid width: '{}'", val.to_string_lossy());
                            process::exit(1);
                        });
                    }
                    b"--gap-size" => {
                        let val = args.next().unwrap_or_else(|| {
                            eprintln!("ptx: option '--gap-size' requires an argument");
                            process::exit(1);
                        });
                        cli.config.gap_size =
                            val.to_string_lossy().parse().unwrap_or_else(|_| {
                                eprintln!("ptx: invalid gap size: '{}'", val.to_string_lossy());
                                process::exit(1);
                            });
                    }
                    b"--ignore-file" => {
                        let val = args.next().unwrap_or_else(|| {
                            eprintln!("ptx: option '--ignore-file' requires an argument");
                            process::exit(1);
                        });
                        let path = val.to_string_lossy();
                        match ptx::read_word_file(&path) {
                            Ok(words) => cli.config.ignore_words = words,
                            Err(e) => {
                                eprintln!("ptx: {}: {}", path, e);
                                process::exit(1);
                            }
                        }
                    }
                    b"--only-file" => {
                        let val = args.next().unwrap_or_else(|| {
                            eprintln!("ptx: option '--only-file' requires an argument");
                            process::exit(1);
                        });
                        let path = val.to_string_lossy();
                        match ptx::read_word_file(&path) {
                            Ok(words) => cli.config.only_words = Some(words),
                            Err(e) => {
                                eprintln!("ptx: {}: {}", path, e);
                                process::exit(1);
                            }
                        }
                    }
                    b"--help" => {
                        print_help();
                        process::exit(0);
                    }
                    b"--version" => {
                        println!("ptx (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                        process::exit(0);
                    }
                    _ => {
                        eprintln!("ptx: unrecognized option '{}'", s);
                        eprintln!("Try 'ptx --help' for more information.");
                        process::exit(1);
                    }
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            // Short options
            let s = arg.to_string_lossy();
            let mut chars = s[1..].chars();
            while let Some(ch) = chars.next() {
                match ch {
                    'f' => cli.config.ignore_case = true,
                    'A' => cli.config.auto_reference = true,
                    'G' => cli.config.traditional = true,
                    'r' => cli.config.references = true,
                    'R' => cli.config.right_reference = true,
                    'T' => cli.config.format = OutputFormat::Tex,
                    'O' => cli.config.format = OutputFormat::Roff,
                    'w' => {
                        let rest: String = chars.collect();
                        let val_str = if rest.is_empty() {
                            let val = args.next().unwrap_or_else(|| {
                                eprintln!("ptx: option requires an argument -- 'w'");
                                process::exit(1);
                            });
                            val.to_string_lossy().into_owned()
                        } else {
                            rest
                        };
                        cli.config.width = val_str.parse().unwrap_or_else(|_| {
                            eprintln!("ptx: invalid width: '{}'", val_str);
                            process::exit(1);
                        });
                        break;
                    }
                    'g' => {
                        let rest: String = chars.collect();
                        let val_str = if rest.is_empty() {
                            let val = args.next().unwrap_or_else(|| {
                                eprintln!("ptx: option requires an argument -- 'g'");
                                process::exit(1);
                            });
                            val.to_string_lossy().into_owned()
                        } else {
                            rest
                        };
                        cli.config.gap_size = val_str.parse().unwrap_or_else(|_| {
                            eprintln!("ptx: invalid gap size: '{}'", val_str);
                            process::exit(1);
                        });
                        break;
                    }
                    'b' => {
                        let rest: String = chars.collect();
                        let val_str = if rest.is_empty() {
                            let val = args.next().unwrap_or_else(|| {
                                eprintln!("ptx: option requires an argument -- 'b'");
                                process::exit(1);
                            });
                            val.to_string_lossy().into_owned()
                        } else {
                            rest
                        };
                        cli.config.word_regexp = Some(val_str);
                        break;
                    }
                    'i' => {
                        let rest: String = chars.collect();
                        let val_str = if rest.is_empty() {
                            let val = args.next().unwrap_or_else(|| {
                                eprintln!("ptx: option requires an argument -- 'i'");
                                process::exit(1);
                            });
                            val.to_string_lossy().into_owned()
                        } else {
                            rest
                        };
                        match ptx::read_word_file(&val_str) {
                            Ok(words) => cli.config.ignore_words = words,
                            Err(e) => {
                                eprintln!("ptx: {}: {}", val_str, e);
                                process::exit(1);
                            }
                        }
                        break;
                    }
                    'o' => {
                        let rest: String = chars.collect();
                        let val_str = if rest.is_empty() {
                            let val = args.next().unwrap_or_else(|| {
                                eprintln!("ptx: option requires an argument -- 'o'");
                                process::exit(1);
                            });
                            val.to_string_lossy().into_owned()
                        } else {
                            rest
                        };
                        match ptx::read_word_file(&val_str) {
                            Ok(words) => cli.config.only_words = Some(words),
                            Err(e) => {
                                eprintln!("ptx: {}: {}", val_str, e);
                                process::exit(1);
                            }
                        }
                        break;
                    }
                    _ => {
                        eprintln!("ptx: invalid option -- '{}'", ch);
                        eprintln!("Try 'ptx --help' for more information.");
                        process::exit(1);
                    }
                }
            }
        } else {
            cli.files.push(arg.to_string_lossy().into_owned());
        }
    }

    cli
}

fn print_help() {
    print!(
        "Usage: ptx [OPTION]... [INPUT]...\n\
         Output a permuted index, including context, of the words in the input files.\n\n\
         With no FILE, or when FILE is -, read standard input.\n\n\
         \x20 -A, --auto-reference         output automatically generated references\n\
         \x20 -G, --traditional            behave more like System V 'ptx'\n\
         \x20 -T, --format=tex             generate output as TeX directives\n\
         \x20 -O, --format=roff            generate output as roff directives\n\
         \x20 -R, --right-side-refs        put references at right, not counted in -w\n\
         \x20 -f, --ignore-case            fold lower case to upper case for sorting\n\
         \x20 -g, --gap-size=NUMBER        gap size in columns between fields\n\
         \x20 -w, --width=NUMBER           output width in columns, reference excluded\n\
         \x20 -b, --break-file=FILE        word break characters in this FILE\n\
         \x20 -i, --ignore-file=FILE       read ignore word list from FILE\n\
         \x20 -o, --only-file=FILE         read only word list from this FILE\n\
         \x20 -r, --references             first field of each line is a reference\n\
         \x20     --help                   display this help and exit\n\
         \x20     --version                output version information and exit\n"
    );
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let cli = parse_args();

    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(64 * 1024, stdout.lock());

    if cli.files.is_empty() || (cli.files.len() == 1 && cli.files[0] == "-") {
        // Read from stdin
        let stdin = io::stdin();
        let reader = BufReader::new(stdin.lock());
        if let Err(e) = ptx::generate_ptx(reader, &mut out, &cli.config) {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("ptx: {}", e);
            process::exit(1);
        }
    } else {
        // Concatenate all input files
        let mut all_input = String::new();
        for file in &cli.files {
            if file == "-" {
                let stdin = io::stdin();
                for line in stdin.lock().lines() {
                    match line {
                        Ok(l) => {
                            all_input.push_str(&l);
                            all_input.push('\n');
                        }
                        Err(e) => {
                            eprintln!("ptx: standard input: {}", e);
                            process::exit(1);
                        }
                    }
                }
            } else {
                match std::fs::read_to_string(file) {
                    Ok(content) => all_input.push_str(&content),
                    Err(e) => {
                        eprintln!("ptx: {}: {}", file, e);
                        process::exit(1);
                    }
                }
            }
        }

        let reader = io::BufReader::new(all_input.as_bytes());
        if let Err(e) = ptx::generate_ptx(reader, &mut out, &cli.config) {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("ptx: {}", e);
            process::exit(1);
        }
    }

    if let Err(e) = out.flush() {
        #[allow(clippy::collapsible_if)]
        if e.kind() != io::ErrorKind::BrokenPipe {
            eprintln!("ptx: write error: {}", e);
            process::exit(1);
        }
    }
}
