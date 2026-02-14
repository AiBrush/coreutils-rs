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
use coreutils_rs::common::io::read_file;
use coreutils_rs::common::io_error_msg;

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

/// Raw fd stdout for zero-overhead writes on Unix.
/// Bypasses BufWriter/StdoutLock overhead — our callers already batch
/// output into large (4MB+) chunks, so no intermediate buffering needed.
#[cfg(unix)]
#[inline]
fn raw_stdout() -> ManuallyDrop<std::fs::File> {
    unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) }
}

/// Enlarge pipe buffers on Linux for higher throughput.
/// 8MB pipe buffers allow larger reads/writes per syscall, reducing total
/// syscall count for the streaming encode/decode paths.
#[cfg(target_os = "linux")]
fn enlarge_pipes() {
    const PIPE_SIZE: i32 = 8 * 1024 * 1024;
    unsafe {
        libc::fcntl(0, libc::F_SETPIPE_SZ, PIPE_SIZE); // stdin
        libc::fcntl(1, libc::F_SETPIPE_SZ, PIPE_SIZE); // stdout
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    #[cfg(target_os = "linux")]
    enlarge_pipes();

    let cli = Cli::parse();

    let filename = cli.file.as_deref().unwrap_or("-");

    // Encode path: write directly to raw fd (chunks are 3MB+, BufWriter is overhead).
    // Decode path: use BufWriter since decode_clean_slice writes variable-sized chunks.
    #[cfg(unix)]
    let mut raw = raw_stdout();

    // Write directly to raw fd for both encode and decode — bypasses BufWriter.
    // Encode writes 3MB+ chunks; decode (now batch mode via read_stdin + decode_owned)
    // writes the entire decoded output in a single write_all. No intermediate
    // buffering needed since all callers produce large contiguous output.
    #[cfg(unix)]
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
            libc::madvise(
                m.as_ptr() as *mut libc::c_void,
                m.len(),
                libc::MADV_SEQUENTIAL,
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

/// Try to create a MAP_PRIVATE (copy-on-write) mmap of stdin for in-place decode.
/// MAP_PRIVATE means writes only affect our process's copy. For base64 decode,
/// the whitespace stripping only modifies ~1.3% of pages (newline positions),
/// and decode_inplace writes shorter data back, so total COW is minimal.
#[cfg(unix)]
fn try_mmap_stdin_mut() -> Option<memmap2::MmapMut> {
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
    let mmap = unsafe { MmapOptions::new().populate().map_copy(&file) }.ok();
    std::mem::forget(file);
    #[cfg(target_os = "linux")]
    if let Some(ref m) = mmap {
        unsafe {
            libc::madvise(
                m.as_ptr() as *mut libc::c_void,
                m.len(),
                libc::MADV_SEQUENTIAL,
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

fn process_stdin(cli: &Cli, out: &mut impl Write) -> io::Result<()> {
    if cli.decode {
        // Try MAP_PRIVATE mmap for in-place strip+decode (zero allocation)
        #[cfg(unix)]
        if let Some(mut mmap) = try_mmap_stdin_mut() {
            return b64::decode_mmap_inplace(&mut mmap, cli.ignore_garbage, out);
        }

        // For piped stdin: use streaming decode to avoid 64MB read_stdin pre-alloc.
        // decode_stream reads 16MB chunks, strips whitespace with SIMD memchr2,
        // and decodes in-place. For 10MB input this processes everything in 1 chunk.
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        return b64::decode_stream(&mut reader, cli.ignore_garbage, out);
    }

    // For encode: try mmap for zero-copy stdin when redirected from a file
    #[cfg(unix)]
    if let Some(mmap) = try_mmap_stdin() {
        return b64::encode_to_writer(&mmap, cli.wrap, out);
    }

    // For piped encode: use streaming to overlap pipe read with encode+write.
    // read_full reads 12MB aligned chunks from the pipe, encodes + fuse_wraps
    // each chunk, then writes in a single syscall. For 10MB input this processes
    // everything in 1 iteration, avoiding the 64MB read_stdin pre-allocation.
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    b64::encode_stream(&mut reader, cli.wrap, out)
}

fn process_file(filename: &str, cli: &Cli, out: &mut impl Write) -> io::Result<()> {
    if cli.decode {
        // For decode: try MAP_PRIVATE mmap for in-place strip+decode (zero alloc).
        // Falls back to read_file + decode_to_writer for small files or non-mmap platforms.
        #[cfg(unix)]
        {
            let file = std::fs::File::open(Path::new(filename))?;
            let metadata = file.metadata()?;
            if metadata.len() > 0
                && metadata.file_type().is_file()
                && let Ok(mut mmap) = unsafe { MmapOptions::new().populate().map_copy(&file) }
            {
                #[cfg(target_os = "linux")]
                unsafe {
                    libc::madvise(
                        mmap.as_ptr() as *mut libc::c_void,
                        mmap.len(),
                        libc::MADV_SEQUENTIAL,
                    );
                }
                return b64::decode_mmap_inplace(&mut mmap, cli.ignore_garbage, out);
            }
        }
        let data = read_file(Path::new(filename))?;
        b64::decode_to_writer(&data, cli.ignore_garbage, out)
    } else {
        // For encode: use read-only mmap (no modification needed)
        let data = read_file(Path::new(filename))?;
        b64::encode_to_writer(&data, cli.wrap, out)
    }
}
