use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

use coreutils_rs::common::io::{read_file, read_stdin};
use coreutils_rs::common::io_error_msg;
use coreutils_rs::rev;

struct Cli {
    files: Vec<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli { files: Vec::new() };

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
            match bytes {
                b"--help" => {
                    print!(
                        "Usage: rev [OPTION]... [FILE]...\n\
                         Reverse lines characterwise.\n\n\
                         With no FILE, or when FILE is -, read standard input.\n\n\
                         \x20     --help     display this help and exit\n\
                         \x20     --version  output version information and exit\n"
                    );
                    process::exit(0);
                }
                b"--version" => {
                    println!("rev (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                    process::exit(0);
                }
                _ => {
                    eprintln!("rev: unrecognized option '{}'", arg.to_string_lossy());
                    process::exit(1);
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            // rev doesn't have short options, treat as file
            cli.files.push(arg.to_string_lossy().into_owned());
        } else {
            cli.files.push(arg.to_string_lossy().into_owned());
        }
    }

    cli
}

/// Enlarge pipe buffers on Linux for higher throughput.
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

    // Use BufWriter for output
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
                    eprintln!("rev: standard input: {}", io_error_msg(&e));
                    had_error = true;
                    continue;
                }
            }
        } else {
            match read_file(Path::new(filename)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("rev: {}: {}", filename, io_error_msg(&e));
                    had_error = true;
                    continue;
                }
            }
        };

        if let Err(e) = rev::rev_bytes(&data, &mut out) {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("rev: write error: {}", io_error_msg(&e));
            had_error = true;
        }
    }

    if let Err(e) = out.flush()
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("rev: write error: {}", io_error_msg(&e));
        had_error = true;
    }

    if had_error {
        process::exit(1);
    }
}
