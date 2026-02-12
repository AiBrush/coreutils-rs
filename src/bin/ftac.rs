use std::io::{self, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

use clap::Parser;

use coreutils_rs::common::io::{FileData, read_file, read_stdin};
use coreutils_rs::tac;

#[derive(Parser)]
#[command(
    name = "ftac",
    about = "Concatenate and print files in reverse",
    version
)]
struct Cli {
    /// Attach the separator before instead of after
    #[arg(short = 'b', long = "before")]
    before: bool,

    /// Interpret the separator as a regular expression
    #[arg(short = 'r', long = "regex")]
    regex: bool,

    /// Use STRING as the separator instead of newline
    #[arg(
        short = 's',
        long = "separator",
        value_name = "STRING",
        allow_hyphen_values = true
    )]
    separator: Option<String>,

    /// Files to process (reads stdin if none given)
    files: Vec<String>,
}

fn run(cli: &Cli, files: &[String], out: &mut impl Write) -> bool {
    let mut had_error = false;

    for filename in files {
        let data: FileData = if filename == "-" {
            match read_stdin() {
                Ok(d) => FileData::Owned(d),
                Err(e) => {
                    eprintln!("ftac: standard input: {}", e);
                    had_error = true;
                    continue;
                }
            }
        } else {
            match read_file(Path::new(filename)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("ftac: {}: {}", filename, e);
                    had_error = true;
                    continue;
                }
            }
        };

        // tac reads backward — use MADV_RANDOM instead of SEQUENTIAL
        #[cfg(unix)]
        {
            if let FileData::Mmap(ref mmap) = data {
                unsafe {
                    libc::madvise(
                        mmap.as_ptr() as *mut libc::c_void,
                        mmap.len(),
                        libc::MADV_RANDOM,
                    );
                }
            }
        }

        let bytes: &[u8] = &data;

        let result = if cli.regex {
            let sep = cli.separator.as_deref().unwrap_or("\n");
            tac::tac_regex_separator(bytes, sep, cli.before, out)
        } else if let Some(ref sep) = cli.separator {
            tac::tac_string_separator(bytes, sep.as_bytes(), cli.before, out)
        } else {
            tac::tac_bytes(bytes, b'\n', cli.before, out)
        };

        if let Err(e) = result {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("ftac: write error: {}", e);
            had_error = true;
        }
    }

    had_error
}

fn main() {
    let cli = Cli::parse();

    let files: Vec<String> = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
    };

    // Raw fd stdout on Unix — tac core handles buffering internally
    #[cfg(unix)]
    let had_error = {
        let mut raw = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
        run(&cli, &files, &mut *raw)
    };
    #[cfg(not(unix))]
    let had_error = {
        let stdout = io::stdout();
        let mut locked = stdout.lock();
        run(&cli, &files, &mut locked)
    };

    if had_error {
        process::exit(1);
    }
}
