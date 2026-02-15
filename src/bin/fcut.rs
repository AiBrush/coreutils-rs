#[cfg(not(target_os = "linux"))]
use std::io::BufWriter;
use std::io::{self, BufReader, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

#[cfg(unix)]
use memmap2::MmapOptions;

use coreutils_rs::common::io::read_file_mmap;
use coreutils_rs::common::io_error_msg;
use coreutils_rs::cut::{self, CutMode};

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
        // SAFETY: IoSlice is #[repr(transparent)] over iovec on Unix,
        // so &[IoSlice] has the same memory layout as &[iovec].
        // Direct pointer cast eliminates Vec allocation + copy per call.
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
    bytes: Option<String>,
    characters: Option<String>,
    fields: Option<String>,
    delimiter: Option<String>,
    complement: bool,
    only_delimited: bool,
    output_delimiter: Option<String>,
    zero_terminated: bool,
    files: Vec<String>,
}

/// Hand-rolled argument parser — eliminates clap's ~100-200µs initialization.
/// cut's args: -b, -c, -f (with LIST), -d (with DELIM), -s, -z, -n, --complement,
/// --output-delimiter, and positional files.
fn parse_args() -> Cli {
    let mut cli = Cli {
        bytes: None,
        characters: None,
        fields: None,
        delimiter: None,
        complement: false,
        only_delimited: false,
        output_delimiter: None,
        zero_terminated: false,
        files: Vec::new(),
    };

    let mut args = std::env::args_os().skip(1);
    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            // Everything after -- is positional
            for a in args {
                cli.files.push(a.to_string_lossy().into_owned());
            }
            break;
        }
        if bytes.starts_with(b"--") {
            // Long options
            if bytes.starts_with(b"--bytes=") {
                cli.bytes = Some(std::str::from_utf8(&bytes[8..]).unwrap_or("").to_string());
            } else if bytes.starts_with(b"--characters=") {
                cli.characters = Some(std::str::from_utf8(&bytes[13..]).unwrap_or("").to_string());
            } else if bytes.starts_with(b"--fields=") {
                cli.fields = Some(std::str::from_utf8(&bytes[9..]).unwrap_or("").to_string());
            } else if bytes.starts_with(b"--delimiter=") {
                cli.delimiter = Some(std::str::from_utf8(&bytes[12..]).unwrap_or("").to_string());
            } else if bytes.starts_with(b"--output-delimiter=") {
                cli.output_delimiter =
                    Some(std::str::from_utf8(&bytes[19..]).unwrap_or("").to_string());
            } else {
                match bytes {
                    b"--bytes" => {
                        if let Some(v) = args.next() {
                            cli.bytes = Some(v.to_string_lossy().into_owned());
                        } else {
                            eprintln!("cut: option '--bytes' requires an argument");
                            process::exit(1);
                        }
                    }
                    b"--characters" => {
                        if let Some(v) = args.next() {
                            cli.characters = Some(v.to_string_lossy().into_owned());
                        } else {
                            eprintln!("cut: option '--characters' requires an argument");
                            process::exit(1);
                        }
                    }
                    b"--fields" => {
                        if let Some(v) = args.next() {
                            cli.fields = Some(v.to_string_lossy().into_owned());
                        } else {
                            eprintln!("cut: option '--fields' requires an argument");
                            process::exit(1);
                        }
                    }
                    b"--delimiter" => {
                        if let Some(v) = args.next() {
                            cli.delimiter = Some(v.to_string_lossy().into_owned());
                        } else {
                            eprintln!("cut: option '--delimiter' requires an argument");
                            process::exit(1);
                        }
                    }
                    b"--output-delimiter" => {
                        if let Some(v) = args.next() {
                            cli.output_delimiter = Some(v.to_string_lossy().into_owned());
                        } else {
                            eprintln!("cut: option '--output-delimiter' requires an argument");
                            process::exit(1);
                        }
                    }
                    b"--complement" => cli.complement = true,
                    b"--only-delimited" => cli.only_delimited = true,
                    b"--zero-terminated" => cli.zero_terminated = true,
                    b"--help" => {
                        print!(
                            "Usage: cut OPTION... [FILE]...\n\
                            Print selected parts of lines from each FILE to standard output.\n\n\
                            With no FILE, or when FILE is -, read standard input.\n\n\
                            Mandatory arguments to long options are mandatory for short options too.\n\
                            \x20 -b, --bytes=LIST        select only these bytes\n\
                            \x20 -c, --characters=LIST   select only these characters\n\
                            \x20 -d, --delimiter=DELIM   use DELIM instead of TAB for field delimiter\n\
                            \x20 -f, --fields=LIST       select only these fields;  also print any line\n\
                            \x20                           that contains no delimiter character, unless\n\
                            \x20                           the -s option is specified\n\
                            \x20 -n                       (ignored)\n\
                            \x20     --complement         complement the set of selected bytes, characters\n\
                            \x20                           or fields\n\
                            \x20 -s, --only-delimited     do not print lines not containing delimiters\n\
                            \x20     --output-delimiter=STRING  use STRING as the output delimiter\n\
                            \x20                           the default is to use the input delimiter\n\
                            \x20 -z, --zero-terminated    line delimiter is NUL, not newline\n\
                            \x20     --help               display this help and exit\n\
                            \x20     --version            output version information and exit\n"
                        );
                        process::exit(0);
                    }
                    b"--version" => {
                        println!("cut (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                        process::exit(0);
                    }
                    _ => {
                        eprintln!("cut: unrecognized option '{}'", arg.to_string_lossy());
                        eprintln!("Try 'cut --help' for more information.");
                        process::exit(1);
                    }
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            // Short options: can be combined (-sf1-3 means -s -f 1-3)
            let mut i = 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'n' => {} // ignored (POSIX compat)
                    b's' => cli.only_delimited = true,
                    b'z' => cli.zero_terminated = true,
                    b'b' | b'c' | b'd' | b'f' => {
                        // These take a value: rest of arg or next arg
                        let flag = bytes[i];
                        let val = if i + 1 < bytes.len() {
                            // Value attached: -b1-3, -d:
                            std::str::from_utf8(&bytes[i + 1..])
                                .unwrap_or("")
                                .to_string()
                        } else if let Some(v) = args.next() {
                            v.to_string_lossy().into_owned()
                        } else {
                            eprintln!("cut: option requires an argument -- '{}'", flag as char);
                            process::exit(1);
                        };
                        match flag {
                            b'b' => cli.bytes = Some(val),
                            b'c' => cli.characters = Some(val),
                            b'd' => cli.delimiter = Some(val),
                            b'f' => cli.fields = Some(val),
                            _ => unreachable!(),
                        }
                        // Skip remaining bytes since they were consumed as value
                        i = bytes.len();
                        continue;
                    }
                    _ => {
                        eprintln!("cut: invalid option -- '{}'", bytes[i] as char);
                        eprintln!("Try 'cut --help' for more information.");
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

    let file_size = stat.st_size as usize;
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    // MAP_POPULATE for files >= 4MB to prefault pages; lazy for smaller files
    let mmap = if file_size >= 4 * 1024 * 1024 {
        unsafe { MmapOptions::new().populate().map(&file) }.ok()
    } else {
        unsafe { MmapOptions::new().map(&file) }.ok()
    };
    std::mem::forget(file); // Don't close stdin
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

    // Pre-warm rayon's global thread pool in background (opportunistic).
    // Overlaps pool creation (~200-500µs) with arg parsing and file I/O.
    // Race with first rayon::scope() is safe: build_global() is internally
    // synchronized — whichever call wins initializes the pool, the other is a no-op.
    std::thread::spawn(|| {
        let _ = rayon::ThreadPoolBuilder::new().build_global();
    });

    #[cfg(target_os = "linux")]
    enlarge_pipes();

    let cli = parse_args();

    // Determine mode
    let mode_count =
        cli.bytes.is_some() as u8 + cli.characters.is_some() as u8 + cli.fields.is_some() as u8;
    if mode_count == 0 {
        eprintln!("cut: you must specify a list of bytes, characters, or fields");
        eprintln!("Try 'cut --help' for more information.");
        process::exit(1);
    }
    if mode_count > 1 {
        eprintln!("cut: only one type of list may be specified");
        eprintln!("Try 'cut --help' for more information.");
        process::exit(1);
    }

    let (mode, spec) = if let Some(ref s) = cli.bytes {
        (CutMode::Bytes, s.as_str())
    } else if let Some(ref s) = cli.characters {
        (CutMode::Characters, s.as_str())
    } else {
        (CutMode::Fields, cli.fields.as_ref().unwrap().as_str())
    };

    let ranges = match cut::parse_ranges(spec) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cut: {}", e);
            process::exit(1);
        }
    };

    let delim = if let Some(ref d) = cli.delimiter {
        if d.len() != 1 {
            eprintln!("cut: the delimiter must be a single character");
            eprintln!("Try 'cut --help' for more information.");
            process::exit(1);
        }
        d.as_bytes()[0]
    } else {
        b'\t'
    };

    // Default output delimiter: field delimiter for -f, empty for -b/-c
    // GNU cut only uses a delimiter between fields, not between byte/char ranges
    let output_delim = if let Some(ref od) = cli.output_delimiter {
        od.as_bytes().to_vec()
    } else if mode == CutMode::Fields {
        vec![delim]
    } else {
        vec![]
    };

    let line_delim = if cli.zero_terminated { b'\0' } else { b'\n' };

    let files = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
    };

    // On Linux: VmspliceWriter directly — no BufWriter wrapper.
    // cut's batch processing already produces large output buffers (Vec<u8> or IoSlice),
    // so BufWriter's internal buffering is pure overhead (extra memcpy for output < 16MB).
    #[cfg(target_os = "linux")]
    let mut out = VmspliceWriter::new();
    // On other Unix: raw fd stdout with BufWriter
    #[cfg(all(unix, not(target_os = "linux")))]
    let mut raw = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
    #[cfg(all(unix, not(target_os = "linux")))]
    let mut out = BufWriter::with_capacity(16 * 1024 * 1024, &mut *raw);
    #[cfg(not(unix))]
    let stdout = io::stdout();
    #[cfg(not(unix))]
    let mut out = BufWriter::with_capacity(16 * 1024 * 1024, stdout.lock());
    let mut had_error = false;

    let cfg = cut::CutConfig {
        mode,
        ranges: &ranges,
        complement: cli.complement,
        delim,
        output_delim: &output_delim,
        suppress_no_delim: cli.only_delimited,
        line_delim,
    };

    // Try to mmap stdin for zero-copy (only used if stdin is a regular file)
    #[cfg(unix)]
    let stdin_mmap = {
        if files.iter().any(|f| f == "-") {
            try_mmap_stdin()
        } else {
            None
        }
    };

    // Pre-read all stdin data for piped input.
    // On Linux: try splice+memfd for zero-copy (kernel→kernel), fallback to read_stdin.
    // splice avoids the kernel→userspace copy, saving ~0.5ms for 10MB.
    #[cfg(target_os = "linux")]
    let splice_mmap: Option<memmap2::MmapMut> =
        if stdin_mmap.is_none() && files.iter().any(|f| f == "-") {
            coreutils_rs::common::io::splice_stdin_to_mmap().unwrap_or(None)
        } else {
            None
        };
    #[cfg(not(target_os = "linux"))]
    let splice_mmap: Option<memmap2::MmapMut> = None;

    #[allow(unused_variables)]
    let has_splice = splice_mmap.is_some();

    #[cfg(unix)]
    let mut stdin_buf: Option<Vec<u8>> =
        if stdin_mmap.is_none() && !has_splice && files.iter().any(|f| f == "-") {
            match coreutils_rs::common::io::read_stdin() {
                Ok(buf) => Some(buf),
                Err(e) => {
                    if e.kind() != io::ErrorKind::BrokenPipe {
                        eprintln!("cut: {}", io_error_msg(&e));
                        process::exit(1);
                    }
                    Some(Vec::new())
                }
            }
        } else {
            None
        };
    #[cfg(not(unix))]
    let mut stdin_buf: Option<Vec<u8>> = if files.iter().any(|f| f == "-") {
        match coreutils_rs::common::io::read_stdin() {
            Ok(buf) => Some(buf),
            Err(e) => {
                if e.kind() != io::ErrorKind::BrokenPipe {
                    eprintln!("cut: {}", io_error_msg(&e));
                    process::exit(1);
                }
                Some(Vec::new())
            }
        }
    } else {
        None
    };

    // For piped stdin with mutable data: try in-place extraction.
    // In-place avoids allocating intermediate Vec output buffers entirely.
    // Generalizes the old field-1-only path to all field and byte patterns.
    let mut stdin_inplace_done = false;

    // Try in-place on splice_mmap first (Linux: zero-copy pipe→memfd→mmap)
    #[cfg(target_os = "linux")]
    let mut splice_mmap = splice_mmap;
    #[cfg(target_os = "linux")]
    let mut splice_inplace_len: usize = 0;
    #[cfg(target_os = "linux")]
    if let Some(ref mut mmap_data) = splice_mmap
        && !mmap_data.is_empty()
        && let Some(new_len) = cut::process_cut_data_mut(mmap_data, &cfg)
    {
        splice_inplace_len = new_len;
        stdin_inplace_done = true;
    }

    // Try in-place on stdin_buf (Vec<u8>) if splice didn't handle it
    if !stdin_inplace_done
        && let Some(ref mut data) = stdin_buf
        && !data.is_empty()
        && let Some(new_len) = cut::process_cut_data_mut(data, &cfg)
    {
        data.truncate(new_len);
        stdin_inplace_done = true;
    }

    for filename in &files {
        let result: io::Result<()> = if filename == "-" {
            #[cfg(unix)]
            {
                if stdin_inplace_done {
                    // Write in-place processed data directly to output
                    #[cfg(target_os = "linux")]
                    {
                        if splice_inplace_len > 0 {
                            if let Some(ref mmap_data) = splice_mmap {
                                out.write_all(&mmap_data[..splice_inplace_len])
                            } else {
                                Ok(())
                            }
                        } else if let Some(ref data) = stdin_buf {
                            out.write_all(data)
                        } else {
                            Ok(())
                        }
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        if let Some(ref data) = stdin_buf {
                            out.flush().and_then(|()| out.get_mut().write_all(data))
                        } else {
                            Ok(())
                        }
                    }
                } else if let Some(ref data) = stdin_mmap {
                    cut::process_cut_data(data, &cfg, &mut out)
                } else {
                    // Try splice mmap first (Linux only), then stdin_buf
                    #[cfg(target_os = "linux")]
                    if let Some(ref data) = splice_mmap {
                        cut::process_cut_data(data.as_ref(), &cfg, &mut out)
                    } else if let Some(ref data) = stdin_buf {
                        cut::process_cut_data(data, &cfg, &mut out)
                    } else {
                        let reader = BufReader::new(io::stdin().lock());
                        cut::process_cut_reader(reader, &cfg, &mut out)
                    }
                    #[cfg(not(target_os = "linux"))]
                    if let Some(ref data) = stdin_buf {
                        cut::process_cut_data(data, &cfg, &mut out)
                    } else {
                        let reader = BufReader::new(io::stdin().lock());
                        cut::process_cut_reader(reader, &cfg, &mut out)
                    }
                }
            }
            #[cfg(not(unix))]
            {
                if stdin_inplace_done {
                    if let Some(ref data) = stdin_buf {
                        out.write_all(data)
                    } else {
                        Ok(())
                    }
                } else if let Some(ref data) = stdin_buf {
                    cut::process_cut_data(data, &cfg, &mut out)
                } else {
                    let reader = BufReader::new(io::stdin().lock());
                    cut::process_cut_reader(reader, &cfg, &mut out)
                }
            }
        } else {
            match read_file_mmap(Path::new(filename)) {
                Ok(data) => cut::process_cut_data(&data, &cfg, &mut out),
                Err(e) => {
                    eprintln!("cut: {}: {}", filename, io_error_msg(&e));
                    had_error = true;
                    continue;
                }
            }
        };

        if let Err(e) = result {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("cut: write error: {}", io_error_msg(&e));
            had_error = true;
        }
    }

    if let Err(e) = out.flush() {
        if e.kind() == io::ErrorKind::BrokenPipe {
            process::exit(0);
        }
        eprintln!("cut: write error: {}", io_error_msg(&e));
        had_error = true;
    }

    if had_error {
        process::exit(1);
    }
}
