use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

use coreutils_rs::common::io::{read_file, read_stdin};
use coreutils_rs::common::io_error_msg;
use coreutils_rs::expand::{TabStops, parse_tab_stops, unexpand_bytes};

struct Cli {
    all: bool,
    first_only: bool,
    tabs: TabStops,
    files: Vec<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        all: false,
        first_only: false,
        tabs: TabStops::Regular(8),
        files: Vec::new(),
    };

    let mut args = std::env::args_os().skip(1);
    let mut tab_spec: Option<String> = None;

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
            if bytes.starts_with(b"--tabs=") {
                let val = arg.to_string_lossy();
                tab_spec = Some(val[7..].to_string());
                // -t implies -a for unexpand
                cli.all = true;
                continue;
            }
            match bytes {
                b"--all" => cli.all = true,
                b"--first-only" => cli.first_only = true,
                b"--tabs" => {
                    tab_spec = Some(
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("unexpand: option '--tabs' requires an argument");
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned(),
                    );
                    // -t implies -a for unexpand
                    cli.all = true;
                }
                b"--help" => {
                    print!(
                        "Usage: unexpand [OPTION]... [FILE]...\n\
                         Convert blanks in each FILE to tabs, writing to standard output.\n\n\
                         With no FILE, or when FILE is -, read standard input.\n\n\
                         Mandatory arguments to long options are mandatory for short options too.\n\
                         \x20 -a, --all                  convert all blanks, instead of just initial blanks\n\
                         \x20     --first-only            convert only leading sequences of blanks (overrides -a)\n\
                         \x20 -t, --tabs=N               have tabs N characters apart, not 8\n\
                         \x20 -t, --tabs=LIST            use comma separated list of tab positions\n\
                         \x20     --help                 display this help and exit\n\
                         \x20     --version              output version information and exit\n"
                    );
                    process::exit(0);
                }
                b"--version" => {
                    println!("unexpand (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                    process::exit(0);
                }
                _ => {
                    eprintln!("unexpand: unrecognized option '{}'", arg.to_string_lossy());
                    eprintln!("Try 'unexpand --help' for more information.");
                    process::exit(1);
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            let mut i = 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'a' => cli.all = true,
                    b't' => {
                        if i + 1 < bytes.len() {
                            let val = arg.to_string_lossy();
                            tab_spec = Some(val[i + 1..].to_string());
                        } else {
                            tab_spec = Some(
                                args.next()
                                    .unwrap_or_else(|| {
                                        eprintln!("unexpand: option requires an argument -- 't'");
                                        process::exit(1);
                                    })
                                    .to_string_lossy()
                                    .into_owned(),
                            );
                        }
                        // -t implies -a for unexpand
                        cli.all = true;
                        break;
                    }
                    _ => {
                        if bytes[i].is_ascii_digit() {
                            let val = arg.to_string_lossy();
                            tab_spec = Some(val[i..].to_string());
                            break;
                        }
                        eprintln!("unexpand: invalid option -- '{}'", bytes[i] as char);
                        eprintln!("Try 'unexpand --help' for more information.");
                        process::exit(1);
                    }
                }
                i += 1;
            }
        } else {
            cli.files.push(arg.to_string_lossy().into_owned());
        }
    }

    if let Some(spec) = tab_spec {
        match parse_tab_stops(&spec) {
            Ok(tabs) => cli.tabs = tabs,
            Err(e) => {
                eprintln!("unexpand: {}", e);
                process::exit(1);
            }
        }
    }

    // --first-only overrides -a
    if cli.first_only {
        cli.all = false;
    }

    cli
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
    coreutils_rs::common::reset_sigpipe();

    #[cfg(target_os = "linux")]
    enlarge_pipes();

    let cli = parse_args();

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
        let data = if filename == "-" {
            match read_stdin() {
                Ok(d) => coreutils_rs::common::io::FileData::Owned(d),
                Err(e) => {
                    eprintln!("unexpand: standard input: {}", io_error_msg(&e));
                    had_error = true;
                    continue;
                }
            }
        } else {
            match read_file(Path::new(filename)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("unexpand: {}: {}", filename, io_error_msg(&e));
                    had_error = true;
                    continue;
                }
            }
        };

        if let Err(e) = unexpand_bytes(&data, &cli.tabs, cli.all, &mut out) {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("unexpand: write error: {}", io_error_msg(&e));
            had_error = true;
        }
    }

    if let Err(e) = out.flush()
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("unexpand: write error: {}", io_error_msg(&e));
        had_error = true;
    }

    if had_error {
        process::exit(1);
    }
}
