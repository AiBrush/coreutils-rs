use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::process;

use coreutils_rs::common::io_error_msg;
use coreutils_rs::fmt::FmtConfig;

struct Cli {
    width: usize,
    goal: Option<usize>,
    split_only: bool,
    crown_margin: bool,
    tagged: bool,
    uniform_spacing: bool,
    prefix: Option<String>,
    files: Vec<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        width: 75,
        goal: None,
        split_only: false,
        crown_margin: false,
        tagged: false,
        uniform_spacing: false,
        prefix: None,
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
            // Handle --key=value forms.
            if bytes.starts_with(b"--width=") {
                let val = arg.to_string_lossy();
                match val[8..].parse::<usize>() {
                    Ok(w) => cli.width = w,
                    Err(_) => {
                        eprintln!("fmt: invalid width: '{}'", &val[8..]);
                        process::exit(1);
                    }
                }
                continue;
            }
            if bytes.starts_with(b"--goal=") {
                let val = arg.to_string_lossy();
                match val[7..].parse::<usize>() {
                    Ok(g) => cli.goal = Some(g),
                    Err(_) => {
                        eprintln!("fmt: invalid goal width: '{}'", &val[7..]);
                        process::exit(1);
                    }
                }
                continue;
            }
            if bytes.starts_with(b"--prefix=") {
                let val = arg.to_string_lossy();
                cli.prefix = Some(val[9..].to_string());
                continue;
            }
            match bytes {
                b"--crown-margin" => cli.crown_margin = true,
                b"--split-only" => cli.split_only = true,
                b"--tagged-paragraph" => cli.tagged = true,
                b"--uniform-spacing" => cli.uniform_spacing = true,
                b"--width" => {
                    let val = args
                        .next()
                        .unwrap_or_else(|| {
                            eprintln!("fmt: option '--width' requires an argument");
                            process::exit(1);
                        })
                        .to_string_lossy()
                        .into_owned();
                    match val.parse::<usize>() {
                        Ok(w) => cli.width = w,
                        Err(_) => {
                            eprintln!("fmt: invalid width: '{}'", val);
                            process::exit(1);
                        }
                    }
                }
                b"--goal" => {
                    let val = args
                        .next()
                        .unwrap_or_else(|| {
                            eprintln!("fmt: option '--goal' requires an argument");
                            process::exit(1);
                        })
                        .to_string_lossy()
                        .into_owned();
                    match val.parse::<usize>() {
                        Ok(g) => cli.goal = Some(g),
                        Err(_) => {
                            eprintln!("fmt: invalid goal width: '{}'", val);
                            process::exit(1);
                        }
                    }
                }
                b"--prefix" => {
                    let val = args
                        .next()
                        .unwrap_or_else(|| {
                            eprintln!("fmt: option '--prefix' requires an argument");
                            process::exit(1);
                        })
                        .to_string_lossy()
                        .into_owned();
                    cli.prefix = Some(val);
                }
                b"--help" => {
                    print!(
                        "Usage: fmt [-WIDTH] [OPTION]... [FILE]...\n\
                         Reformat each paragraph in the FILE(s), writing to standard output.\n\
                         The option -WIDTH is an abbreviated form of --width=DIGITS.\n\n\
                         With no FILE, or when FILE is -, read standard input.\n\n\
                         Mandatory arguments to long options are mandatory for short options too.\n\
                         \x20 -c, --crown-margin        preserve indentation of first two lines\n\
                         \x20 -p, --prefix=STRING        reformat only lines beginning with STRING,\n\
                         \x20                            reattaching the prefix to reformatted lines\n\
                         \x20 -s, --split-only           split long lines, but do not refill\n\
                         \x20 -t, --tagged-paragraph     indentation of first line different from second\n\
                         \x20 -u, --uniform-spacing      one space between words, two after sentences\n\
                         \x20 -w, --width=WIDTH          maximum line width (default of 75 columns)\n\
                         \x20 -g, --goal=WIDTH           goal width (default of 93% of width)\n\
                         \x20     --help                 display this help and exit\n\
                         \x20     --version              output version information and exit\n"
                    );
                    process::exit(0);
                }
                b"--version" => {
                    println!("fmt (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                    process::exit(0);
                }
                _ => {
                    eprintln!("fmt: unrecognized option '{}'", arg.to_string_lossy());
                    eprintln!("Try 'fmt --help' for more information.");
                    process::exit(1);
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            // Short options.
            let mut i = 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'c' => cli.crown_margin = true,
                    b's' => cli.split_only = true,
                    b't' => cli.tagged = true,
                    b'u' => cli.uniform_spacing = true,
                    b'w' => {
                        if i + 1 < bytes.len() {
                            let val = arg.to_string_lossy();
                            match val[i + 1..].parse::<usize>() {
                                Ok(w) => cli.width = w,
                                Err(_) => {
                                    eprintln!("fmt: invalid width: '{}'", &val[i + 1..]);
                                    process::exit(1);
                                }
                            }
                        } else {
                            let val = args
                                .next()
                                .unwrap_or_else(|| {
                                    eprintln!("fmt: option requires an argument -- 'w'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned();
                            match val.parse::<usize>() {
                                Ok(w) => cli.width = w,
                                Err(_) => {
                                    eprintln!("fmt: invalid width: '{}'", val);
                                    process::exit(1);
                                }
                            }
                        }
                        break;
                    }
                    b'g' => {
                        if i + 1 < bytes.len() {
                            let val = arg.to_string_lossy();
                            match val[i + 1..].parse::<usize>() {
                                Ok(g) => cli.goal = Some(g),
                                Err(_) => {
                                    eprintln!("fmt: invalid goal width: '{}'", &val[i + 1..]);
                                    process::exit(1);
                                }
                            }
                        } else {
                            let val = args
                                .next()
                                .unwrap_or_else(|| {
                                    eprintln!("fmt: option requires an argument -- 'g'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned();
                            match val.parse::<usize>() {
                                Ok(g) => cli.goal = Some(g),
                                Err(_) => {
                                    eprintln!("fmt: invalid goal width: '{}'", val);
                                    process::exit(1);
                                }
                            }
                        }
                        break;
                    }
                    b'p' => {
                        if i + 1 < bytes.len() {
                            let val = arg.to_string_lossy();
                            cli.prefix = Some(val[i + 1..].to_string());
                        } else {
                            let val = args
                                .next()
                                .unwrap_or_else(|| {
                                    eprintln!("fmt: option requires an argument -- 'p'");
                                    process::exit(1);
                                })
                                .to_string_lossy()
                                .into_owned();
                            cli.prefix = Some(val);
                        }
                        break;
                    }
                    b'0'..=b'9' => {
                        // -WIDTH shorthand: -72 means --width=72.
                        let val = arg.to_string_lossy();
                        match val[i..].parse::<usize>() {
                            Ok(w) => cli.width = w,
                            Err(_) => {
                                eprintln!("fmt: invalid width: '{}'", &val[i..]);
                                process::exit(1);
                            }
                        }
                        break;
                    }
                    _ => {
                        eprintln!("fmt: invalid option -- '{}'", bytes[i] as char);
                        eprintln!("Try 'fmt --help' for more information.");
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

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let cli = parse_args();

    let goal = cli.goal.unwrap_or((cli.width * 93) / 100);

    let config = FmtConfig {
        width: cli.width,
        goal,
        split_only: cli.split_only,
        crown_margin: cli.crown_margin,
        tagged: cli.tagged,
        uniform_spacing: cli.uniform_spacing,
        prefix: cli.prefix,
    };

    let files: Vec<String> = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files
    };

    #[cfg(unix)]
    let stdout_raw = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
    #[cfg(unix)]
    let mut out = BufWriter::with_capacity(256 * 1024, &*stdout_raw);
    #[cfg(not(unix))]
    let stdout = io::stdout();
    #[cfg(not(unix))]
    let mut out = BufWriter::with_capacity(256 * 1024, stdout.lock());

    let mut had_error = false;

    for filename in &files {
        let result = if filename == "-" {
            let stdin = io::stdin();
            let reader = BufReader::new(stdin.lock());
            coreutils_rs::fmt::fmt_file(reader, &mut out, &config)
        } else {
            match File::open(filename) {
                Ok(f) => {
                    let reader = BufReader::new(f);
                    coreutils_rs::fmt::fmt_file(reader, &mut out, &config)
                }
                Err(e) => {
                    eprintln!("fmt: {}: {}", filename, io_error_msg(&e));
                    had_error = true;
                    continue;
                }
            }
        };

        if let Err(e) = result {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("fmt: write error: {}", io_error_msg(&e));
            had_error = true;
        }
    }

    if let Err(e) = out.flush()
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("fmt: write error: {}", io_error_msg(&e));
        had_error = true;
    }

    if had_error {
        process::exit(1);
    }
}
