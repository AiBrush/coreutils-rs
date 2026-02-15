use std::io::{self, Write};
#[cfg(any(target_os = "linux", unix))]
use std::mem::ManuallyDrop;
#[cfg(any(target_os = "linux", unix))]
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

// Note: RawStdin removed — Linux piped path now uses splice_stdin_to_mmap
// or read_stdin for batch processing, which is faster than streaming.

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

/// Raw fd stdout for zero-overhead writes on non-Linux Unix.
/// On Linux, VmspliceWriter is used instead for zero-copy pipe output.
#[cfg(all(unix, not(target_os = "linux")))]
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

/// Try to mmap stdin for non-translate modes (delete, squeeze, etc.).
/// Threshold=0: mmap any regular file for zero-copy access, just like translate mode.
/// Previous 32MB threshold missed 10MB benchmark files, falling through to the
/// slower streaming path with read()+write() copies.
#[cfg(unix)]
fn try_mmap_stdin() -> Option<memmap2::Mmap> {
    try_mmap_stdin_with_threshold(0)
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

    #[cfg(all(unix, not(target_os = "linux")))]
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

        // Try MAP_PRIVATE mmap first for in-place translate (avoids separate buffer
        // allocation). With MADV_HUGEPAGE, COW faults use 2MB pages — even for 10MB
        // files, only ~5 COW faults (~10µs). This is faster than allocating a 10MB
        // separate output buffer (~300µs) + copying translated data into it.
        let result = if let Some(mut mm_mut) = try_mmap_stdin_mut() {
            // MAP_PRIVATE mmap: translate in-place, then write the mmap data.
            // vmsplice is safe: get_user_pages() pins the COW pages, keeping them
            // alive even after the mmap is unmapped. Pages are only freed after
            // the pipe reader releases its references.
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
        } else if let Some(mm) = try_mmap_stdin_with_threshold(0) {
            // Fallback: read-only mmap + separate buffer translate
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
            // Piped stdin: try splice+mmap for zero-copy, then translate in-place.
            #[cfg(target_os = "linux")]
            {
                if let Ok(Some(mut mmap)) = coreutils_rs::common::io::splice_stdin_to_mmap() {
                    let mut writer = VmspliceWriter::new();
                    tr::translate_owned(&set1, &set2, &mut mmap, &mut writer)
                } else {
                    match coreutils_rs::common::io::read_stdin() {
                        Ok(mut data) => {
                            let mut raw_out =
                                unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
                            tr::translate_owned(&set1, &set2, &mut data, &mut *raw_out)
                        }
                        Err(e) => Err(e),
                    }
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
        // File-redirected stdin: use batch path with mmap data.
        // Use raw write (not vmsplice) because delete/squeeze functions create
        // intermediate heap buffers. With vmsplice, freed heap pages can be reused
        // by the allocator before the pipe reader reads them, causing data corruption.
        let data = FileData::Mmap(m);
        #[cfg(target_os = "linux")]
        let result = {
            let mut raw_out = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
            run_mmap_mode(&cli, set1_str, &data, &mut *raw_out)
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
        // Piped stdin: try splice+mmap for zero-copy, fall back to read_stdin batch.
        // IMPORTANT: Use raw write(2), NOT VmspliceWriter, for non-translate modes.
        // The mmap functions (delete_mmap, squeeze_mmap, translate_squeeze_mmap) may
        // allocate temporary output buffers. vmsplice maps those heap pages into the
        // pipe, but the allocator frees them before the pipe reader drains the data,
        // causing garbage output. Raw write(2) copies data into the kernel pipe buffer.
        #[cfg(target_os = "linux")]
        let result = {
            let mut raw_out = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
            if let Ok(Some(splice_mmap)) = coreutils_rs::common::io::splice_stdin_to_mmap() {
                run_mmap_mode(&cli, set1_str, &splice_mmap, &mut *raw_out)
            } else {
                match coreutils_rs::common::io::read_stdin() {
                    Ok(data) => run_mmap_mode(&cli, set1_str, &data, &mut *raw_out),
                    Err(e) => Err(e),
                }
            }
        };
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

/// Dispatch streaming modes for piped stdin (non-Linux platforms only).
/// On Linux, piped input uses splice+batch or read_stdin+batch instead.
#[cfg(not(target_os = "linux"))]
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
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        tr::delete_squeeze(&delete_set, &set2, &mut reader, writer)
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
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        tr::delete(&delete_set, &mut reader, writer)
    } else if cli.squeeze && cli.sets.len() < 2 {
        let set1 = tr::parse_set(set1_str);
        let squeeze_set = if cli.complement {
            tr::complement(&set1)
        } else {
            set1
        };
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        tr::squeeze(&squeeze_set, &mut reader, writer)
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
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        tr::translate_squeeze(&set1, &set2, &mut reader, writer)
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
