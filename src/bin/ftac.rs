use std::io::{self, BufWriter, Write};
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
use coreutils_rs::common::io_error_msg;
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
    let mmap = unsafe { MmapOptions::new().populate().map(&file) }.ok();
    std::mem::forget(file); // Don't close stdin
    #[cfg(target_os = "linux")]
    if let Some(ref m) = mmap {
        unsafe {
            let ptr = m.as_ptr() as *mut libc::c_void;
            let len = m.len();
            // WILLNEED pre-faults all pages. Don't use SEQUENTIAL since tac
            // accesses data in reverse order during the output phase.
            libc::madvise(ptr, len, libc::MADV_WILLNEED);
            if len >= 2 * 1024 * 1024 {
                libc::madvise(ptr, len, libc::MADV_HUGEPAGE);
            }
        }
    }
    mmap
}

fn run(cli: &Cli, files: &[String], out: &mut impl Write) -> bool {
    let mut had_error = false;

    for filename in files {
        let mut data: FileData = if filename == "-" {
            #[cfg(unix)]
            {
                match try_mmap_stdin() {
                    Some(mmap) => FileData::Mmap(mmap),
                    None => match read_stdin() {
                        Ok(d) => FileData::Owned(d),
                        Err(e) => {
                            eprintln!("tac: standard input: {}", io_error_msg(&e));
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
                    eprintln!("tac: standard input: {}", io_error_msg(&e));
                    had_error = true;
                    continue;
                }
            }
        } else {
            match read_file(Path::new(filename)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("tac: {}: {}", filename, io_error_msg(&e));
                    had_error = true;
                    continue;
                }
            }
        };

        // Override MADV_SEQUENTIAL from read_file: tac needs reverse access
        // after the forward scan. SEQUENTIAL tells the kernel to drop pages
        // behind the read pointer, which would evict pages needed during the
        // reverse copy phase. MADV_RANDOM keeps all pages resident.
        #[cfg(target_os = "linux")]
        {
            let ptr = data.as_ptr() as *mut libc::c_void;
            let len = data.len();
            if len > 0 {
                unsafe {
                    libc::madvise(ptr, len, libc::MADV_RANDOM);
                }
            }
        }

        let result = if cli.regex {
            let bytes: &[u8] = &data;
            let sep = cli.separator.as_deref().unwrap_or("\n");
            tac::tac_regex_separator(bytes, sep, cli.before, out)
        } else if let Some(ref sep) = cli.separator {
            let bytes: &[u8] = &data;
            tac::tac_string_separator(bytes, sep.as_bytes(), cli.before, out)
        } else if let FileData::Owned(ref mut owned) = data {
            // In-place reversal: no output buffer needed.
            tac::tac_bytes_owned(owned, b'\n', cli.before, out)
        } else {
            let bytes: &[u8] = &data;
            tac::tac_bytes(bytes, b'\n', cli.before, out)
        };

        if let Err(e) = result {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("tac: write error: {}", io_error_msg(&e));
            had_error = true;
        }
    }

    had_error
}

/// Enlarge pipe buffers on Linux for higher throughput.
/// Reads system max from /proc, falls back through decreasing sizes.
#[cfg(target_os = "linux")]
fn enlarge_pipes() {
    let max_size = std::fs::read_to_string("/proc/sys/fs/pipe-max-size")
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok());
    for &fd in &[0i32, 1] {
        if let Some(max) = max_size
            && unsafe { libc::fcntl(fd, libc::F_SETPIPE_SZ, max) } > 0
        {
            continue;
        }
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

    let cli = Cli::parse();

    let files: Vec<String> = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
    };

    // For byte separator (default): use raw stdout for zero-copy writev.
    // The core tac functions batch IoSlices and call write_vectored,
    // which maps to system writev on Unix — zero-copy from mmap pages.
    // For regex/string separator: use BufWriter since those paths
    // use many small write_all calls that benefit from buffering.
    #[cfg(unix)]
    let had_error = {
        let is_byte_sep = !cli.regex && cli.separator.is_none();
        let raw = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
        if is_byte_sep {
            run(&cli, &files, &mut &*raw)
        } else {
            let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, &*raw);
            let err = run(&cli, &files, &mut writer);
            let _ = writer.flush();
            err
        }
    };
    #[cfg(not(unix))]
    let had_error = {
        let stdout = io::stdout();
        let lock = stdout.lock();
        let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, lock);
        let err = run(&cli, &files, &mut writer);
        let _ = writer.flush();
        err
    };

    if had_error {
        process::exit(1);
    }
}
