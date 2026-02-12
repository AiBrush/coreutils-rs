use std::io::{self, BufReader, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

use clap::Parser;
#[cfg(unix)]
use memmap2::MmapOptions;

use coreutils_rs::common::io::read_file;
use coreutils_rs::common::io_error_msg;
use coreutils_rs::cut::{self, CutMode};

#[derive(Parser)]
#[command(name = "cut", about = "Remove sections from each line of files")]
struct Cli {
    /// Select only these bytes
    #[arg(short = 'b', long = "bytes", value_name = "LIST")]
    bytes: Option<String>,

    /// Select only these characters
    #[arg(short = 'c', long = "characters", value_name = "LIST")]
    characters: Option<String>,

    /// Select only these fields
    #[arg(short = 'f', long = "fields", value_name = "LIST")]
    fields: Option<String>,

    /// Use DELIM instead of TAB for field delimiter
    #[arg(short = 'd', long = "delimiter", value_name = "DELIM")]
    delimiter: Option<String>,

    /// Complement the set of selected bytes, characters, or fields
    #[arg(long = "complement")]
    complement: bool,

    /// Do not print lines not containing delimiters
    #[arg(short = 's', long = "only-delimited")]
    only_delimited: bool,

    /// Use STRING as the output delimiter
    #[arg(long = "output-delimiter", value_name = "STRING")]
    output_delimiter: Option<String>,

    /// Line delimiter is NUL, not newline
    #[arg(short = 'z', long = "zero-terminated")]
    zero_terminated: bool,

    /// (ignored, for historical compatibility)
    #[arg(short = 'n', hide = true)]
    _legacy_n: bool,

    /// Files to process
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
    let mmap = unsafe { MmapOptions::new().map(&file) }.ok();
    std::mem::forget(file); // Don't close stdin
    #[cfg(target_os = "linux")]
    if let Some(ref m) = mmap {
        unsafe {
            libc::madvise(
                m.as_ptr() as *mut libc::c_void,
                m.len(),
                libc::MADV_SEQUENTIAL,
            );
        }
    }
    mmap
}

fn main() {
    coreutils_rs::common::reset_sigpipe();
    let cli = Cli::parse();

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

    // Raw fd stdout on Unix for zero-overhead writes
    #[cfg(unix)]
    let mut raw = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
    #[cfg(unix)]
    let mut out = BufWriter::with_capacity(4 * 1024 * 1024, &mut *raw);
    #[cfg(not(unix))]
    let stdout = io::stdout();
    #[cfg(not(unix))]
    let mut out = BufWriter::with_capacity(4 * 1024 * 1024, stdout.lock());
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

    for filename in &files {
        let result: io::Result<()> = if filename == "-" {
            #[cfg(unix)]
            {
                if let Some(ref data) = stdin_mmap {
                    cut::process_cut_data(data, &cfg, &mut out)
                } else {
                    let reader = BufReader::new(io::stdin().lock());
                    cut::process_cut_reader(reader, &cfg, &mut out)
                }
            }
            #[cfg(not(unix))]
            {
                let reader = BufReader::new(io::stdin().lock());
                cut::process_cut_reader(reader, &cfg, &mut out)
            }
        } else {
            match read_file(Path::new(filename)) {
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
