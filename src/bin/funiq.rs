use std::fs::File;
use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::process;

use clap::Parser;
use memmap2::MmapOptions;

use coreutils_rs::common::io_error_msg;
use coreutils_rs::uniq::{
    AllRepeatedMethod, GroupMethod, OutputMode, UniqConfig, process_uniq, process_uniq_bytes,
};

#[derive(Parser)]
#[command(
    name = "uniq",
    about = "Report or omit repeated lines",
    after_help = "A field is a run of blanks (usually spaces and/or TABs), then non-blank \
                  characters. Fields are skipped before chars.\n\n\
                  Note: 'uniq' does not detect repeated lines unless they are adjacent.\n\
                  You may want to sort the input first, or use 'sort -u' without 'uniq'."
)]
struct Cli {
    /// Prefix lines by the number of occurrences
    #[arg(short = 'c', long = "count")]
    count: bool,

    /// Only print duplicate lines, one for each group
    #[arg(short = 'd', long = "repeated")]
    repeated: bool,

    /// Print all duplicate lines
    #[arg(short = 'D', overrides_with = "all_repeated")]
    all_duplicates: bool,

    /// Print all duplicate lines, delimited with METHOD (none, prepend, separate)
    #[arg(
        long = "all-repeated",
        value_name = "METHOD",
        num_args = 0..=1,
        default_missing_value = "none",
        require_equals = true
    )]
    all_repeated: Option<String>,

    /// Avoid comparing the first N fields
    #[arg(
        short = 'f',
        long = "skip-fields",
        value_name = "N",
        default_value = "0"
    )]
    skip_fields: usize,

    /// Show all items, delimited by empty line (separate, prepend, append, both)
    #[arg(
        long = "group",
        value_name = "METHOD",
        num_args = 0..=1,
        default_missing_value = "separate",
        require_equals = true
    )]
    group: Option<String>,

    /// Ignore differences in case when comparing
    #[arg(short = 'i', long = "ignore-case")]
    ignore_case: bool,

    /// Avoid comparing the first N characters
    #[arg(
        short = 's',
        long = "skip-chars",
        value_name = "N",
        default_value = "0"
    )]
    skip_chars: usize,

    /// Only print unique lines
    #[arg(short = 'u', long = "unique")]
    unique: bool,

    /// Compare no more than N characters in lines
    #[arg(short = 'w', long = "check-chars", value_name = "N")]
    check_chars: Option<usize>,

    /// Line delimiter is NUL, not newline
    #[arg(short = 'z', long = "zero-terminated")]
    zero_terminated: bool,

    /// Input file (default: stdin)
    input: Option<String>,

    /// Output file (default: stdout)
    output: Option<String>,
}

fn main() {
    coreutils_rs::common::reset_sigpipe();
    let cli = Cli::parse();

    // Determine output mode
    let mode = if let Some(ref method_str) = cli.group {
        let method = match method_str.as_str() {
            "separate" => GroupMethod::Separate,
            "prepend" => GroupMethod::Prepend,
            "append" => GroupMethod::Append,
            "both" => GroupMethod::Both,
            other => {
                eprintln!("uniq: invalid argument '{}' for '--group'", other);
                eprintln!(
                    "Valid arguments are:\n  - 'separate'\n  - 'prepend'\n  - 'append'\n  - 'both'"
                );
                eprintln!("Try 'uniq --help' for more information.");
                process::exit(1);
            }
        };
        // --group is incompatible with -c, -d, -D, -u
        if cli.count
            || cli.repeated
            || cli.all_duplicates
            || cli.all_repeated.is_some()
            || cli.unique
        {
            eprintln!("uniq: --group is mutually exclusive with -c/-d/-D/-u");
            eprintln!("Try 'uniq --help' for more information.");
            process::exit(1);
        }
        OutputMode::Group(method)
    } else if cli.all_duplicates || cli.all_repeated.is_some() {
        let method = if let Some(ref method_str) = cli.all_repeated {
            match method_str.as_str() {
                "none" => AllRepeatedMethod::None,
                "prepend" => AllRepeatedMethod::Prepend,
                "separate" => AllRepeatedMethod::Separate,
                other => {
                    eprintln!("uniq: invalid argument '{}' for '--all-repeated'", other);
                    eprintln!("Valid arguments are:\n  - 'none'\n  - 'prepend'\n  - 'separate'");
                    eprintln!("Try 'uniq --help' for more information.");
                    process::exit(1);
                }
            }
        } else {
            AllRepeatedMethod::None
        };
        OutputMode::AllRepeated(method)
    } else if cli.repeated {
        OutputMode::RepeatedOnly
    } else if cli.unique {
        OutputMode::UniqueOnly
    } else {
        OutputMode::Default
    };

    // -c is incompatible with -D/--all-repeated and --group
    if cli.count && matches!(mode, OutputMode::AllRepeated(_) | OutputMode::Group(_)) {
        eprintln!("uniq: printing all duplicated lines and repeat counts is meaningless");
        eprintln!("Try 'uniq --help' for more information.");
        process::exit(1);
    }

    let config = UniqConfig {
        mode,
        count: cli.count,
        ignore_case: cli.ignore_case,
        skip_fields: cli.skip_fields,
        skip_chars: cli.skip_chars,
        check_chars: cli.check_chars,
        zero_terminated: cli.zero_terminated,
    };

    // Dispatch to output file or stdout, avoiding Box<dyn Write> for stdout (common case)
    if let Some(ref path) = cli.output
        && path != "-"
    {
        let output = match File::create(path) {
            Ok(f) => BufWriter::new(f),
            Err(e) => {
                eprintln!("uniq: {}: {}", path, io_error_msg(&e));
                process::exit(1);
            }
        };
        run_uniq(&cli, &config, output);
        return;
    }

    // Raw fd stdout — process_uniq/process_uniq_bytes already wrap in BufWriter(16MB),
    // so we pass the raw fd directly to avoid double-buffering overhead.
    #[cfg(unix)]
    {
        let mut raw = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
        run_uniq(&cli, &config, &mut *raw);
    }
    #[cfg(not(unix))]
    {
        let stdout = io::stdout();
        run_uniq(&cli, &config, stdout.lock());
    }
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
            libc::madvise(
                m.as_ptr() as *mut libc::c_void,
                m.len(),
                libc::MADV_WILLNEED,
            );
        }
    }
    mmap
}

fn run_uniq(cli: &Cli, config: &UniqConfig, output: impl Write) {
    let result = match cli.input.as_deref() {
        Some("-") | None => {
            // Stdin: try mmap first (zero-copy for file redirects), fall back to streaming
            #[cfg(unix)]
            {
                match try_mmap_stdin() {
                    Some(mmap) => process_uniq_bytes(&mmap, output, config),
                    None => process_uniq(io::stdin().lock(), output, config),
                }
            }
            #[cfg(not(unix))]
            process_uniq(io::stdin().lock(), output, config)
        }
        Some(path) => {
            // File: use mmap for zero-copy performance
            let file = match File::open(path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("uniq: {}: {}", path, io_error_msg(&e));
                    process::exit(1);
                }
            };
            let metadata = match file.metadata() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("uniq: {}: {}", path, io_error_msg(&e));
                    process::exit(1);
                }
            };

            if metadata.len() == 0 {
                // Empty file, nothing to do
                return;
            }

            // Use mmap for files — MADV_SEQUENTIAL + WILLNEED + HUGEPAGE
            let mmap = match unsafe { MmapOptions::new().map(&file) } {
                Ok(m) => {
                    #[cfg(target_os = "linux")]
                    {
                        let _ = m.advise(memmap2::Advice::Sequential);
                        unsafe {
                            libc::madvise(
                                m.as_ptr() as *mut libc::c_void,
                                m.len(),
                                libc::MADV_WILLNEED,
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
                    m
                }
                Err(e) => {
                    eprintln!("uniq: {}: {}", path, io_error_msg(&e));
                    process::exit(1);
                }
            };

            process_uniq_bytes(&mmap, output, config)
        }
    };

    if let Err(e) = result {
        // Ignore broken pipe
        if e.kind() != io::ErrorKind::BrokenPipe {
            eprintln!("uniq: {}", io_error_msg(&e));
            process::exit(1);
        }
    }
}
