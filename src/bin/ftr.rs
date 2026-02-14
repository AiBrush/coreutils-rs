use std::io::{self, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::process;

use clap::Parser;

use coreutils_rs::common::io::{FileData, read_stdin};
use coreutils_rs::common::io_error_msg;
use coreutils_rs::tr;

#[derive(Parser)]
#[command(
    name = "tr",
    about = "Translate, squeeze, and/or delete characters",
    override_usage = "tr [OPTION]... SET1 [SET2]"
)]
struct Cli {
    /// Use the complement of SET1
    #[arg(short = 'c', short_alias = 'C', long = "complement")]
    complement: bool,

    /// Delete characters in SET1, do not translate
    #[arg(short = 'd', long = "delete")]
    delete: bool,

    /// Replace each sequence of a repeated character that is listed
    /// in the last specified SET, with a single occurrence of that character
    #[arg(short = 's', long = "squeeze-repeats")]
    squeeze: bool,

    /// First truncate SET1 to length of SET2
    #[arg(short = 't', long = "truncate-set1")]
    truncate: bool,

    /// Character sets
    #[arg(required = true)]
    sets: Vec<String>,
}

/// Raw fd stdout for zero-overhead writes on Unix.
#[cfg(unix)]
#[inline]
fn raw_stdout() -> ManuallyDrop<std::fs::File> {
    unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) }
}

/// Try to mmap stdin if it's a regular file (e.g., shell redirect `< file`).
/// Returns None if stdin is a pipe/terminal, or on non-unix platforms.
#[cfg(unix)]
fn try_mmap_stdin() -> Option<memmap2::Mmap> {
    use std::os::unix::io::AsRawFd;
    let stdin = io::stdin();
    let fd = stdin.as_raw_fd();

    // Check if stdin is a regular file via fstat
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return None;
    }
    if (stat.st_mode & libc::S_IFMT) != libc::S_IFREG || stat.st_size <= 0 {
        return None;
    }

    // mmap the stdin file descriptor with MAP_POPULATE for pre-faulted pages
    // SAFETY: fd is valid, file is regular, size > 0
    use std::os::unix::io::FromRawFd;
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mmap: Option<memmap2::Mmap> =
        unsafe { memmap2::MmapOptions::new().populate().map(&file) }.ok();
    std::mem::forget(file); // Don't close stdin
    #[cfg(target_os = "linux")]
    if let Some(ref m) = mmap {
        unsafe {
            libc::madvise(
                m.as_ptr() as *mut libc::c_void,
                m.len(),
                libc::MADV_SEQUENTIAL | libc::MADV_WILLNEED,
            );
            if m.len() >= 2 * 1024 * 1024 {
                libc::madvise(
                    m.as_ptr() as *mut libc::c_void,
                    m.len(),
                    libc::MADV_HUGEPAGE,
                );
            }
        }
    }
    mmap
}

#[cfg(not(unix))]
fn try_mmap_stdin() -> Option<memmap2::Mmap> {
    None
}

/// Enlarge pipe buffers on Linux for higher throughput.
/// Default pipe buffer is 64KB; increasing to 8MB reduces syscalls
/// when reading/writing through pipes (e.g., `cat file | ftr`).
/// 8MB allows the 8MB stream buffer to be filled/written in one syscall.
#[cfg(target_os = "linux")]
fn enlarge_pipe_bufs() {
    const PIPE_SIZE: i32 = 8 * 1024 * 1024;
    unsafe {
        libc::fcntl(0, libc::F_SETPIPE_SZ, PIPE_SIZE); // stdin
        libc::fcntl(1, libc::F_SETPIPE_SZ, PIPE_SIZE); // stdout
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    #[cfg(target_os = "linux")]
    enlarge_pipe_bufs();

    let cli = Cli::parse();

    let set1_str = &cli.sets[0];

    // Load ALL stdin data: try mmap first (zero-copy for file redirects),
    // fall back to read_stdin() for pipes (raw libc::read, 64MB pre-alloc).
    // This unified approach means all modes (translate, delete, squeeze) use
    // the optimized _mmap batch paths with parallel processing + single write.
    let data: FileData = match try_mmap_stdin() {
        Some(m) => FileData::Mmap(m),
        None => match read_stdin() {
            Ok(d) => FileData::Owned(d),
            Err(e) => {
                eprintln!("tr: {}", io_error_msg(&e));
                process::exit(1);
            }
        },
    };

    #[cfg(unix)]
    let mut raw = raw_stdout();

    // Pure translate mode: bypass BufWriter entirely.
    let is_pure_translate = !cli.delete && !cli.squeeze && cli.sets.len() >= 2;

    if is_pure_translate {
        let set2_str = &cli.sets[1];
        let mut set1 = tr::parse_set(set1_str);
        if cli.complement {
            set1 = tr::complement(&set1);
        }
        let set2 = if cli.truncate {
            let raw_set = tr::parse_set(set2_str);
            set1.truncate(raw_set.len());
            raw_set
        } else {
            tr::expand_set2(set2_str, set1.len())
        };
        // Use in-place translate for owned data (piped stdin) to avoid output buffer.
        // For mmap'd data (file redirect), use the mmap path that allocates an output buffer.
        let result = match data {
            FileData::Owned(mut vec) => {
                #[cfg(unix)]
                {
                    tr::translate_owned(&set1, &set2, &mut vec, &mut *raw)
                }
                #[cfg(not(unix))]
                {
                    let stdout = io::stdout();
                    let mut lock = stdout.lock();
                    tr::translate_owned(&set1, &set2, &mut vec, &mut lock)
                }
            }
            _ => {
                #[cfg(unix)]
                {
                    tr::translate_mmap(&set1, &set2, &data, &mut *raw)
                }
                #[cfg(not(unix))]
                {
                    let stdout = io::stdout();
                    let mut lock = stdout.lock();
                    tr::translate_mmap(&set1, &set2, &data, &mut lock)
                }
            }
        };
        if let Err(e) = result
            && e.kind() != io::ErrorKind::BrokenPipe
        {
            eprintln!("tr: {}", io_error_msg(&e));
            process::exit(1);
        }
        return;
    }

    // All modes use _mmap batch paths: parallel processing + single write_all.
    #[cfg(unix)]
    let result = run_mmap_mode(&cli, set1_str, &data, &mut *raw);
    #[cfg(not(unix))]
    let result = {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        run_mmap_mode(&cli, set1_str, &data, &mut lock)
    };

    if let Err(e) = result
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("tr: {}", io_error_msg(&e));
        process::exit(1);
    }
}

/// Dispatch mmap-based modes — writes directly to raw fd for zero-copy.
fn run_mmap_mode(
    cli: &Cli,
    set1_str: &str,
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    if cli.delete && cli.squeeze {
        if cli.sets.len() < 2 {
            eprintln!("tr: missing operand after '{}'", set1_str);
            eprintln!("Two strings must be given when both deleting and squeezing repeats.");
            eprintln!("Try 'tr --help' for more information.");
            process::exit(1);
        }
        let set2_str = &cli.sets[1];
        let set1 = tr::parse_set(set1_str);
        let set2 = tr::parse_set(set2_str);
        let delete_set = if cli.complement {
            tr::complement(&set1)
        } else {
            set1
        };
        tr::delete_squeeze_mmap(&delete_set, &set2, data, writer)
    } else if cli.delete {
        if cli.sets.len() > 1 {
            eprintln!("tr: extra operand '{}'", cli.sets[1]);
            eprintln!("Only one string may be given when deleting without squeezing.");
            eprintln!("Try 'tr --help' for more information.");
            process::exit(1);
        }
        let set1 = tr::parse_set(set1_str);
        let delete_set = if cli.complement {
            tr::complement(&set1)
        } else {
            set1
        };
        tr::delete_mmap(&delete_set, data, writer)
    } else if cli.squeeze && cli.sets.len() < 2 {
        let set1 = tr::parse_set(set1_str);
        let squeeze_set = if cli.complement {
            tr::complement(&set1)
        } else {
            set1
        };
        tr::squeeze_mmap(&squeeze_set, data, writer)
    } else if cli.squeeze {
        let set2_str = &cli.sets[1];
        let mut set1 = tr::parse_set(set1_str);
        if cli.complement {
            set1 = tr::complement(&set1);
        }
        let set2 = if cli.truncate {
            let raw_set = tr::parse_set(set2_str);
            set1.truncate(raw_set.len());
            raw_set
        } else {
            tr::expand_set2(set2_str, set1.len())
        };
        tr::translate_squeeze_mmap(&set1, &set2, data, writer)
    } else {
        eprintln!("tr: missing operand after '{}'", set1_str);
        eprintln!("Two strings must be given when translating.");
        eprintln!("Try 'tr --help' for more information.");
        process::exit(1);
    }
}
