use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

use clap::Parser;
#[cfg(unix)]
use memmap2::MmapOptions;

use coreutils_rs::common::io::{FileData, read_file, read_stdin};
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
        let iovs: Vec<libc::iovec> = bufs
            .iter()
            .map(|b| libc::iovec {
                iov_base: b.as_ptr() as *mut libc::c_void,
                iov_len: b.len(),
            })
            .collect();
        let n = unsafe { libc::vmsplice(1, iovs.as_ptr(), iovs.len(), 0) };
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

#[derive(Parser)]
#[command(
    name = "tac",
    about = "Concatenate and print files in reverse",
    version
)]
struct Cli {
    /// Attach the separator before instead of after
    #[arg(short = 'b', long = "before")]
    before: bool,

    /// Interpret the separator as a regular expression
    #[arg(short = 'r', long = "regex")]
    regex: bool,

    /// Use STRING as the separator instead of newline
    #[arg(
        short = 's',
        long = "separator",
        value_name = "STRING",
        allow_hyphen_values = true
    )]
    separator: Option<String>,

    /// Files to process (reads stdin if none given)
    files: Vec<String>,
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
    let mmap = unsafe { MmapOptions::new().populate().map(&file) }.ok();
    std::mem::forget(file); // Don't close stdin
    #[cfg(target_os = "linux")]
    if let Some(ref m) = mmap {
        unsafe {
            let ptr = m.as_ptr() as *mut libc::c_void;
            let len = m.len();
            // WILLNEED pre-faults all pages. Don't use SEQUENTIAL since tac
            // accesses data in reverse order during the output phase.
            libc::madvise(ptr, len, libc::MADV_WILLNEED);
            if len >= 2 * 1024 * 1024 {
                libc::madvise(ptr, len, libc::MADV_HUGEPAGE);
            }
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
            match read_file(Path::new(filename)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("tac: {}: {}", filename, io_error_msg(&e));
                    had_error = true;
                    continue;
                }
            }
        };

        // Override MADV_SEQUENTIAL from read_file: the contiguous buffer
        // parallel approach scans forward in chunks (memchr) then copies records
        // to the output buffer. WILLNEED pre-faults all pages so the parallel
        // threads don't stall on page faults. Don't use SEQUENTIAL since
        // multiple threads access different regions concurrently.
        #[cfg(target_os = "linux")]
        {
            let ptr = data.as_ptr() as *mut libc::c_void;
            let len = data.len();
            if len > 0 {
                unsafe {
                    libc::madvise(ptr, len, libc::MADV_WILLNEED);
                }
            }
        }

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
/// Reads system max from /proc, falls back through decreasing sizes.
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

    let files: Vec<String> = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
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
