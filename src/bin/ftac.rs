use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

use clap::Parser;

use coreutils_rs::tac;

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

/// Memory-mapped file data (either mmap or owned Vec).
enum TacData {
    Mmap(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for TacData {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            TacData::Mmap(m) => m,
            TacData::Owned(v) => v,
        }
    }
}

/// Read file for tac: mmap WITH MAP_POPULATE for backward scan.
/// MAP_POPULATE pre-creates all page table entries, avoiding ~35K minor page faults
/// during backward memrchr scan. MADV_SEQUENTIAL + HUGEPAGE for large files.
fn read_file_for_tac(path: &Path) -> io::Result<TacData> {
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    let len = metadata.len();

    if len == 0 || !metadata.file_type().is_file() {
        if len > 0 {
            let mut buf = Vec::new();
            let mut reader = file;
            io::Read::read_to_end(&mut reader, &mut buf)?;
            return Ok(TacData::Owned(buf));
        }
        return Ok(TacData::Owned(Vec::new()));
    }

    // Small files (< 1MB): direct read is faster than mmap
    if len < 1024 * 1024 {
        let mut buf = vec![0u8; len as usize];
        let mut total = 0;
        let mut reader = &file;
        while total < buf.len() {
            match io::Read::read(&mut reader, &mut buf[total..]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        buf.truncate(total);
        return Ok(TacData::Owned(buf));
    }

    // Large files: mmap WITH populate for backward scan.
    match unsafe { memmap2::MmapOptions::new().populate().map(&file) } {
        Ok(mmap) => {
            #[cfg(target_os = "linux")]
            {
                let _ = mmap.advise(memmap2::Advice::Sequential);
                if len >= 2 * 1024 * 1024 {
                    let _ = mmap.advise(memmap2::Advice::HugePage);
                }
            }
            Ok(TacData::Mmap(mmap))
        }
        Err(_) => {
            let mut buf = Vec::with_capacity(len as usize);
            let mut reader = file;
            io::Read::read_to_end(&mut reader, &mut buf)?;
            Ok(TacData::Owned(buf))
        }
    }
}

/// Try to mmap stdin if it's a regular file (e.g., shell redirect `< file`).
#[cfg(unix)]
fn try_mmap_stdin() -> Option<memmap2::Mmap> {
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

    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mmap = unsafe { memmap2::MmapOptions::new().populate().map(&file) }.ok();
    std::mem::forget(file);
    #[cfg(target_os = "linux")]
    if let Some(ref m) = mmap {
        let _ = m.advise(memmap2::Advice::Sequential);
    }
    mmap
}

fn read_stdin_data() -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    io::stdin().lock().read_to_end(&mut buf)?;
    Ok(buf)
}

fn run(cli: &Cli, files: &[String], out: &mut impl Write) -> bool {
    let mut had_error = false;

    for filename in files {
        let data: TacData = if filename == "-" {
            #[cfg(unix)]
            {
                match try_mmap_stdin() {
                    Some(mmap) => TacData::Mmap(mmap),
                    None => match read_stdin_data() {
                        Ok(d) => TacData::Owned(d),
                        Err(e) => {
                            eprintln!("tac: standard input: {}", e);
                            had_error = true;
                            continue;
                        }
                    },
                }
            }
            #[cfg(not(unix))]
            match read_stdin_data() {
                Ok(d) => TacData::Owned(d),
                Err(e) => {
                    eprintln!("tac: standard input: {}", e);
                    had_error = true;
                    continue;
                }
            }
        } else {
            match read_file_for_tac(Path::new(filename)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("tac: {}: {}", filename, e);
                    had_error = true;
                    continue;
                }
            }
        };

        let bytes: &[u8] = &data;

        let result = if cli.regex {
            let sep = cli.separator.as_deref().unwrap_or("\n");
            tac::tac_regex_separator(bytes, sep, cli.before, out)
        } else if let Some(ref sep) = cli.separator {
            tac::tac_string_separator(bytes, sep.as_bytes(), cli.before, out)
        } else {
            tac::tac_bytes(bytes, b'\n', cli.before, out)
        };

        if let Err(e) = result {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("tac: write error: {}", e);
            had_error = true;
        }
    }

    had_error
}

fn main() {
    let cli = Cli::parse();

    let files: Vec<String> = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
    };

    // Write directly to raw fd WITHOUT BufWriter.
    // BufWriter copies all data through its internal buffer, destroying
    // the zero-copy writev approach of tac core (~70ms wasted for 141MB).
    #[cfg(unix)]
    let had_error = {
        use std::mem::ManuallyDrop;
        let raw_file = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
        let mut writer: &std::fs::File = &raw_file;
        let err = run(&cli, &files, &mut writer);
        let _ = writer.flush();
        err
    };
    #[cfg(not(unix))]
    let had_error = {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        run(&cli, &files, &mut lock)
    };

    if had_error {
        process::exit(1);
    }
}
