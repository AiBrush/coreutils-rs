use std::io::{self, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::process;

use clap::Parser;

use coreutils_rs::common::io::FileData;
use coreutils_rs::common::io_error_msg;
use coreutils_rs::tr;

/// Raw stdin reader for zero-overhead pipe reads on Linux.
/// Bypasses Rust's StdinLock (mutex + 8KB BufReader) to use libc::read(0) directly.
/// Each read returns immediately with whatever data is available in the pipe,
/// enabling pipelining with upstream cat: ftr processes chunk N while cat writes N+1.
#[cfg(target_os = "linux")]
struct RawStdin;

#[cfg(target_os = "linux")]
impl io::Read for RawStdin {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let ret = unsafe { libc::read(0, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if ret >= 0 {
                return Ok(ret as usize);
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }
}

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

/// Try to create a MAP_PRIVATE (copy-on-write) mmap of stdin for in-place translate.
/// MAP_PRIVATE means writes only affect our process's copy — the underlying file
/// is unmodified. The kernel uses COW: only pages we actually modify get physically
/// copied, so for sparse translations (e.g., `tr 'aeiou' 'AEIOU'` where only ~40%
/// of bytes change), this is significantly cheaper than allocating a full copy.
#[cfg(unix)]
fn try_mmap_stdin_mut() -> Option<memmap2::MmapMut> {
    use std::os::unix::io::AsRawFd;
    let stdin = io::stdin();
    let fd = stdin.as_raw_fd();

    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return None;
    }
    if (stat.st_mode & libc::S_IFMT) != libc::S_IFREG || stat.st_size <= 0 {
        return None;
    }

    use std::os::unix::io::FromRawFd;
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    // map_copy creates MAP_PRIVATE mapping — writes are COW, file untouched
    let mmap = unsafe { memmap2::MmapOptions::new().populate().map_copy(&file) }.ok();
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
fn try_mmap_stdin_mut() -> Option<memmap2::MmapMut> {
    None
}

/// Enlarge pipe buffers on Linux to the maximum allowed size.
/// Reads /proc/sys/fs/pipe-max-size for the system limit, then falls back
/// through decreasing sizes. Larger pipe buffers dramatically reduce
/// read/write syscall count for piped input (64KB default → 10x more syscalls).
#[cfg(target_os = "linux")]
fn enlarge_pipe_bufs() {
    // Try to read the system max pipe size
    let max_size = std::fs::read_to_string("/proc/sys/fs/pipe-max-size")
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok());
    for &fd in &[0i32, 1] {
        if let Some(max) = max_size
            && unsafe { libc::fcntl(fd, libc::F_SETPIPE_SZ, max) } > 0
        {
            continue;
        }
        // Fallback: try decreasing sizes when /proc read fails or max is denied
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
    enlarge_pipe_bufs();

    let cli = Cli::parse();

    let set1_str = &cli.sets[0];

    #[cfg(unix)]
    let mut raw = raw_stdout();

    // Pure translate mode: bypass BufWriter entirely.
    // For mmap path, use MAP_PRIVATE (COW) mmap and translate in-place to
    // eliminate the full output buffer allocation. The kernel only copies
    // pages that are actually modified.
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

        // For small-to-medium files, use read-only mmap + separate output buffer.
        // This avoids MAP_PRIVATE COW page faults entirely. For large files (>= 32MB),
        // MAP_PRIVATE is better because it avoids doubling memory usage.
        let result = if let Some(mm) = try_mmap_stdin() {
            if mm.len() < 64 * 1024 * 1024 {
                // Read-only mmap: translate into separate buffer (no COW faults)
                #[cfg(unix)]
                {
                    tr::translate_mmap_readonly(&set1, &set2, &mm, &mut *raw)
                }
                #[cfg(not(unix))]
                {
                    let stdout = io::stdout();
                    let mut lock = stdout.lock();
                    tr::translate_mmap_readonly(&set1, &set2, &mm, &mut lock)
                }
            } else {
                // Large file: use MAP_PRIVATE for in-place translate
                drop(mm);
                if let Some(mut mm_mut) = try_mmap_stdin_mut() {
                    #[cfg(unix)]
                    {
                        tr::translate_mmap_inplace(&set1, &set2, &mut mm_mut, &mut *raw)
                    }
                    #[cfg(not(unix))]
                    {
                        let stdout = io::stdout();
                        let mut lock = stdout.lock();
                        tr::translate_mmap_inplace(&set1, &set2, &mut mm_mut, &mut lock)
                    }
                } else {
                    // Fallback: streaming path with raw stdin
                    #[cfg(target_os = "linux")]
                    {
                        tr::translate(&set1, &set2, &mut RawStdin, &mut *raw)
                    }
                    #[cfg(all(unix, not(target_os = "linux")))]
                    {
                        let stdin = io::stdin();
                        let mut reader = stdin.lock();
                        tr::translate(&set1, &set2, &mut reader, &mut *raw)
                    }
                    #[cfg(not(unix))]
                    {
                        let stdin = io::stdin();
                        let mut reader = stdin.lock();
                        let stdout = io::stdout();
                        let mut lock = stdout.lock();
                        tr::translate(&set1, &set2, &mut reader, &mut lock)
                    }
                }
            }
        } else {
            // Piped stdin: use streaming path with RawStdin for pipelining.
            // Streaming processes each chunk immediately after read, enabling
            // overlap with upstream cat. RawStdin bypasses StdinLock overhead.
            #[cfg(target_os = "linux")]
            {
                tr::translate(&set1, &set2, &mut RawStdin, &mut *raw)
            }
            #[cfg(all(unix, not(target_os = "linux")))]
            {
                let stdin = io::stdin();
                let mut reader = stdin.lock();
                tr::translate(&set1, &set2, &mut reader, &mut *raw)
            }
            #[cfg(not(unix))]
            {
                let stdin = io::stdin();
                let mut reader = stdin.lock();
                let stdout = io::stdout();
                let mut lock = stdout.lock();
                tr::translate(&set1, &set2, &mut reader, &mut lock)
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

    // Try read-only mmap for non-translate modes (delete, squeeze, etc.)
    let mmap = try_mmap_stdin();

    if let Some(m) = mmap {
        // File-redirected stdin: use batch path with mmap data
        let data = FileData::Mmap(m);
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
    } else {
        // Piped stdin: use streaming functions directly for pipelining.
        // Streaming processes each chunk as it arrives from the pipe,
        // overlapping with upstream cat's writes. Avoids the 16MB read_stdin
        // pre-allocation and eliminates waiting for EOF before processing.
        #[cfg(unix)]
        let result = run_streaming_mode(&cli, set1_str, &mut *raw);
        #[cfg(not(unix))]
        let result = {
            let stdout = io::stdout();
            let mut lock = stdout.lock();
            run_streaming_mode(&cli, set1_str, &mut lock)
        };
        if let Err(e) = result
            && e.kind() != io::ErrorKind::BrokenPipe
        {
            eprintln!("tr: {}", io_error_msg(&e));
            process::exit(1);
        }
    }
}

/// Dispatch streaming modes for piped stdin — reads and processes chunks
/// incrementally for pipelining with upstream cat/pipe. RawStdin on Linux
/// bypasses StdinLock overhead; streaming processes each chunk as it arrives
/// instead of waiting for EOF like the batch path.
fn run_streaming_mode(cli: &Cli, set1_str: &str, writer: &mut impl Write) -> io::Result<()> {
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
        #[cfg(target_os = "linux")]
        return tr::delete_squeeze(&delete_set, &set2, &mut RawStdin, writer);
        #[cfg(not(target_os = "linux"))]
        {
            let stdin = io::stdin();
            let mut reader = stdin.lock();
            tr::delete_squeeze(&delete_set, &set2, &mut reader, writer)
        }
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
        #[cfg(target_os = "linux")]
        return tr::delete(&delete_set, &mut RawStdin, writer);
        #[cfg(not(target_os = "linux"))]
        {
            let stdin = io::stdin();
            let mut reader = stdin.lock();
            tr::delete(&delete_set, &mut reader, writer)
        }
    } else if cli.squeeze && cli.sets.len() < 2 {
        let set1 = tr::parse_set(set1_str);
        let squeeze_set = if cli.complement {
            tr::complement(&set1)
        } else {
            set1
        };
        #[cfg(target_os = "linux")]
        return tr::squeeze(&squeeze_set, &mut RawStdin, writer);
        #[cfg(not(target_os = "linux"))]
        {
            let stdin = io::stdin();
            let mut reader = stdin.lock();
            tr::squeeze(&squeeze_set, &mut reader, writer)
        }
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
        #[cfg(target_os = "linux")]
        return tr::translate_squeeze(&set1, &set2, &mut RawStdin, writer);
        #[cfg(not(target_os = "linux"))]
        {
            let stdin = io::stdin();
            let mut reader = stdin.lock();
            tr::translate_squeeze(&set1, &set2, &mut reader, writer)
        }
    } else {
        eprintln!("tr: missing operand after '{}'", set1_str);
        eprintln!("Two strings must be given when translating.");
        eprintln!("Try 'tr --help' for more information.");
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
