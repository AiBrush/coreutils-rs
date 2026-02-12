use std::process;

use clap::Parser;

use coreutils_rs::common::io_error_msg;
use coreutils_rs::sort::{
    CheckMode, KeyDef, KeyOpts, SortConfig, parse_buffer_size, sort_and_output,
};

#[derive(Parser)]
#[command(name = "sort", about = "Sort lines of text files")]
struct Cli {
    /// Ignore leading blanks
    #[arg(short = 'b', long = "ignore-leading-blanks")]
    ignore_leading_blanks: bool,

    /// Consider only blanks and alphanumeric characters
    #[arg(short = 'd', long = "dictionary-order")]
    dictionary_order: bool,

    /// Fold lower case to upper case characters
    #[arg(short = 'f', long = "ignore-case")]
    ignore_case: bool,

    /// Compare according to general numerical value
    #[arg(short = 'g', long = "general-numeric-sort")]
    general_numeric: bool,

    /// Compare human readable numbers (e.g., 2K 1G)
    #[arg(short = 'h', long = "human-numeric-sort")]
    human_numeric: bool,

    /// Consider only printable characters
    #[arg(short = 'i', long = "ignore-nonprinting")]
    ignore_nonprinting: bool,

    /// Compare (unknown) < 'JAN' < ... < 'DEC'
    #[arg(short = 'M', long = "month-sort")]
    month_sort: bool,

    /// Compare according to string numerical value
    #[arg(short = 'n', long = "numeric-sort")]
    numeric_sort: bool,

    /// Shuffle, but group identical keys
    #[arg(short = 'R', long = "random-sort")]
    random_sort: bool,

    /// Reverse the result of comparisons
    #[arg(short = 'r', long = "reverse")]
    reverse: bool,

    /// Natural sort of (version) numbers within text
    #[arg(short = 'V', long = "version-sort")]
    version_sort: bool,

    /// Sort via a key; KEYDEF gives location and type
    #[arg(short = 'k', long = "key", value_name = "KEYDEF")]
    keys: Vec<String>,

    /// Use SEP instead of non-blank to blank transition
    #[arg(short = 't', long = "field-separator", value_name = "SEP")]
    field_separator: Option<String>,

    /// Output only the first of an equal run
    #[arg(short = 'u', long = "unique")]
    unique: bool,

    /// Stabilize sort by disabling last-resort comparison
    #[arg(short = 's', long = "stable")]
    stable: bool,

    /// Check for sorted input; do not sort
    #[arg(short = 'c', long = "check", default_missing_value = "diagnose", num_args = 0..=1, require_equals = true)]
    check: Option<String>,

    /// Like -c, but do not report first bad line
    #[arg(short = 'C')]
    check_quiet: bool,

    /// Merge already sorted files; do not sort
    #[arg(short = 'm', long = "merge")]
    merge: bool,

    /// Write result to FILE instead of standard output
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    output: Option<String>,

    /// Use DIR for temporaries, not $TMPDIR or /tmp
    #[arg(short = 'T', long = "temporary-directory", value_name = "DIR")]
    temp_dir: Option<String>,

    /// Change the number of sorts run concurrently to N
    #[arg(long = "parallel", value_name = "N")]
    parallel: Option<usize>,

    /// Use SIZE for main memory buffer
    #[arg(short = 'S', long = "buffer-size", value_name = "SIZE")]
    buffer_size: Option<String>,

    /// Line delimiter is NUL, not newline
    #[arg(short = 'z', long = "zero-terminated")]
    zero_terminated: bool,

    /// Files to sort
    files: Vec<String>,
}

fn main() {
    coreutils_rs::common::reset_sigpipe();
    let cli = Cli::parse();

    // Parse key definitions
    let mut keys: Vec<KeyDef> = Vec::new();
    for key_spec in &cli.keys {
        match KeyDef::parse(key_spec) {
            Ok(k) => keys.push(k),
            Err(e) => {
                eprintln!("sort: invalid key specification '{}': {}", key_spec, e);
                process::exit(2);
            }
        }
    }

    // Parse field separator
    let separator = cli.field_separator.as_ref().map(|s| {
        if s.len() == 1 {
            s.as_bytes()[0]
        } else if s == "\\0" {
            b'\0'
        } else if s == "\\t" {
            b'\t'
        } else {
            eprintln!("sort: multi-character tab '{}'", s);
            process::exit(2);
        }
    });

    // Build global options
    let global_opts = KeyOpts {
        ignore_leading_blanks: cli.ignore_leading_blanks,
        dictionary_order: cli.dictionary_order,
        ignore_case: cli.ignore_case,
        general_numeric: cli.general_numeric,
        human_numeric: cli.human_numeric,
        ignore_nonprinting: cli.ignore_nonprinting,
        month: cli.month_sort,
        numeric: cli.numeric_sort,
        random: cli.random_sort,
        version: cli.version_sort,
        reverse: cli.reverse,
    };

    // Determine check mode
    let check = if cli.check_quiet {
        CheckMode::Quiet
    } else if let Some(ref val) = cli.check {
        match val.as_str() {
            "quiet" | "silent" => CheckMode::Quiet,
            _ => CheckMode::Diagnose,
        }
    } else {
        CheckMode::None
    };

    // Parse buffer size
    let buffer_size = cli.buffer_size.as_ref().map(|s| {
        parse_buffer_size(s).unwrap_or_else(|e| {
            eprintln!("sort: invalid buffer size: {}", e);
            process::exit(2);
        })
    });

    let random_seed = if cli.random_sort {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(42)
    } else {
        0
    };

    let config = SortConfig {
        keys,
        separator,
        global_opts,
        unique: cli.unique,
        stable: cli.stable,
        reverse: cli.reverse,
        check,
        merge: cli.merge,
        output_file: cli.output,
        zero_terminated: cli.zero_terminated,
        parallel: cli.parallel,
        buffer_size,
        temp_dir: cli.temp_dir,
        random_seed,
    };

    let inputs = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files
    };

    if let Err(e) = sort_and_output(&inputs, &config) {
        // SIGPIPE is reset to SIG_DFL, so broken pipe kills the process
        // before we get here. This is a fallback for edge cases.
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            process::exit(2);
        }
        eprintln!("sort: {}", io_error_msg(&e));
        process::exit(2);
    }
}
