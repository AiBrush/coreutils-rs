use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

#[cfg(unix)]
use memmap2::MmapOptions;

use coreutils_rs::common::io::{FileData, read_file_mmap, read_file_vec, read_stdin};
use coreutils_rs::common::io_error_msg;
use coreutils_rs::tac;

/// Writer that uses vmsplice(2) for zero-copy pipe output on Linux.
/// For tac's write_vectored path, vmsplice with scatter-gather iovecs
/// references mmap pages directly in the pipe (no kernel memcpy).
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
        // SAFETY: IoSlice is #[repr(transparent)] over iovec on Unix,
        // so &[IoSlice] has the same memory layout as &[iovec].
        // Direct pointer cast eliminates the per-call copy loop (1024 iterations)
        // and 16KB memset that the stack-allocated array approach required.
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

struct Cli {
    before: bool,
    regex: bool,
    separator: Option<String>,
    files: Vec<String>,
}

/// Hand-rolled argument parser — eliminates clap's ~100-200µs initialization.
/// tac has very few options: -b, -r, -s STRING, --help, --version, and files.
fn parse_args() -> Cli {
    let mut cli = Cli {
        before: false,
        regex: false,
        separator: None,
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
            if bytes.starts_with(b"--separator=") {
                let val = arg.to_string_lossy();
                cli.separator = Some(val[12..].to_string());
                continue;
            }
            match bytes {
                b"--before" => cli.before = true,
                b"--regex" => cli.regex = true,
                b"--separator" => {
                    cli.separator = Some(
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("tac: option '--separator' requires an argument");
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                b"--help" => {
                    print!(
                        "Usage: tac [OPTION]... [FILE]...\n\
                         Write each FILE to standard output, last line first.\n\n\
                         With no FILE, or when FILE is -, read standard input.\n\n\
                         Mandatory arguments to long options are mandatory for short options too.\n\
                         \x20 -b, --before             attach the separator before instead of after\n\
                         \x20 -r, --regex              interpret the separator as a regular expression\n\
                         \x20 -s, --separator=STRING    use STRING as the separator instead of newline\n\
                         \x20     --help               display this help and exit\n\
                         \x20     --version            output version information and exit\n"
                    );
                    process::exit(0);
                }
                b"--version" => {
                    println!("tac (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                    process::exit(0);
                }
                _ => {
                    eprintln!("tac: unrecognized option '{}'", arg.to_string_lossy());
                    eprintln!("Try 'tac --help' for more information.");
                    process::exit(1);
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            let mut i = 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'b' => cli.before = true,
                    b'r' => cli.regex = true,
                    b's' => {
                        // -s takes a value: rest of this arg or next arg
                        if i + 1 < bytes.len() {
                            let val = arg.to_string_lossy();
                            cli.separator = Some(val[i + 1..].to_string());
                        } else {
                            cli.separator = Some(
                                args.next()
                                    .unwrap_or_else(|| {
                                        eprintln!("tac: option requires an argument -- 's'");
                                        process::exit(1);
                                    })
                                    .to_string_lossy()
                                    .into_owned(),
                            );
                        }
                        break; // consumed rest of arg
                    }
                    _ => {
                        eprintln!("tac: invalid option -- '{}'", bytes[i] as char);
                        eprintln!("Try 'tac --help' for more information.");
                        process::exit(1);
                    }
                }
                i += 1;
            }
        } else {
            cli.files.push(arg.to_string_lossy().into_owned());
        }
    }

    cli
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
            let ptr = m.as_ptr() as *mut libc::c_void;
            let len = m.len();
            // HUGEPAGE first: must be set before any page faults occur.
            // Reduces ~25,600 minor faults to ~50 for 100MB.
            if len >= 2 * 1024 * 1024 {
                libc::madvise(ptr, len, libc::MADV_HUGEPAGE);
            }
            // WILLNEED after: async readahead will use 2MB huge pages.
            // Don't use SEQUENTIAL since tac accesses data in reverse order.
            libc::madvise(ptr, len, libc::MADV_WILLNEED);
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
            // For files < 16MB, use read() instead of mmap to avoid page fault
            // overhead. mmap on 10MB = 2560 minor page faults (~0.8ms) even with
            // warm page cache, because each new process has empty page tables.
            // read() copies data to heap in large chunks with cheaper sequential
            // faults. For >= 16MB, mmap enables zero-copy vmsplice output and
            // parallel scanning benefits outweigh fault overhead.
            let path = Path::new(filename);
            const TAC_MMAP_THRESHOLD: u64 = 16 * 1024 * 1024;
            match std::fs::metadata(path) {
                Ok(ref m) if m.is_file() && m.len() > 0 && m.len() < TAC_MMAP_THRESHOLD => {
                    match read_file_vec(path) {
                        Ok(d) => FileData::Owned(d),
                        Err(e) => {
                            eprintln!("tac: {}: {}", filename, io_error_msg(&e));
                            had_error = true;
                            continue;
                        }
                    }
                }
                _ => match read_file_mmap(path) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("tac: {}: {}", filename, io_error_msg(&e));
                        had_error = true;
                        continue;
                    }
                },
            }
        };

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
/// Skips /proc read (~50µs) — directly tries decreasing sizes via fcntl.
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
    coreutils_rs::common::reset_sigpipe();

    #[cfg(target_os = "linux")]
    enlarge_pipes();

    let mut cli = parse_args();

    let files: Vec<String> = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        std::mem::take(&mut cli.files)
    };

    let is_byte_sep = !cli.regex && cli.separator.is_none();

    // On Linux: VmspliceWriter for zero-copy pipe output via vmsplice(2).
    // write_vectored maps to vmsplice with scatter-gather iovecs,
    // referencing mmap pages directly in the pipe (no kernel memcpy).
    #[cfg(target_os = "linux")]
    let had_error = {
        if is_byte_sep {
            let mut vwriter = VmspliceWriter::new();
            run(&cli, &files, &mut vwriter)
        } else {
            let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, VmspliceWriter::new());
            let err = run(&cli, &files, &mut writer);
            let _ = writer.flush();
            err
        }
    };
    // On other Unix: raw fd stdout for zero-copy writev.
    #[cfg(all(unix, not(target_os = "linux")))]
    let had_error = {
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
        if is_byte_sep {
            let mut writer = lock;
            run(&cli, &files, &mut writer)
        } else {
            let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, lock);
            let err = run(&cli, &files, &mut writer);
            let _ = writer.flush();
            err
        }
    };

    if had_error {
        process::exit(1);
    }
}
