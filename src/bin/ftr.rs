use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::process;

use clap::Parser;

use coreutils_rs::tr;

#[derive(Parser)]
#[command(
    name = "ftr",
    about = "Translate, squeeze, and/or delete characters",
    override_usage = "ftr [OPTION]... SET1 [SET2]"
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
    let mmap: Option<memmap2::Mmap> = unsafe { memmap2::Mmap::map(&file) }.ok();
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
    let cli = Cli::parse();

    let set1_str = &cli.sets[0];

    // Raw fd stdout on Unix with 64KB BufWriter for batching.
    // Raw fd bypasses StdoutLock overhead; small BufWriter avoids double-buffering
    // since our streaming functions already process and write 256KB chunks.
    #[cfg(unix)]
    let mut raw = raw_stdout();
    #[cfg(unix)]
    let mut writer = BufWriter::with_capacity(64 * 1024, &mut *raw);
    #[cfg(not(unix))]
    let stdout = io::stdout();
    #[cfg(not(unix))]
    let mut writer = BufWriter::with_capacity(64 * 1024, stdout.lock());

    // Try to mmap stdin for zero-copy reads
    let mmap = try_mmap_stdin();

    let result = if cli.delete && cli.squeeze {
        // -d -s: delete SET1 chars, then squeeze SET2 chars
        if cli.sets.len() < 2 {
            eprintln!("ftr: missing operand after '{}'", set1_str);
            eprintln!("Two strings must be given when both deleting and squeezing repeats.");
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
        if let Some(ref data) = mmap {
            tr::delete_squeeze_mmap(&delete_set, &set2, data, &mut writer)
        } else {
            let mut stdin = io::stdin().lock();
            tr::delete_squeeze(&delete_set, &set2, &mut stdin, &mut writer)
        }
    } else if cli.delete {
        // -d only: delete SET1 chars
        if cli.sets.len() > 1 {
            eprintln!("ftr: extra operand '{}'", cli.sets[1]);
            eprintln!("Only one string may be given when deleting without squeezing.");
            process::exit(1);
        }
        let set1 = tr::parse_set(set1_str);
        let delete_set = if cli.complement {
            tr::complement(&set1)
        } else {
            set1
        };
        if let Some(ref data) = mmap {
            tr::delete_mmap(&delete_set, data, &mut writer)
        } else {
            let mut stdin = io::stdin().lock();
            tr::delete(&delete_set, &mut stdin, &mut writer)
        }
    } else if cli.squeeze && cli.sets.len() < 2 {
        // -s only with one set: squeeze SET1 chars
        let set1 = tr::parse_set(set1_str);
        let squeeze_set = if cli.complement {
            tr::complement(&set1)
        } else {
            set1
        };
        if let Some(ref data) = mmap {
            tr::squeeze_mmap(&squeeze_set, data, &mut writer)
        } else {
            let mut stdin = io::stdin().lock();
            tr::squeeze(&squeeze_set, &mut stdin, &mut writer)
        }
    } else if cli.squeeze {
        // -s with two sets: translate SET1->SET2, then squeeze SET2 chars
        let set2_str = &cli.sets[1];
        let mut set1 = tr::parse_set(set1_str);
        if cli.complement {
            set1 = tr::complement(&set1);
        }
        let set2 = if cli.truncate {
            let raw = tr::parse_set(set2_str);
            set1.truncate(raw.len());
            raw
        } else {
            tr::expand_set2(set2_str, set1.len())
        };
        if let Some(ref data) = mmap {
            tr::translate_squeeze_mmap(&set1, &set2, data, &mut writer)
        } else {
            let mut stdin = io::stdin().lock();
            tr::translate_squeeze(&set1, &set2, &mut stdin, &mut writer)
        }
    } else {
        // Default: translate SET1 -> SET2
        if cli.sets.len() < 2 {
            eprintln!("ftr: missing operand after '{}'", set1_str);
            eprintln!("Two strings must be given when translating.");
            process::exit(1);
        }
        let set2_str = &cli.sets[1];
        let mut set1 = tr::parse_set(set1_str);
        if cli.complement {
            set1 = tr::complement(&set1);
        }
        let set2 = if cli.truncate {
            let raw = tr::parse_set(set2_str);
            set1.truncate(raw.len());
            raw
        } else {
            tr::expand_set2(set2_str, set1.len())
        };
        if let Some(ref data) = mmap {
            tr::translate_mmap(&set1, &set2, data, &mut writer)
        } else {
            let mut stdin = io::stdin().lock();
            tr::translate(&set1, &set2, &mut stdin, &mut writer)
        }
    };

    // Flush buffered output
    let _ = writer.flush();

    if let Err(e) = result
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("ftr: {}", e);
        process::exit(1);
    }
}
