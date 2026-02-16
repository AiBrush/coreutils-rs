use std::io::{self, Read, Write};
use std::path::Path;

use memchr::memchr_iter;

use crate::common::io::{FileData, read_file, read_stdin};

/// Mode for head operation
#[derive(Clone, Debug)]
pub enum HeadMode {
    /// First N lines (default: 10)
    Lines(u64),
    /// All but last N lines
    LinesFromEnd(u64),
    /// First N bytes
    Bytes(u64),
    /// All but last N bytes
    BytesFromEnd(u64),
}

/// Configuration for head
#[derive(Clone, Debug)]
pub struct HeadConfig {
    pub mode: HeadMode,
    pub zero_terminated: bool,
}

impl Default for HeadConfig {
    fn default() -> Self {
        Self {
            mode: HeadMode::Lines(10),
            zero_terminated: false,
        }
    }
}

/// Parse a numeric argument with optional suffix (K, M, G, etc.)
/// Supports: b(512), kB(1000), K(1024), MB(1e6), M(1048576), GB(1e9), G(1<<30),
/// TB, T, PB, P, EB, E, ZB, Z, YB, Y
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size".to_string());
    }

    // Find where the numeric part ends
    let mut num_end = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || (i == 0 && (c == '+' || c == '-')) {
            num_end = i + c.len_utf8();
        } else {
            break;
        }
    }

    if num_end == 0 {
        return Err(format!("invalid number: '{}'", s));
    }

    let num_str = &s[..num_end];
    let suffix = &s[num_end..];

    let num: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid number: '{}'", num_str))?;

    let multiplier: u64 = match suffix {
        "" => 1,
        "b" => 512,
        "kB" => 1000,
        "K" | "KiB" => 1024,
        "MB" => 1_000_000,
        "M" | "MiB" => 1_048_576,
        "GB" => 1_000_000_000,
        "G" | "GiB" => 1_073_741_824,
        "TB" => 1_000_000_000_000,
        "T" | "TiB" => 1_099_511_627_776,
        "PB" => 1_000_000_000_000_000,
        "P" | "PiB" => 1_125_899_906_842_624,
        "EB" => 1_000_000_000_000_000_000,
        "E" | "EiB" => 1_152_921_504_606_846_976,
        // ZB/Z/YB/Y would overflow u64, treat as max
        "ZB" | "Z" | "ZiB" | "YB" | "Y" | "YiB" => {
            if num > 0 {
                return Ok(u64::MAX);
            }
            return Ok(0);
        }
        _ => return Err(format!("invalid suffix in '{}'", s)),
    };

    num.checked_mul(multiplier)
        .ok_or_else(|| format!("number too large: '{}'", s))
}

/// Output first N lines from data
pub fn head_lines(data: &[u8], n: u64, delimiter: u8, out: &mut impl Write) -> io::Result<()> {
    if n == 0 || data.is_empty() {
        return Ok(());
    }

    let mut count = 0u64;
    for pos in memchr_iter(delimiter, data) {
        count += 1;
        if count == n {
            return out.write_all(&data[..=pos]);
        }
    }

    // Fewer than N lines — output everything
    out.write_all(data)
}

/// Output all but last N lines from data
pub fn head_lines_from_end(
    data: &[u8],
    n: u64,
    delimiter: u8,
    out: &mut impl Write,
) -> io::Result<()> {
    if n == 0 {
        return out.write_all(data);
    }
    if data.is_empty() {
        return Ok(());
    }

    // Count total lines
    let total_lines: u64 = memchr_iter(delimiter, data).count() as u64;

    if n >= total_lines {
        return Ok(());
    }

    let target = total_lines - n;
    head_lines(data, target, delimiter, out)
}

/// Output first N bytes from data
pub fn head_bytes(data: &[u8], n: u64, out: &mut impl Write) -> io::Result<()> {
    let n = n.min(data.len() as u64) as usize;
    if n > 0 {
        out.write_all(&data[..n])?;
    }
    Ok(())
}

/// Output all but last N bytes from data
pub fn head_bytes_from_end(data: &[u8], n: u64, out: &mut impl Write) -> io::Result<()> {
    if n >= data.len() as u64 {
        return Ok(());
    }
    let end = data.len() - n as usize;
    if end > 0 {
        out.write_all(&data[..end])?;
    }
    Ok(())
}

/// Use sendfile for zero-copy byte output on Linux
#[cfg(target_os = "linux")]
pub fn sendfile_bytes(path: &Path, n: u64, out_fd: i32) -> io::Result<bool> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOATIME)
        .open(path)
        .or_else(|_| std::fs::File::open(path))?;

    let metadata = file.metadata()?;
    let file_size = metadata.len();
    let to_send = n.min(file_size) as usize;

    if to_send == 0 {
        return Ok(true);
    }

    use std::os::unix::io::AsRawFd;
    let in_fd = file.as_raw_fd();
    let mut offset: libc::off_t = 0;
    let mut remaining = to_send;

    while remaining > 0 {
        let chunk = remaining.min(0x7ffff000); // sendfile max per call
        let ret = unsafe { libc::sendfile(out_fd, in_fd, &mut offset, chunk) };
        if ret > 0 {
            remaining -= ret as usize;
        } else if ret == 0 {
            break;
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
    }

    Ok(true)
}

/// Streaming head for positive line count on a regular file.
/// Reads small chunks from the start, never mmaps the whole file.
/// This is the critical fast path: `head -n 10` on a 100MB file
/// reads only a few KB instead of mapping all 100MB.
fn head_lines_streaming_file(
    path: &Path,
    n: u64,
    delimiter: u8,
    out: &mut impl Write,
) -> io::Result<bool> {
    if n == 0 {
        return Ok(true);
    }

    #[cfg(target_os = "linux")]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOATIME)
            .open(path)
            .or_else(|_| std::fs::File::open(path))?
    };
    #[cfg(not(target_os = "linux"))]
    let file = std::fs::File::open(path)?;

    let mut reader = io::BufReader::with_capacity(1024 * 1024, file);
    let mut buf = [0u8; 262144];
    let mut count = 0u64;

    loop {
        let bytes_read = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };

        let chunk = &buf[..bytes_read];

        for pos in memchr_iter(delimiter, chunk) {
            count += 1;
            if count == n {
                out.write_all(&chunk[..=pos])?;
                return Ok(true);
            }
        }

        out.write_all(chunk)?;
    }

    Ok(true)
}

/// Process a single file/stdin for head
pub fn head_file(
    filename: &str,
    config: &HeadConfig,
    out: &mut impl Write,
    tool_name: &str,
) -> io::Result<bool> {
    let delimiter = if config.zero_terminated { b'\0' } else { b'\n' };

    if filename != "-" {
        let path = Path::new(filename);

        // Fast paths that avoid reading/mmapping the whole file
        match &config.mode {
            HeadMode::Lines(n) => {
                // Streaming: read small chunks, stop after N lines
                match head_lines_streaming_file(path, *n, delimiter, out) {
                    Ok(true) => return Ok(true),
                    Err(e) => {
                        eprintln!(
                            "{}: cannot open '{}' for reading: {}",
                            tool_name,
                            filename,
                            crate::common::io_error_msg(&e)
                        );
                        return Ok(false);
                    }
                    _ => {}
                }
            }
            HeadMode::Bytes(n) => {
                // sendfile: zero-copy, reads only N bytes
                #[cfg(target_os = "linux")]
                {
                    use std::os::unix::io::AsRawFd;
                    let stdout = io::stdout();
                    let out_fd = stdout.as_raw_fd();
                    if let Ok(true) = sendfile_bytes(path, *n, out_fd) {
                        return Ok(true);
                    }
                }
                // Non-Linux: still avoid full mmap
                #[cfg(not(target_os = "linux"))]
                {
                    if let Ok(true) = head_bytes_streaming_file(path, *n, out) {
                        return Ok(true);
                    }
                }
            }
            _ => {
                // LinesFromEnd and BytesFromEnd need the whole file — use mmap
            }
        }
    }

    // Slow path: read entire file (needed for -n -N, -c -N, or stdin)
    let data: FileData = if filename == "-" {
        match read_stdin() {
            Ok(d) => FileData::Owned(d),
            Err(e) => {
                eprintln!(
                    "{}: standard input: {}",
                    tool_name,
                    crate::common::io_error_msg(&e)
                );
                return Ok(false);
            }
        }
    } else {
        match read_file(Path::new(filename)) {
            Ok(d) => d,
            Err(e) => {
                eprintln!(
                    "{}: cannot open '{}' for reading: {}",
                    tool_name,
                    filename,
                    crate::common::io_error_msg(&e)
                );
                return Ok(false);
            }
        }
    };

    match &config.mode {
        HeadMode::Lines(n) => head_lines(&data, *n, delimiter, out)?,
        HeadMode::LinesFromEnd(n) => head_lines_from_end(&data, *n, delimiter, out)?,
        HeadMode::Bytes(n) => head_bytes(&data, *n, out)?,
        HeadMode::BytesFromEnd(n) => head_bytes_from_end(&data, *n, out)?,
    }

    Ok(true)
}

/// Streaming head for positive byte count on non-Linux.
#[cfg(not(target_os = "linux"))]
fn head_bytes_streaming_file(path: &Path, n: u64, out: &mut impl Write) -> io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    let mut remaining = n as usize;
    let mut buf = [0u8; 65536];

    while remaining > 0 {
        let to_read = remaining.min(buf.len());
        let bytes_read = match file.read(&mut buf[..to_read]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        out.write_all(&buf[..bytes_read])?;
        remaining -= bytes_read;
    }

    Ok(true)
}

/// Process head for stdin streaming (line mode, positive count)
/// Reads chunks and counts lines, stopping early once count reached.
pub fn head_stdin_lines_streaming(n: u64, delimiter: u8, out: &mut impl Write) -> io::Result<()> {
    if n == 0 {
        return Ok(());
    }

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut buf = [0u8; 262144];
    let mut count = 0u64;

    loop {
        let bytes_read = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };

        let chunk = &buf[..bytes_read];

        // Count delimiters in this chunk
        for pos in memchr_iter(delimiter, chunk) {
            count += 1;
            if count == n {
                out.write_all(&chunk[..=pos])?;
                return Ok(());
            }
        }

        // Haven't reached N lines yet, output entire chunk
        out.write_all(chunk)?;
    }

    Ok(())
}
