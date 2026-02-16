use std::io::{self, BufWriter, Write};
use std::process;

use coreutils_rs::common::{io_error_msg, reset_sigpipe};
use coreutils_rs::tail::{self, FollowMode, TailConfig, TailMode};

struct Cli {
    config: TailConfig,
    quiet: bool,
    verbose: bool,
    files: Vec<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        config: TailConfig::default(),
        quiet: false,
        verbose: false,
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
            if let Some(val) = s.strip_prefix("--lines=") {
                parse_lines_value(val, &mut cli.config);
            } else if let Some(val) = s.strip_prefix("--bytes=") {
                parse_bytes_value(val, &mut cli.config);
            } else if let Some(val) = s.strip_prefix("--pid=") {
                cli.config.pid = Some(val.parse().unwrap_or_else(|_| {
                    eprintln!("tail: invalid PID: '{}'", val);
                    process::exit(1);
                }));
            } else if let Some(val) = s.strip_prefix("--sleep-interval=") {
                cli.config.sleep_interval = val.parse().unwrap_or_else(|_| {
                    eprintln!("tail: invalid number of seconds: '{}'", val);
                    process::exit(1);
                });
            } else if let Some(val) = s.strip_prefix("--max-unchanged-stats=") {
                cli.config.max_unchanged_stats = val.parse().unwrap_or_else(|_| {
                    eprintln!("tail: invalid number: '{}'", val);
                    process::exit(1);
                });
            } else if let Some(val) = s.strip_prefix("--follow=") {
                match val {
                    "name" => cli.config.follow = FollowMode::Name,
                    "descriptor" => cli.config.follow = FollowMode::Descriptor,
                    _ => {
                        eprintln!("tail: invalid argument '{}' for '--follow'", val);
                        process::exit(1);
                    }
                }
            } else {
                match bytes {
                    b"--lines" => {
                        let val = args.next().unwrap_or_else(|| {
                            eprintln!("tail: option '--lines' requires an argument");
                            process::exit(1);
                        });
                        parse_lines_value(&val.to_string_lossy(), &mut cli.config);
                    }
                    b"--bytes" => {
                        let val = args.next().unwrap_or_else(|| {
                            eprintln!("tail: option '--bytes' requires an argument");
                            process::exit(1);
                        });
                        parse_bytes_value(&val.to_string_lossy(), &mut cli.config);
                    }
                    b"--follow" => cli.config.follow = FollowMode::Descriptor,
                    b"--retry" => cli.config.retry = true,
                    b"--quiet" | b"--silent" => cli.quiet = true,
                    b"--verbose" => cli.verbose = true,
                    b"--zero-terminated" => cli.config.zero_terminated = true,
                    b"--pid" => {
                        let val = args.next().unwrap_or_else(|| {
                            eprintln!("tail: option '--pid' requires an argument");
                            process::exit(1);
                        });
                        cli.config.pid = Some(val.to_string_lossy().parse().unwrap_or_else(|_| {
                            eprintln!("tail: invalid PID: '{}'", val.to_string_lossy());
                            process::exit(1);
                        }));
                    }
                    b"--sleep-interval" => {
                        let val = args.next().unwrap_or_else(|| {
                            eprintln!("tail: option '--sleep-interval' requires an argument");
                            process::exit(1);
                        });
                        cli.config.sleep_interval =
                            val.to_string_lossy().parse().unwrap_or_else(|_| {
                                eprintln!(
                                    "tail: invalid number of seconds: '{}'",
                                    val.to_string_lossy()
                                );
                                process::exit(1);
                            });
                    }
                    b"--max-unchanged-stats" => {
                        let val = args.next().unwrap_or_else(|| {
                            eprintln!("tail: option '--max-unchanged-stats' requires an argument");
                            process::exit(1);
                        });
                        cli.config.max_unchanged_stats =
                            val.to_string_lossy().parse().unwrap_or_else(|_| {
                                eprintln!("tail: invalid number: '{}'", val.to_string_lossy());
                                process::exit(1);
                            });
                    }
                    b"--help" => {
                        print_help();
                        process::exit(0);
                    }
                    b"--version" => {
                        println!("tail (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                        process::exit(0);
                    }
                    _ => {
                        eprintln!("tail: unrecognized option '{}'", s);
                        eprintln!("Try 'tail --help' for more information.");
                        process::exit(1);
                    }
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            let s = arg.to_string_lossy();
            let chars: Vec<char> = s[1..].chars().collect();
            let mut i = 0;
            while i < chars.len() {
                match chars[i] {
                    'n' => {
                        let val = if i + 1 < chars.len() {
                            s[1 + i + 1..].to_string()
                        } else {
                            args.next()
                                .unwrap_or_else(|| {
                                    eprintln!("tail: option requires an argument -- 'n'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned()
                        };
                        parse_lines_value(&val, &mut cli.config);
                        break;
                    }
                    'c' => {
                        let val = if i + 1 < chars.len() {
                            s[1 + i + 1..].to_string()
                        } else {
                            args.next()
                                .unwrap_or_else(|| {
                                    eprintln!("tail: option requires an argument -- 'c'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned()
                        };
                        parse_bytes_value(&val, &mut cli.config);
                        break;
                    }
                    'f' => cli.config.follow = FollowMode::Descriptor,
                    'F' => {
                        cli.config.follow = FollowMode::Name;
                        cli.config.retry = true;
                    }
                    'q' => cli.quiet = true,
                    'v' => cli.verbose = true,
                    'z' => cli.config.zero_terminated = true,
                    's' => {
                        let val = if i + 1 < chars.len() {
                            s[1 + i + 1..].to_string()
                        } else {
                            args.next()
                                .unwrap_or_else(|| {
                                    eprintln!("tail: option requires an argument -- 's'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned()
                        };
                        cli.config.sleep_interval = val.parse().unwrap_or_else(|_| {
                            eprintln!("tail: invalid number of seconds: '{}'", val);
                            process::exit(1);
                        });
                        break;
                    }
                    '0'..='9' | '+' => {
                        // Legacy: tail -N means tail -n N, tail +N means tail -n +N
                        let num_str: String = chars[i..].iter().collect();
                        parse_lines_value(&num_str, &mut cli.config);
                        break;
                    }
                    _ => {
                        eprintln!("tail: invalid option -- '{}'", chars[i]);
                        eprintln!("Try 'tail --help' for more information.");
                        process::exit(1);
                    }
                }
                i += 1;
            }
        } else {
            cli.files.push(arg.to_string_lossy().into_owned());
        }
    }

    cli
}

fn parse_lines_value(val: &str, config: &mut TailConfig) {
    if let Some(stripped) = val.strip_prefix('+') {
        match tail::parse_size(stripped) {
            Ok(n) => config.mode = TailMode::LinesFrom(n),
            Err(_) => {
                eprintln!("tail: invalid number of lines: '{}'", val);
                process::exit(1);
            }
        }
    } else {
        let clean = val.strip_prefix('-').unwrap_or(val);
        match tail::parse_size(clean) {
            Ok(n) => config.mode = TailMode::Lines(n),
            Err(_) => {
                eprintln!("tail: invalid number of lines: '{}'", val);
                process::exit(1);
            }
        }
    }
}

fn parse_bytes_value(val: &str, config: &mut TailConfig) {
    if let Some(stripped) = val.strip_prefix('+') {
        match tail::parse_size(stripped) {
            Ok(n) => config.mode = TailMode::BytesFrom(n),
            Err(_) => {
                eprintln!("tail: invalid number of bytes: '{}'", val);
                process::exit(1);
            }
        }
    } else {
        let clean = val.strip_prefix('-').unwrap_or(val);
        match tail::parse_size(clean) {
            Ok(n) => config.mode = TailMode::Bytes(n),
            Err(_) => {
                eprintln!("tail: invalid number of bytes: '{}'", val);
                process::exit(1);
            }
        }
    }
}

fn print_help() {
    print!(
        "Usage: tail [OPTION]... [FILE]...\n\
         Print the last 10 lines of each FILE to standard output.\n\
         With more than one FILE, precede each with a header giving the file name.\n\n\
         With no FILE, or when FILE is -, read standard input.\n\n\
         Mandatory arguments to long options are mandatory for short options too.\n\
         \x20 -c, --bytes=[+]NUM       output the last NUM bytes; or use -c +NUM to\n\
         \x20                          output starting with byte NUM of each file\n\
         \x20 -f, --follow[={{name|descriptor}}]\n\
         \x20                          output appended data as the file grows;\n\
         \x20                          an absent option argument means 'descriptor'\n\
         \x20 -F                       same as --follow=name --retry\n\
         \x20 -n, --lines=[+]NUM       output the last NUM lines, instead of the last 10;\n\
         \x20                          or use -n +NUM to output starting with line NUM\n\
         \x20     --max-unchanged-stats=N\n\
         \x20                          with --follow=name, reopen a FILE which has not\n\
         \x20                          changed size after N (default 5) iterations\n\
         \x20     --pid=PID            with -f, terminate after process ID, PID dies\n\
         \x20 -q, --quiet, --silent    never output headers giving file names\n\
         \x20     --retry              keep trying to open a file if it is inaccessible\n\
         \x20 -s, --sleep-interval=N   with -f, sleep for approximately N seconds\n\
         \x20                          (default 1.0) between iterations\n\
         \x20 -v, --verbose            always output headers giving file names\n\
         \x20 -z, --zero-terminated    line delimiter is NUL, not newline\n\
         \x20     --help               display this help and exit\n\
         \x20     --version            output version information and exit\n\n\
         NUM may have a multiplier suffix:\n\
         b 512, kB 1000, K 1024, MB 1000*1000, M 1024*1024,\n\
         GB 1000*1000*1000, G 1024*1024*1024, and so on for T, P, E, Z, Y.\n\
         Binary prefixes can be used, too: KiB=K, MiB=M, and so on.\n"
    );
}

/// Enlarge pipe buffers on Linux.
#[cfg(target_os = "linux")]
fn enlarge_pipes() {
    for &fd in &[0i32, 1] {
        for &size in &[8 * 1024 * 1024i32, 1024 * 1024, 256 * 1024] {
            if unsafe { libc::fcntl(fd, libc::F_SETPIPE_SZ, size) } > 0 {
                break;
            }
        }
    }
}

fn main() {
    reset_sigpipe();

    #[cfg(target_os = "linux")]
    enlarge_pipes();

    let cli = parse_args();

    let files: Vec<String> = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files
    };

    let tool_name = "tail";
    let show_headers = if cli.quiet {
        false
    } else if cli.verbose {
        true
    } else {
        files.len() > 1
    };

    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(256 * 1024, stdout.lock());
    let mut had_error = false;
    let mut first = true;

    for filename in &files {
        if show_headers {
            if !first {
                let _ = out.write_all(b"\n");
            }
            let display_name = if filename == "-" {
                "standard input"
            } else {
                filename.as_str()
            };
            let _ = writeln!(out, "==> {} <==", display_name);
        }
        first = false;

        match tail::tail_file(filename, &cli.config, &mut out, tool_name) {
            Ok(true) => {}
            Ok(false) => had_error = true,
            Err(e) => {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    let _ = out.flush();
                    process::exit(0);
                }
                eprintln!("{}: write error: {}", tool_name, io_error_msg(&e));
                had_error = true;
            }
        }
    }

    let _ = out.flush();

    // Follow mode
    if cli.config.follow != FollowMode::None {
        for filename in &files {
            if filename != "-" {
                let _ = tail::follow_file(filename, &cli.config, &mut out);
            }
        }
    }

    if had_error {
        process::exit(1);
    }
}
