use std::io::{self, BufWriter, Write};
use std::mem::ManuallyDrop;
use std::process;

use coreutils_rs::cat::{self, CatConfig};
use coreutils_rs::common::{io_error_msg, reset_sigpipe};

struct Cli {
    config: CatConfig,
    files: Vec<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        config: CatConfig::default(),
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
            match bytes {
                b"--show-all" => {
                    cli.config.show_nonprinting = true;
                    cli.config.show_ends = true;
                    cli.config.show_tabs = true;
                }
                b"--number-nonblank" => {
                    cli.config.number_nonblank = true;
                }
                b"--show-ends" => {
                    cli.config.show_ends = true;
                }
                b"--number" => {
                    cli.config.number = true;
                }
                b"--squeeze-blank" => {
                    cli.config.squeeze_blank = true;
                }
                b"--show-tabs" => {
                    cli.config.show_tabs = true;
                }
                b"--show-nonprinting" => {
                    cli.config.show_nonprinting = true;
                }
                b"--help" => {
                    print_help();
                    process::exit(0);
                }
                b"--version" => {
                    println!("cat (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                    process::exit(0);
                }
                _ => {
                    let s = arg.to_string_lossy();
                    eprintln!("cat: unrecognized option '{}'", s);
                    eprintln!("Try 'cat --help' for more information.");
                    process::exit(1);
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            for &b in &bytes[1..] {
                match b {
                    b'A' => {
                        cli.config.show_nonprinting = true;
                        cli.config.show_ends = true;
                        cli.config.show_tabs = true;
                    }
                    b'b' => {
                        cli.config.number_nonblank = true;
                    }
                    b'e' => {
                        cli.config.show_nonprinting = true;
                        cli.config.show_ends = true;
                    }
                    b'E' => {
                        cli.config.show_ends = true;
                    }
                    b'n' => {
                        cli.config.number = true;
                    }
                    b's' => {
                        cli.config.squeeze_blank = true;
                    }
                    b't' => {
                        cli.config.show_nonprinting = true;
                        cli.config.show_tabs = true;
                    }
                    b'T' => {
                        cli.config.show_tabs = true;
                    }
                    b'v' => {
                        cli.config.show_nonprinting = true;
                    }
                    b'u' => {
                        // -u is ignored (POSIX requires it, GNU ignores it)
                    }
                    _ => {
                        eprintln!("cat: invalid option -- '{}'", b as char);
                        eprintln!("Try 'cat --help' for more information.");
                        process::exit(1);
                    }
                }
            }
        } else {
            cli.files.push(arg.to_string_lossy().into_owned());
        }
    }

    // -b overrides -n
    if cli.config.number_nonblank {
        cli.config.number = false;
    }

    cli
}

fn print_help() {
    print!(
        "Usage: cat [OPTION]... [FILE]...\n\
         Concatenate FILE(s) to standard output.\n\n\
         With no FILE, or when FILE is -, read standard input.\n\n\
         \x20 -A, --show-all           equivalent to -vET\n\
         \x20 -b, --number-nonblank    number nonempty output lines, overrides -n\n\
         \x20 -e                       equivalent to -vE\n\
         \x20 -E, --show-ends          display $ at end of each line\n\
         \x20 -n, --number             number all output lines\n\
         \x20 -s, --squeeze-blank      suppress repeated empty output lines\n\
         \x20 -t                       equivalent to -vT\n\
         \x20 -T, --show-tabs          display TAB characters as ^I\n\
         \x20 -u                       (ignored)\n\
         \x20 -v, --show-nonprinting   use ^ and M- notation, except for LFD and TAB\n\
         \x20     --help               display this help and exit\n\
         \x20     --version            output version information and exit\n"
    );
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
    reset_sigpipe();

    #[cfg(target_os = "linux")]
    enlarge_pipes();

    let cli = parse_args();

    let files: Vec<String> = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files
    };

    let tool_name = "cat";

    // For plain cat, use raw fd output to avoid BufWriter overhead
    if cli.config.is_plain() {
        #[cfg(unix)]
        {
            use std::os::unix::io::FromRawFd;
            let mut raw_out = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
            let mut had_error = false;
            for filename in &files {
                match cat::cat_file(filename, &cli.config, &mut 1u64, &mut *raw_out, tool_name) {
                    Ok(true) => {}
                    Ok(false) => had_error = true,
                    Err(e) => {
                        if e.kind() == io::ErrorKind::BrokenPipe {
                            process::exit(0);
                        }
                        eprintln!("{}: write error: {}", tool_name, io_error_msg(&e));
                        had_error = true;
                    }
                }
            }
            if had_error {
                process::exit(1);
            }
            return;
        }
        #[cfg(not(unix))]
        {
            let stdout = io::stdout();
            let mut out = stdout.lock();
            let mut had_error = false;
            for filename in &files {
                match cat::cat_file(filename, &cli.config, &mut 1u64, &mut out, tool_name) {
                    Ok(true) => {}
                    Ok(false) => had_error = true,
                    Err(e) => {
                        if e.kind() == io::ErrorKind::BrokenPipe {
                            process::exit(0);
                        }
                        eprintln!("{}: write error: {}", tool_name, io_error_msg(&e));
                        had_error = true;
                    }
                }
            }
            if had_error {
                process::exit(1);
            }
            return;
        }
    }

    // With options, use BufWriter
    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(256 * 1024, stdout.lock());
    let mut had_error = false;
    let mut line_num = 1u64;

    for filename in &files {
        match cat::cat_file(filename, &cli.config, &mut line_num, &mut out, tool_name) {
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

    if had_error {
        process::exit(1);
    }
}
