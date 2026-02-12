use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::process;

use clap::Parser;

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

    // mmap the stdin file descriptor
    // SAFETY: fd is valid, file is regular, size > 0
    use std::os::unix::io::FromRawFd;
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mmap: Option<memmap2::Mmap> = unsafe { memmap2::MmapOptions::new().map(&file) }.ok();
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

#[cfg(not(unix))]
fn try_mmap_stdin() -> Option<memmap2::Mmap> {
    None
}

fn main() {
    coreutils_rs::common::reset_sigpipe();
    let cli = Cli::parse();

    let set1_str = &cli.sets[0];

    // Try to mmap stdin for zero-copy reads
    let mmap = try_mmap_stdin();

    #[cfg(unix)]
    let mut raw = raw_stdout();

    // Pure translate mode: bypass BufWriter entirely.
    // translate() writes 1MB chunks directly, so no buffering needed.
    // This saves one full memcpy of the data through BufWriter's internal buffer.
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
        #[cfg(unix)]
        let result = if let Some(ref data) = mmap {
            tr::translate_mmap(&set1, &set2, data, &mut *raw)
        } else {
            let mut stdin = io::stdin().lock();
            tr::translate(&set1, &set2, &mut stdin, &mut *raw)
        };
        #[cfg(not(unix))]
        let result = {
            let stdout = io::stdout();
            let mut lock = stdout.lock();
            if let Some(ref data) = mmap {
                tr::translate_mmap(&set1, &set2, data, &mut lock)
            } else {
                let mut stdin = io::stdin().lock();
                tr::translate(&set1, &set2, &mut stdin, &mut lock)
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

    // === Mmap fast paths: bypass BufWriter entirely ===
    // All _mmap functions do internal 1MB buffering (or zero-copy IoSlice),
    // so BufWriter just adds an extra memcpy. Writing directly to fd is faster.
    if let Some(ref data) = mmap {
        #[cfg(unix)]
        let result = run_mmap_mode(&cli, set1_str, data, &mut *raw);
        #[cfg(not(unix))]
        let result = {
            let stdout = io::stdout();
            let mut lock = stdout.lock();
            run_mmap_mode(&cli, set1_str, data, &mut lock)
        };
        if let Err(e) = result
            && e.kind() != io::ErrorKind::BrokenPipe
        {
            eprintln!("tr: {}", io_error_msg(&e));
            process::exit(1);
        }
        return;
    }

    // === Streaming paths (stdin pipe): use BufWriter for batching ===
    #[cfg(unix)]
    let mut writer = BufWriter::with_capacity(1024 * 1024, &mut *raw);
    #[cfg(not(unix))]
    let stdout = io::stdout();
    #[cfg(not(unix))]
    let mut writer = BufWriter::with_capacity(1024 * 1024, stdout.lock());

    let result = run_streaming_mode(&cli, set1_str, &mut writer);

    // Flush buffered output
    let _ = writer.flush();

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

/// Dispatch streaming modes — uses BufWriter for batching small writes.
fn run_streaming_mode(cli: &Cli, set1_str: &str, writer: &mut impl Write) -> io::Result<()> {
    let mut stdin = io::stdin().lock();

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
        tr::delete_squeeze(&delete_set, &set2, &mut stdin, writer)
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
        tr::delete(&delete_set, &mut stdin, writer)
    } else if cli.squeeze && cli.sets.len() < 2 {
        let set1 = tr::parse_set(set1_str);
        let squeeze_set = if cli.complement {
            tr::complement(&set1)
        } else {
            set1
        };
        tr::squeeze(&squeeze_set, &mut stdin, writer)
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
        tr::translate_squeeze(&set1, &set2, &mut stdin, writer)
    } else {
        eprintln!("tr: missing operand after '{}'", set1_str);
        eprintln!("Two strings must be given when translating.");
        eprintln!("Try 'tr --help' for more information.");
        process::exit(1);
    }
}
