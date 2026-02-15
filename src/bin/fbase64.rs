use std::io::{self, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

#[cfg(unix)]
use memmap2::MmapOptions;

use coreutils_rs::base64::core as b64;
use coreutils_rs::common::io::read_file;
use coreutils_rs::common::io_error_msg;

/// Raw stdin reader for zero-overhead pipe reads on Linux.
/// Bypasses Rust's StdinLock (mutex + 8KB BufReader) for direct libc::read(0).
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
    decode: bool,
    ignore_garbage: bool,
    wrap: usize,
    file: Option<String>,
}

/// Hand-rolled argument parser — eliminates clap's ~100-200µs initialization.
/// base64's args are simple: -d, -i, -w COLS, and an optional FILE positional.
fn parse_args() -> Cli {
    let mut cli = Cli {
        decode: false,
        ignore_garbage: false,
        wrap: 76,
        file: None,
    };

    let mut args = std::env::args_os().skip(1);
    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            if let Some(f) = args.next() {
                cli.file = Some(f.to_string_lossy().into_owned());
            }
            break;
        }
        if bytes.starts_with(b"--") {
            if bytes.starts_with(b"--wrap=") {
                let val = std::str::from_utf8(&bytes[7..]).unwrap_or("76");
                cli.wrap = val.parse().unwrap_or_else(|_| {
                    eprintln!("base64: invalid wrap size: '{}'", val);
                    process::exit(1);
                });
            } else {
                match bytes {
                    b"--decode" => cli.decode = true,
                    b"--ignore-garbage" => cli.ignore_garbage = true,
                    b"--wrap" => {
                        if let Some(v) = args.next() {
                            let s = v.to_string_lossy();
                            cli.wrap = s.parse().unwrap_or_else(|_| {
                                eprintln!("base64: invalid wrap size: '{}'", s);
                                process::exit(1);
                            });
                        } else {
                            eprintln!("base64: option '--wrap' requires an argument");
                            process::exit(1);
                        }
                    }
                    b"--help" => {
                        print!(
                            "Usage: base64 [OPTION]... [FILE]\n\
                            Base64 encode or decode FILE, or standard input, to standard output.\n\n\
                            With no FILE, or when FILE is -, read standard input.\n\n\
                            Mandatory arguments to long options are mandatory for short options too.\n\
                            \x20 -d, --decode          decode data\n\
                            \x20 -i, --ignore-garbage  when decoding, ignore non-alphabet characters\n\
                            \x20 -w, --wrap=COLS       wrap encoded lines after COLS character (default 76).\n\
                            \x20                         Use 0 to disable line wrapping\n\
                            \x20     --help             display this help and exit\n\
                            \x20     --version          output version information and exit\n\n\
                            The data are encoded as described for the base64 alphabet in RFC 4648.\n\
                            When decoding, the input may contain newlines in addition to the bytes of\n\
                            the formal base64 alphabet.  Use --ignore-garbage to attempt to recover\n\
                            from any other non-alphabet bytes in the encoded stream.\n"
                        );
                        process::exit(0);
                    }
                    b"--version" => {
                        println!("base64 (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                        process::exit(0);
                    }
                    _ => {
                        eprintln!("base64: unrecognized option '{}'", arg.to_string_lossy());
                        eprintln!("Try 'base64 --help' for more information.");
                        process::exit(1);
                    }
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            let mut i = 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'd' => cli.decode = true,
                    b'i' => cli.ignore_garbage = true,
                    b'w' => {
                        // -w can be followed by value in same arg (-w76) or next arg (-w 76)
                        if i + 1 < bytes.len() {
                            let val = std::str::from_utf8(&bytes[i + 1..]).unwrap_or("76");
                            cli.wrap = val.parse().unwrap_or_else(|_| {
                                eprintln!("base64: invalid wrap size: '{}'", val);
                                process::exit(1);
                            });
                            i = bytes.len();
                            continue;
                        } else if let Some(v) = args.next() {
                            let s = v.to_string_lossy();
                            cli.wrap = s.parse().unwrap_or_else(|_| {
                                eprintln!("base64: invalid wrap size: '{}'", s);
                                process::exit(1);
                            });
                        } else {
                            eprintln!("base64: option requires an argument -- 'w'");
                            process::exit(1);
                        }
                    }
                    _ => {
                        eprintln!("base64: invalid option -- '{}'", bytes[i] as char);
                        eprintln!("Try 'base64 --help' for more information.");
                        process::exit(1);
                    }
                }
                i += 1;
            }
        } else {
            cli.file = Some(arg.to_string_lossy().into_owned());
        }
    }

    cli
}

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

/// Raw fd stdout for zero-overhead writes on Unix (non-Linux).
#[cfg(all(unix, not(target_os = "linux")))]
#[inline]
fn raw_stdout() -> ManuallyDrop<std::fs::File> {
    unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) }
}

/// Enlarge pipe buffers on Linux for higher throughput.
/// Skips /proc read — directly tries decreasing sizes via fcntl.
/// Saves ~50µs startup vs reading /proc/sys/fs/pipe-max-size.
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

    let cli = parse_args();

    let filename = cli.file.as_deref().unwrap_or("-");

    #[cfg(target_os = "linux")]
    let result = {
        let mut writer = VmspliceWriter::new();
        if filename == "-" {
            process_stdin(&cli, &mut writer)
        } else {
            process_file(filename, &cli, &mut writer)
        }
    };
    #[cfg(all(unix, not(target_os = "linux")))]
    let result = {
        let mut raw = raw_stdout();
        if filename == "-" {
            process_stdin(&cli, &mut *raw)
        } else {
            process_file(filename, &cli, &mut *raw)
        }
    };
    #[cfg(not(unix))]
    let result = {
        let stdout = io::stdout();
        let mut out = io::BufWriter::with_capacity(8 * 1024 * 1024, stdout.lock());
        let r = if filename == "-" {
            process_stdin(&cli, &mut out)
        } else {
            process_file(filename, &cli, &mut out)
        };
        if let Err(e) = out.flush()
            && e.kind() != io::ErrorKind::BrokenPipe
        {
            eprintln!("base64: {}", io_error_msg(&e));
            process::exit(1);
        }
        r
    };

    if let Err(e) = result {
        if e.kind() == io::ErrorKind::BrokenPipe {
            process::exit(0);
        }
        if filename != "-" {
            eprintln!("base64: {}: {}", filename, io_error_msg(&e));
        } else {
            eprintln!("base64: {}", io_error_msg(&e));
        }
        process::exit(1);
    }
}

/// Try to mmap stdin as read-only if it's a regular file (e.g., shell redirect `< file`).
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
    // No MAP_POPULATE: it synchronously faults all pages with 4KB,
    // defeating MADV_HUGEPAGE which must be set before faults occur.
    let mmap = unsafe { MmapOptions::new().map(&file) }.ok();
    std::mem::forget(file);
    #[cfg(target_os = "linux")]
    if let Some(ref m) = mmap {
        unsafe {
            let ptr = m.as_ptr() as *mut libc::c_void;
            let len = m.len();
            // HUGEPAGE first: reduces ~25,600 minor faults to ~50 for 100MB.
            if len >= 2 * 1024 * 1024 {
                libc::madvise(ptr, len, libc::MADV_HUGEPAGE);
            }
            libc::madvise(ptr, len, libc::MADV_SEQUENTIAL | libc::MADV_WILLNEED);
        }
    }
    mmap
}

fn process_stdin(cli: &Cli, out: &mut impl Write) -> io::Result<()> {
    if cli.decode {
        #[cfg(unix)]
        if let Some(mmap) = try_mmap_stdin() {
            return b64::decode_to_writer(&mmap, cli.ignore_garbage, out);
        }

        #[cfg(target_os = "linux")]
        return b64::decode_stream(&mut RawStdin, cli.ignore_garbage, out);
        #[cfg(not(target_os = "linux"))]
        {
            let stdin = io::stdin();
            let mut reader = stdin.lock();
            return b64::decode_stream(&mut reader, cli.ignore_garbage, out);
        }
    }

    #[cfg(unix)]
    if let Some(mmap) = try_mmap_stdin() {
        return b64::encode_to_writer(&mmap, cli.wrap, out);
    }

    #[cfg(target_os = "linux")]
    return b64::encode_stream(&mut RawStdin, cli.wrap, out);
    #[cfg(not(target_os = "linux"))]
    {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        b64::encode_stream(&mut reader, cli.wrap, out)
    }
}

fn process_file(filename: &str, cli: &Cli, out: &mut impl Write) -> io::Result<()> {
    if cli.decode {
        // Use mmap (read-only) + decode_to_writer instead of read_file_vec + decode_mmap_inplace.
        // This saves ~2-5ms on 13.5MB: mmap with HUGEPAGE has ~0.5ms overhead vs ~5ms for read().
        // decode_to_writer handles immutable data via try_decode_uniform_lines (copies to local
        // buffers) and SIMD gap-copy fallback (allocates clean Vec), never modifying the input.
        let data = read_file(Path::new(filename))?;
        b64::decode_to_writer(&data, cli.ignore_garbage, out)
    } else {
        let data = read_file(Path::new(filename))?;
        b64::encode_to_writer(&data, cli.wrap, out)
    }
}
