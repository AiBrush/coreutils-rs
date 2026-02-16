use std::io::{self, Read, Write};
use std::path::Path;

use crate::common::io::{read_file, read_stdin};

/// Configuration for cat
#[derive(Clone, Debug, Default)]
pub struct CatConfig {
    pub number: bool,
    pub number_nonblank: bool,
    pub show_ends: bool,
    pub show_tabs: bool,
    pub show_nonprinting: bool,
    pub squeeze_blank: bool,
}

impl CatConfig {
    /// Returns true if no special processing is needed (plain cat)
    pub fn is_plain(&self) -> bool {
        !self.number
            && !self.number_nonblank
            && !self.show_ends
            && !self.show_tabs
            && !self.show_nonprinting
            && !self.squeeze_blank
    }
}

/// Use splice for zero-copy file→stdout on Linux (file → pipe)
#[cfg(target_os = "linux")]
pub fn splice_file_to_stdout(path: &Path) -> io::Result<bool> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    // Check if stdout is a pipe (splice only works with pipes)
    let stdout = io::stdout();
    let out_fd = stdout.as_raw_fd();
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(out_fd, &mut stat) } != 0 {
        return Ok(false);
    }
    let stdout_is_pipe = (stat.st_mode & libc::S_IFMT) == libc::S_IFIFO;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOATIME)
        .open(path)
        .or_else(|_| std::fs::File::open(path))?;

    let in_fd = file.as_raw_fd();
    let metadata = file.metadata()?;
    let file_size = metadata.len() as usize;

    if file_size == 0 {
        return Ok(true);
    }

    if stdout_is_pipe {
        // splice: zero-copy file→pipe
        let mut remaining = file_size;
        while remaining > 0 {
            let chunk = remaining.min(1024 * 1024 * 1024);
            let ret = unsafe {
                libc::splice(
                    in_fd,
                    std::ptr::null_mut(),
                    out_fd,
                    std::ptr::null_mut(),
                    chunk,
                    libc::SPLICE_F_MOVE,
                )
            };
            if ret > 0 {
                remaining -= ret as usize;
            } else if ret == 0 {
                break;
            } else {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                // splice not supported — fall through to sendfile
                return sendfile_to_stdout(in_fd, file_size, out_fd);
            }
        }
        Ok(true)
    } else {
        // sendfile: zero-copy file→socket/file
        sendfile_to_stdout(in_fd, file_size, out_fd)
    }
}

#[cfg(target_os = "linux")]
fn sendfile_to_stdout(in_fd: i32, file_size: usize, out_fd: i32) -> io::Result<bool> {
    let mut offset: libc::off_t = 0;
    let mut remaining = file_size;

    while remaining > 0 {
        let chunk = remaining.min(0x7ffff000);
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

/// Plain cat for a single file — tries splice/sendfile, then falls back to mmap+write
pub fn cat_plain_file(path: &Path, out: &mut impl Write) -> io::Result<bool> {
    // Try zero-copy first on Linux
    #[cfg(target_os = "linux")]
    {
        match splice_file_to_stdout(path) {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(_) => {} // fall through
        }
    }

    // Fallback: mmap + write
    let data = read_file(path)?;
    if !data.is_empty() {
        out.write_all(&data)?;
    }
    Ok(true)
}

/// Plain cat for stdin — try splice on Linux, otherwise bulk read+write
pub fn cat_plain_stdin(out: &mut impl Write) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        // Try splice stdin→stdout if both are pipes
        let stdin_fd = 0i32;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(1, &mut stat) } == 0
            && (stat.st_mode & libc::S_IFMT) == libc::S_IFIFO
        {
            // stdout is a pipe, try splice from stdin
            loop {
                let ret = unsafe {
                    libc::splice(
                        stdin_fd,
                        std::ptr::null_mut(),
                        1,
                        std::ptr::null_mut(),
                        1024 * 1024 * 1024,
                        libc::SPLICE_F_MOVE,
                    )
                };
                if ret > 0 {
                    continue;
                } else if ret == 0 {
                    return Ok(());
                } else {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    // splice not supported, fall through to read+write
                    break;
                }
            }
        }
    }

    // Fallback: read+write loop
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut buf = [0u8; 131072]; // 128KB buffer
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        out.write_all(&buf[..n])?;
    }
    Ok(())
}

/// Build the 256-byte lookup table for non-printing character display.
/// Returns (table, needs_expansion) where needs_expansion[b] is true if
/// the byte maps to more than one output byte.
fn _build_nonprinting_table(show_tabs: bool) -> ([u8; 256], [bool; 256]) {
    let mut table = [0u8; 256];
    let mut multi = [false; 256];

    for i in 0..256u16 {
        let b = i as u8;
        match b {
            b'\n' => {
                table[i as usize] = b'\n';
            }
            b'\t' => {
                if show_tabs {
                    table[i as usize] = b'I';
                    multi[i as usize] = true;
                } else {
                    table[i as usize] = b'\t';
                }
            }
            0..=8 | 10..=31 => {
                // Control chars: ^@ through ^_
                table[i as usize] = b + 64;
                multi[i as usize] = true;
            }
            32..=126 => {
                table[i as usize] = b;
            }
            127 => {
                // DEL: ^?
                table[i as usize] = b'?';
                multi[i as usize] = true;
            }
            128..=159 => {
                // M-^@ through M-^_
                table[i as usize] = b - 128 + 64;
                multi[i as usize] = true;
            }
            160..=254 => {
                // M-space through M-~
                table[i as usize] = b - 128;
                multi[i as usize] = true;
            }
            255 => {
                // M-^?
                table[i as usize] = b'?';
                multi[i as usize] = true;
            }
        }
    }

    (table, multi)
}

/// Write a non-printing byte in cat -v notation
#[inline]
fn write_nonprinting(b: u8, show_tabs: bool, out: &mut Vec<u8>) {
    match b {
        b'\t' if !show_tabs => out.push(b'\t'),
        b'\n' => out.push(b'\n'),
        0..=8 | 10..=31 => {
            out.push(b'^');
            out.push(b + 64);
        }
        9 => {
            // show_tabs must be true here
            out.push(b'^');
            out.push(b'I');
        }
        32..=126 => out.push(b),
        127 => {
            out.push(b'^');
            out.push(b'?');
        }
        128..=159 => {
            out.push(b'M');
            out.push(b'-');
            out.push(b'^');
            out.push(b - 128 + 64);
        }
        160..=254 => {
            out.push(b'M');
            out.push(b'-');
            out.push(b - 128);
        }
        255 => {
            out.push(b'M');
            out.push(b'-');
            out.push(b'^');
            out.push(b'?');
        }
    }
}

/// Cat with options (numbering, show-ends, show-tabs, show-nonprinting, squeeze)
pub fn cat_with_options(
    data: &[u8],
    config: &CatConfig,
    line_num: &mut u64,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // Pre-allocate output buffer (worst case: every byte expands to 4 chars for M-^X)
    // In practice, most files are mostly printable, so 1.1x is a good estimate
    let estimated = data.len() + data.len() / 10 + 1024;
    let mut buf = Vec::with_capacity(estimated.min(16 * 1024 * 1024));

    let mut prev_blank = false;
    let mut pos = 0;

    while pos < data.len() {
        // Find end of this line
        let line_end = memchr::memchr(b'\n', &data[pos..])
            .map(|p| pos + p + 1)
            .unwrap_or(data.len());

        let line = &data[pos..line_end];
        let is_blank = line == b"\n" || line.is_empty();

        // Squeeze blank lines
        if config.squeeze_blank && is_blank && prev_blank {
            pos = line_end;
            continue;
        }
        prev_blank = is_blank;

        // Line numbering
        if config.number_nonblank {
            if !is_blank {
                let _ = write!(buf, "{:6}\t", line_num);
                *line_num += 1;
            }
        } else if config.number {
            let _ = write!(buf, "{:6}\t", line_num);
            *line_num += 1;
        }

        // Process line content
        if config.show_nonprinting || config.show_tabs {
            let content_end = if line.last() == Some(&b'\n') {
                line.len() - 1
            } else {
                line.len()
            };

            for &b in &line[..content_end] {
                if config.show_nonprinting {
                    write_nonprinting(b, config.show_tabs, &mut buf);
                } else if config.show_tabs && b == b'\t' {
                    buf.extend_from_slice(b"^I");
                } else {
                    buf.push(b);
                }
            }

            if config.show_ends && line.last() == Some(&b'\n') {
                buf.push(b'$');
            }
            if line.last() == Some(&b'\n') {
                buf.push(b'\n');
            }
        } else {
            // No character transformation needed
            if config.show_ends {
                let content_end = if line.last() == Some(&b'\n') {
                    line.len() - 1
                } else {
                    line.len()
                };
                buf.extend_from_slice(&line[..content_end]);
                if line.last() == Some(&b'\n') {
                    buf.push(b'$');
                    buf.push(b'\n');
                }
            } else {
                buf.extend_from_slice(line);
            }
        }

        // Flush buffer periodically to avoid excessive memory use
        if buf.len() >= 8 * 1024 * 1024 {
            out.write_all(&buf)?;
            buf.clear();
        }

        pos = line_end;
    }

    if !buf.is_empty() {
        out.write_all(&buf)?;
    }

    Ok(())
}

/// Process a single file for cat
pub fn cat_file(
    filename: &str,
    config: &CatConfig,
    line_num: &mut u64,
    out: &mut impl Write,
    tool_name: &str,
) -> io::Result<bool> {
    if filename == "-" {
        if config.is_plain() {
            match cat_plain_stdin(out) {
                Ok(()) => return Ok(true),
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!(
                        "{}: standard input: {}",
                        tool_name,
                        crate::common::io_error_msg(&e)
                    );
                    return Ok(false);
                }
            }
        }
        match read_stdin() {
            Ok(data) => {
                cat_with_options(&data, config, line_num, out)?;
                Ok(true)
            }
            Err(e) => {
                eprintln!(
                    "{}: standard input: {}",
                    tool_name,
                    crate::common::io_error_msg(&e)
                );
                Ok(false)
            }
        }
    } else {
        let path = Path::new(filename);

        // Check if it's a directory
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {
                eprintln!("{}: {}: Is a directory", tool_name, filename);
                return Ok(false);
            }
            _ => {}
        }

        if config.is_plain() {
            match cat_plain_file(path, out) {
                Ok(true) => return Ok(true),
                Ok(false) => {} // fall through
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!(
                        "{}: {}: {}",
                        tool_name,
                        filename,
                        crate::common::io_error_msg(&e)
                    );
                    return Ok(false);
                }
            }
        }

        match read_file(path) {
            Ok(data) => {
                cat_with_options(&data, config, line_num, out)?;
                Ok(true)
            }
            Err(e) => {
                eprintln!(
                    "{}: {}: {}",
                    tool_name,
                    filename,
                    crate::common::io_error_msg(&e)
                );
                Ok(false)
            }
        }
    }
}
