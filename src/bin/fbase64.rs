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

use coreutils_rs::base64::core as b64;
use coreutils_rs::common::io::{read_file, read_file_vec};
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

#[derive(Parser)]
#[command(
    name = "base64",
    about = "Base64 encode or decode FILE, or standard input, to standard output.",
    after_help = "With no FILE, or when FILE is -, read standard input.\n\n\
        The data are encoded as described for the base64 alphabet in RFC 4648.\n\
        When decoding, the input may contain newlines in addition to the bytes of\n\
        the formal base64 alphabet.  Use --ignore-garbage to attempt to recover\n\
        from any other non-alphabet bytes in the encoded stream.",
    version
)]
struct Cli {
    /// Decode data
    #[arg(short = 'd', long = "decode")]
    decode: bool,

    /// When decoding, ignore non-alphabet characters
    #[arg(short = 'i', long = "ignore-garbage")]
    ignore_garbage: bool,

    /// Wrap encoded lines after COLS character (default 76).
    /// Use 0 to disable line wrapping
    #[arg(short = 'w', long = "wrap", value_name = "COLS", default_value = "76")]
    wrap: usize,

    /// File to process (reads stdin if omitted or -)
    file: Option<String>,
}

/// Raw fd stdout for zero-overhead writes on Unix (non-Linux).
/// On Linux, VmspliceWriter is used instead for zero-copy pipe output.
#[cfg(all(unix, not(target_os = "linux")))]
#[inline]
fn raw_stdout() -> ManuallyDrop<std::fs::File> {
    unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) }
}

/// Raw fd stdout for zero-overhead writes on Linux.
/// Uses regular write() — vmsplice is unsafe because the caller may free/reuse
/// buffers before the pipe reader consumes the data.
#[cfg(target_os = "linux")]
#[inline]
fn raw_stdout_linux() -> ManuallyDrop<std::fs::File> {
    unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) }
}

/// Enlarge pipe buffers on Linux for higher throughput.
/// Reads system max from /proc, falls back through decreasing sizes.
/// Larger pipe buffers = fewer write() syscalls for encode/decode output.
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

    let filename = cli.file.as_deref().unwrap_or("-");

    // On Linux: use VmspliceWriter for zero-copy pipe output (vmsplice(2)).
    // Eliminates memcpy from user buffer to kernel pipe buffer, saving ~1-2ms
    // for 10MB+ output. Falls back to write() for non-pipe stdout.
    // On other Unix: use raw fd (no BufWriter since callers produce large chunks).
    #[cfg(target_os = "linux")]
    let mut raw = raw_stdout_linux();
    #[cfg(target_os = "linux")]
    let result = if filename == "-" {
        process_stdin(&cli, &mut *raw)
    } else {
        process_file(filename, &cli, &mut *raw)
    };
    #[cfg(all(unix, not(target_os = "linux")))]
    let mut raw = raw_stdout();
    #[cfg(all(unix, not(target_os = "linux")))]
    let result = if filename == "-" {
        process_stdin(&cli, &mut *raw)
    } else {
        process_file(filename, &cli, &mut *raw)
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
    let mmap = unsafe { MmapOptions::new().populate().map(&file) }.ok();
    std::mem::forget(file);
    #[cfg(target_os = "linux")]
    if let Some(ref m) = mmap {
        unsafe {
            let ptr = m.as_ptr() as *mut libc::c_void;
            let len = m.len();
            libc::madvise(ptr, len, libc::MADV_SEQUENTIAL | libc::MADV_WILLNEED);
            if len >= 2 * 1024 * 1024 {
                libc::madvise(ptr, len, libc::MADV_HUGEPAGE);
            }
        }
    }
    mmap
}

fn process_stdin(cli: &Cli, out: &mut impl Write) -> io::Result<()> {
    if cli.decode {
        // Try read-only mmap for stdin decode (avoids MAP_PRIVATE COW overhead).
        // For large data, decode_to_writer uses bulk strip+decode (faster than per-line).
        #[cfg(unix)]
        if let Some(mmap) = try_mmap_stdin() {
            return b64::decode_to_writer(&mmap, cli.ignore_garbage, out);
        }

        // For piped stdin: use streaming decode. RawStdin on Linux bypasses
        // StdinLock overhead for direct libc::read(0).
        #[cfg(target_os = "linux")]
        return b64::decode_stream(&mut RawStdin, cli.ignore_garbage, out);
        #[cfg(not(target_os = "linux"))]
        {
            let stdin = io::stdin();
            let mut reader = stdin.lock();
            return b64::decode_stream(&mut reader, cli.ignore_garbage, out);
        }
    }

    // For encode: try mmap for zero-copy stdin when redirected from a file
    #[cfg(unix)]
    if let Some(mmap) = try_mmap_stdin() {
        return b64::encode_to_writer(&mmap, cli.wrap, out);
    }

    // For piped encode: streaming with RawStdin on Linux.
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
        // Decode: read to Vec for in-place strip+decode.
        // Avoids separate 12MB+ clean buffer — strip whitespace in-place on the Vec,
        // then parallel decode from the cleaned slice. Saves one large allocation.
        let mut data = read_file_vec(Path::new(filename))?;
        b64::decode_mmap_inplace(&mut data, cli.ignore_garbage, out)
    } else {
        // Encode: use read-only mmap (zero-copy, no modification needed).
        let data = read_file(Path::new(filename))?;
        b64::encode_to_writer(&data, cli.wrap, out)
    }
}
