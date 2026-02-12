use std::io::{self, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

use clap::Parser;
#[cfg(unix)]
use memmap2::MmapOptions;

use coreutils_rs::common::io::{FileData, read_file, read_stdin};
use coreutils_rs::tac;

#[derive(Parser)]
#[command(
    name = "tac",
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

/// Try to mmap stdin if it's a regular file (e.g., shell redirect `< file`).
/// Returns None if stdin is a pipe/terminal.
#[cfg(unix)]
fn try_mmap_stdin() -> Option<memmap2::Mmap> {
    use std::os::unix::io::{AsRawFd, FromRawFd};
    let stdin = io::stdin();
    let fd = stdin.as_raw_fd();

    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return None;
    }
    if (stat.st_mode & libc::S_IFMT) != libc::S_IFREG || stat.st_size <= 0 {
        return None;
    }

    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mmap = unsafe { MmapOptions::new().map(&file) }.ok();
    std::mem::forget(file); // Don't close stdin
    #[cfg(target_os = "linux")]
    if let Some(ref m) = mmap {
        unsafe {
            libc::madvise(
                m.as_ptr() as *mut libc::c_void,
                m.len(),
                libc::MADV_SEQUENTIAL,
            );
        }
    }
    mmap
}

fn run(cli: &Cli, files: &[String], out: &mut impl Write) -> bool {
    let mut had_error = false;

    for filename in files {
        let data: FileData = if filename == "-" {
            #[cfg(unix)]
            {
                match try_mmap_stdin() {
                    Some(mmap) => FileData::Mmap(mmap),
                    None => match read_stdin() {
                        Ok(d) => FileData::Owned(d),
                        Err(e) => {
                            eprintln!("ftac: standard input: {}", e);
                            had_error = true;
                            continue;
                        }
                    },
                }
            }
            #[cfg(not(unix))]
            match read_stdin() {
                Ok(d) => FileData::Owned(d),
                Err(e) => {
                    eprintln!("tac: standard input: {}", e);
                    had_error = true;
                    continue;
                }
            }
        } else {
            match read_file(Path::new(filename)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("tac: {}: {}", filename, e);
                    had_error = true;
                    continue;
                }
            }
        };

        // tac uses backward SIMD scan (memrchr_iter) for single-byte separator,
        // so MADV_RANDOM is optimal (no sequential readahead benefit).
        // WILLNEED still helps pre-fault all pages, HUGEPAGE reduces TLB misses.
        #[cfg(unix)]
        {
            if let FileData::Mmap(ref mmap) = data {
                unsafe {
                    // Pre-fault all pages for backward scan
                    libc::madvise(
                        mmap.as_ptr() as *mut libc::c_void,
                        mmap.len(),
                        libc::MADV_WILLNEED,
                    );
                    // HUGEPAGE reduces TLB misses for large files (Linux only)
                    #[cfg(target_os = "linux")]
                    if mmap.len() >= 2 * 1024 * 1024 {
                        libc::madvise(
                            mmap.as_ptr() as *mut libc::c_void,
                            mmap.len(),
                            libc::MADV_HUGEPAGE,
                        );
                    }
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
            eprintln!("tac: write error: {}", e);
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

    // Raw fd stdout — tac's batched writev sends IoSlice directly to kernel.
    // BufWriter would copy mmap data into its buffer, defeating zero-copy.
    #[cfg(unix)]
    let had_error = {
        let mut raw = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
        run(&cli, &files, &mut *raw)
    };
    #[cfg(not(unix))]
    let had_error = {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        run(&cli, &files, &mut lock)
    };

    if had_error {
        process::exit(1);
    }
}
