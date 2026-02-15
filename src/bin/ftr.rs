use std::io::{self, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::process;

use coreutils_rs::common::io::FileData;
use coreutils_rs::common::io_error_msg;
use coreutils_rs::tr;

/// Writer that uses vmsplice(2) for zero-copy pipe output on Linux.
/// When stdout is a pipe, vmsplice references user-space pages directly
/// in the pipe buffer (no kernel memcpy). Falls back to regular write
/// for non-pipe fds (files, terminals).
#[cfg(target_os = "linux")]
struct VmspliceWriter {
    raw: ManuallyDrop<std::fs::File>,
    is_pipe: bool,
}

#[cfg(target_os = "linux")]
impl VmspliceWriter {
    fn new() -> Self {
        let raw = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
        let is_pipe = unsafe {
            let mut stat: libc::stat = std::mem::zeroed();
            libc::fstat(1, &mut stat) == 0 && (stat.st_mode & libc::S_IFMT) == libc::S_IFIFO
        };
        Self { raw, is_pipe }
    }
}

#[cfg(target_os = "linux")]
impl Write for VmspliceWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.is_pipe || buf.is_empty() {
            return (&*self.raw).write(buf);
        }
        let iov = libc::iovec {
            iov_base: buf.as_ptr() as *mut libc::c_void,
            iov_len: buf.len(),
        };
        let n = unsafe { libc::vmsplice(1, &iov, 1, 0) };
        if n >= 0 {
            Ok(n as usize)
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(0);
            }
            self.is_pipe = false;
            (&*self.raw).write(buf)
        }
    }

    fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        if !self.is_pipe || buf.is_empty() {
            return (&*self.raw).write_all(buf);
        }
        while !buf.is_empty() {
            let iov = libc::iovec {
                iov_base: buf.as_ptr() as *mut libc::c_void,
                iov_len: buf.len(),
            };
            let n = unsafe { libc::vmsplice(1, &iov, 1, 0) };
            if n > 0 {
                buf = &buf[n as usize..];
            } else if n == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "vmsplice wrote 0"));
            } else {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                self.is_pipe = false;
                return (&*self.raw).write_all(buf);
            }
        }
        Ok(())
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        if !self.is_pipe || bufs.is_empty() {
            return (&*self.raw).write_vectored(bufs);
        }
        let count = bufs.len().min(1024);
        let iovs = bufs.as_ptr() as *const libc::iovec;
        let n = unsafe { libc::vmsplice(1, iovs, count, 0) };
        if n >= 0 {
            Ok(n as usize)
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(0);
            }
            self.is_pipe = false;
            (&*self.raw).write_vectored(bufs)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

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

struct Cli {
    complement: bool,
    delete: bool,
    squeeze: bool,
    truncate: bool,
    sets: Vec<String>,
}

/// Hand-rolled argument parser — eliminates clap's ~100-200µs initialization.
/// tr's args are simple: -c/-C, -d, -s, -t flags + 1-2 positional SET args.
fn parse_args() -> Cli {
    let mut cli = Cli {
        complement: false,
        delete: false,
        squeeze: false,
        truncate: false,
        sets: Vec::with_capacity(2),
    };

    let mut args = std::env::args_os().skip(1);
    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            // Everything after -- is positional
            for a in args {
                cli.sets.push(a.to_string_lossy().into_owned());
            }
            break;
        }
        if bytes.starts_with(b"--") {
            match bytes {
                b"--complement" => cli.complement = true,
                b"--delete" => cli.delete = true,
                b"--squeeze-repeats" => cli.squeeze = true,
                b"--truncate-set1" => cli.truncate = true,
                b"--help" => {
                    print!(
                        "Usage: tr [OPTION]... SET1 [SET2]\n\
                        Translate, squeeze, and/or delete characters from standard input,\n\
                        writing to standard output.\n\n\
                        \x20 -c, -C, --complement    use the complement of SET1\n\
                        \x20 -d, --delete            delete characters in SET1, do not translate\n\
                        \x20 -s, --squeeze-repeats   replace each sequence of a repeated character\n\
                        \x20                         that is listed in the last specified SET,\n\
                        \x20                         with a single occurrence of that character\n\
                        \x20 -t, --truncate-set1     first truncate SET1 to length of SET2\n\
                        \x20     --help              display this help and exit\n\
                        \x20     --version           output version information and exit\n"
                    );
                    process::exit(0);
                }
                b"--version" => {
                    println!("tr (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                    process::exit(0);
                }
                _ => {
                    eprintln!("tr: unrecognized option '{}'", arg.to_string_lossy());
                    eprintln!("Try 'tr --help' for more information.");
                    process::exit(1);
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            // Short options: -c, -d, -s, -t (can be combined: -ds, -cd, etc.)
            for &b in &bytes[1..] {
                match b {
                    b'c' | b'C' => cli.complement = true,
                    b'd' => cli.delete = true,
                    b's' => cli.squeeze = true,
                    b't' => cli.truncate = true,
                    _ => {
                        eprintln!("tr: invalid option -- '{}'", b as char);
                        eprintln!("Try 'tr --help' for more information.");
                        process::exit(1);
                    }
                }
            }
        } else {
            cli.sets.push(arg.to_string_lossy().into_owned());
        }
    }

    if cli.sets.is_empty() {
        eprintln!("tr: missing operand");
        eprintln!("Try 'tr --help' for more information.");
        process::exit(1);
    }

    cli
}

/// Raw fd stdout for zero-overhead writes on Unix.
#[cfg(unix)]
#[inline]
fn raw_stdout() -> ManuallyDrop<std::fs::File> {
    unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) }
}

/// Try to mmap stdin if it's a regular file (e.g., shell redirect `< file`).
/// Returns None if stdin is a pipe/terminal, file is too small for mmap
/// benefit, or on non-unix platforms.
/// `min_size` controls the minimum file size for mmap (0 = any size).
#[cfg(unix)]
fn try_mmap_stdin_with_threshold(min_size: usize) -> Option<memmap2::Mmap> {
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

    let file_size = stat.st_size as usize;

    if file_size < min_size {
        return None;
    }

    // mmap the stdin file descriptor.
    // MAP_POPULATE for files >= 4MB to prefault pages during mmap() call.
    // For smaller files, lazy faulting with sequential access is faster.
    // SAFETY: fd is valid, file is regular, size > 0
    use std::os::unix::io::FromRawFd;
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mmap: Option<memmap2::Mmap> = if file_size >= 4 * 1024 * 1024 {
        unsafe { memmap2::MmapOptions::new().populate().map(&file) }.ok()
    } else {
        unsafe { memmap2::MmapOptions::new().map(&file) }.ok()
    };
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

/// Try to mmap stdin with default threshold (32MB) for non-translate modes.
#[cfg(unix)]
fn try_mmap_stdin() -> Option<memmap2::Mmap> {
    try_mmap_stdin_with_threshold(32 * 1024 * 1024)
}

#[cfg(not(unix))]
fn try_mmap_stdin() -> Option<memmap2::Mmap> {
    None
}

#[cfg(not(unix))]
fn try_mmap_stdin_with_threshold(_min_size: usize) -> Option<memmap2::Mmap> {
    None
}

/// Try to create a MAP_PRIVATE (copy-on-write) mmap of stdin for in-place translate.
/// MAP_PRIVATE means writes only affect our process's copy — the underlying file
/// is unmodified. Only used for large files (called after try_mmap_stdin
/// returns a read-only mmap that's >= 64MB and dropped).
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
    // Always use MAP_POPULATE here since this path is only reached for large files
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

/// Enlarge pipe buffers on Linux for higher throughput.
/// Skips /proc read — directly tries decreasing sizes via fcntl.
/// Saves ~50µs startup vs reading /proc/sys/fs/pipe-max-size.
#[cfg(target_os = "linux")]
fn enlarge_pipe_bufs() {
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
    enlarge_pipe_bufs();

    let cli = parse_args();

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

        // For translate mode, mmap any regular file (threshold=0) to avoid
        // the kernel→userspace copy from read(). For piped input, fall through
        // to the streaming path with VmspliceWriter for zero-copy output.
        let result = if let Some(mm) = try_mmap_stdin_with_threshold(0) {
            if mm.len() < 64 * 1024 * 1024 {
                // Read-only mmap: translate into separate buffer (no COW faults)
                #[cfg(target_os = "linux")]
                {
                    let mut writer = VmspliceWriter::new();
                    tr::translate_mmap_readonly(&set1, &set2, &mm, &mut writer)
                }
                #[cfg(all(unix, not(target_os = "linux")))]
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
                    #[cfg(target_os = "linux")]
                    {
                        let mut writer = VmspliceWriter::new();
                        tr::translate_mmap_inplace(&set1, &set2, &mut mm_mut, &mut writer)
                    }
                    #[cfg(all(unix, not(target_os = "linux")))]
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
                    // Fallback: read all stdin, translate in-place, write with vmsplice.
                    #[cfg(target_os = "linux")]
                    {
                        match coreutils_rs::common::io::read_stdin() {
                            Ok(mut data) => {
                                let mut writer = VmspliceWriter::new();
                                tr::translate_owned(&set1, &set2, &mut data, &mut writer)
                            }
                            Err(e) => Err(e),
                        }
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
            // Piped stdin: use streaming translate for pipelining with upstream cat.
            // RawStdin reads chunks as they arrive; ftr processes chunk N while cat writes N+1.
            // This is critical for `cat file | ftr` benchmarks — batch mode kills pipelining.
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
        // File-redirected stdin: use batch path with mmap data + VmspliceWriter
        let data = FileData::Mmap(m);
        #[cfg(target_os = "linux")]
        let result = {
            let mut writer = VmspliceWriter::new();
            run_mmap_mode(&cli, set1_str, &data, &mut writer)
        };
        #[cfg(all(unix, not(target_os = "linux")))]
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
        // Piped stdin: use streaming mode for pipelining with upstream cat.
        // Streaming processes chunks as they arrive from the pipe, enabling
        // ftr to process chunk N while cat writes chunk N+1.
        // Using raw write (not vmsplice) because delete/squeeze allocate temporary
        // output buffers — vmsplice would cause use-after-free.
        #[cfg(target_os = "linux")]
        let result = run_streaming_mode(&cli, set1_str, &mut *raw);
        #[cfg(all(unix, not(target_os = "linux")))]
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
