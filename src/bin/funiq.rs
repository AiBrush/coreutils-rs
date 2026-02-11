use std::fs::File;
use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::process;

use clap::Parser;
use memmap2::MmapOptions;

use coreutils_rs::uniq::{
    AllRepeatedMethod, GroupMethod, OutputMode, UniqConfig, process_uniq, process_uniq_bytes,
};

#[derive(Parser)]
#[command(
    name = "funiq",
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
    let cli = Cli::parse();

    // Determine output mode
    let mode = if let Some(ref method_str) = cli.group {
        let method = match method_str.as_str() {
            "separate" => GroupMethod::Separate,
            "prepend" => GroupMethod::Prepend,
            "append" => GroupMethod::Append,
            "both" => GroupMethod::Both,
            other => {
                eprintln!("funiq: invalid argument '{}' for '--group'", other);
                eprintln!(
                    "Valid arguments are:\n  - 'separate'\n  - 'prepend'\n  - 'append'\n  - 'both'"
                );
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
            eprintln!("funiq: --group is mutually exclusive with -c/-d/-D/-u");
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
                    eprintln!("funiq: invalid argument '{}' for '--all-repeated'", other);
                    eprintln!("Valid arguments are:\n  - 'none'\n  - 'prepend'\n  - 'separate'");
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
        eprintln!("funiq: printing all duplicated lines and repeat counts is meaningless");
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
                eprintln!("funiq: {}: {}", path, e);
                process::exit(1);
            }
        };
        run_uniq(&cli, &config, output);
        return;
    }

    // Raw fd stdout on Unix for zero-overhead writes
    #[cfg(unix)]
    let mut raw = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
    #[cfg(unix)]
    let output = BufWriter::with_capacity(4 * 1024 * 1024, &mut *raw);
    #[cfg(not(unix))]
    let output = BufWriter::with_capacity(4 * 1024 * 1024, io::stdout().lock());

    run_uniq(&cli, &config, output);
}

fn run_uniq(cli: &Cli, config: &UniqConfig, output: impl Write) {
    let result = match cli.input.as_deref() {
        Some("-") | None => {
            // Stdin: use streaming mode
            process_uniq(io::stdin().lock(), output, config)
        }
        Some(path) => {
            // File: use mmap for zero-copy performance
            let file = match File::open(path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("funiq: {}: {}", path, e);
                    process::exit(1);
                }
            };
            let metadata = match file.metadata() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("funiq: {}: {}", path, e);
                    process::exit(1);
                }
            };

            if metadata.len() == 0 {
                // Empty file, nothing to do
                return;
            }

            // Use mmap for files — populate() for eager page table setup
            let mmap = match unsafe { MmapOptions::new().populate().map(&file) } {
                Ok(m) => {
                    #[cfg(target_os = "linux")]
                    {
                        let _ = m.advise(memmap2::Advice::Sequential);
                    }
                    m
                }
                Err(e) => {
                    eprintln!("funiq: {}: {}", path, e);
                    process::exit(1);
                }
            };

            process_uniq_bytes(&mmap, output, config)
        }
    };

    if let Err(e) = result {
        // Ignore broken pipe
        if e.kind() != io::ErrorKind::BrokenPipe {
            eprintln!("funiq: {}", e);
            process::exit(1);
        }
    }
}
