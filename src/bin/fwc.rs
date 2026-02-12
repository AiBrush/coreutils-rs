use std::io::{self, Write};
use std::path::Path;
use std::process;

use clap::Parser;

use coreutils_rs::common::io::{FileData, file_size, read_file, read_stdin};
use coreutils_rs::wc;

#[derive(Parser)]
#[command(
    name = "fwc",
    about = "Print newline, word, and byte counts for each FILE"
)]
struct Cli {
    /// Print the byte counts
    #[arg(short = 'c', long = "bytes")]
    bytes: bool,

    /// Print the character counts
    #[arg(short = 'm', long = "chars")]
    chars: bool,

    /// Print the newline counts
    #[arg(short = 'l', long = "lines")]
    lines: bool,

    /// Print the maximum display width
    #[arg(short = 'L', long = "max-line-length")]
    max_line_length: bool,

    /// Print the word counts
    #[arg(short = 'w', long = "words")]
    words: bool,

    /// Read input from the files specified by NUL-terminated names in file F
    #[arg(long = "files0-from", value_name = "F")]
    files0_from: Option<String>,

    /// When to print a line with total counts; WHEN can be: auto, always, never, only
    #[arg(long = "total", value_name = "WHEN", default_value = "auto")]
    total: String,

    /// Files to process (reads stdin if none given)
    files: Vec<String>,
}

/// Which fields to display.
struct ShowFlags {
    lines: bool,
    words: bool,
    bytes: bool,
    chars: bool,
    max_line_length: bool,
}

impl ShowFlags {
    /// True if only -c (bytes) is requested and nothing else needs file content.
    fn bytes_only(&self) -> bool {
        self.bytes && !self.lines && !self.words && !self.chars && !self.max_line_length
    }
}

/// Compute number of decimal digits needed to display a value.
/// Uses integer arithmetic to avoid floating-point precision issues.
fn num_width(n: u64) -> usize {
    if n == 0 {
        return 1;
    }
    let mut width = 0;
    let mut val = n;
    while val > 0 {
        val /= 10;
        width += 1;
    }
    width
}

fn main() {
    let cli = Cli::parse();

    // Detect locale once at startup
    let utf8_locale = wc::is_utf8_locale();

    // If no flags specified, default to lines + words + bytes.
    // If any flag is specified, only show the explicitly requested ones.
    let no_explicit = !cli.bytes && !cli.chars && !cli.words && !cli.lines && !cli.max_line_length;
    let show = ShowFlags {
        lines: cli.lines || no_explicit,
        words: cli.words || no_explicit,
        bytes: cli.bytes || no_explicit,
        chars: cli.chars,
        max_line_length: cli.max_line_length,
    };

    let total_mode = cli.total.as_str();

    // Collect files to process
    let files: Vec<String> = if let Some(ref f0f) = cli.files0_from {
        if !cli.files.is_empty() {
            eprintln!("fwc: extra operand '{}'", cli.files[0]);
            eprintln!("file operands cannot be combined with --files0-from");
            process::exit(1);
        }
        read_files0_from(f0f)
    } else if cli.files.is_empty() {
        vec!["-".to_string()] // stdin
    } else {
        cli.files.clone()
    };

    // Phase 1: Compute all counts
    let mut results: Vec<(wc::WcCounts, String)> = Vec::new();
    let mut total = wc::WcCounts::default();
    let mut had_error = false;
    let mut has_stdin = false;

    for filename in &files {
        if filename == "-" {
            has_stdin = true;
        }

        // Fast path: -c only on regular files — just stat, no read
        if show.bytes_only() && filename != "-" {
            match file_size(Path::new(filename)) {
                Ok(size) => {
                    let counts = wc::WcCounts {
                        bytes: size,
                        ..Default::default()
                    };
                    total.bytes += size;
                    results.push((counts, filename.clone()));
                    continue;
                }
                Err(e) => {
                    eprintln!("fwc: {}: {}", filename, e);
                    had_error = true;
                    continue;
                }
            }
        }

        // Read file data (zero-copy mmap for large files)
        let data: FileData = if filename == "-" {
            match read_stdin() {
                Ok(d) => FileData::Owned(d),
                Err(e) => {
                    eprintln!("fwc: standard input: {}", e);
                    had_error = true;
                    continue;
                }
            }
        } else {
            match read_file(Path::new(filename)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("fwc: {}: {}", filename, e);
                    had_error = true;
                    continue;
                }
            }
        };

        // Compute only the requested metrics — each uses its own optimized pass.
        // Uses parallel variants for large files (>4MB) to exploit multi-core CPUs.
        // Default mode uses a combined parallel pass (lines+words+chars together)
        // to keep data cache-warm between metric computations within each chunk.
        let counts = if show.lines && show.words && show.chars && !show.max_line_length {
            // Full parallel pass: lines + words + chars
            let (lines, words, chars) = wc::count_lwc_parallel(&data, utf8_locale);
            wc::WcCounts {
                lines,
                words,
                bytes: data.len() as u64,
                chars,
                max_line_length: 0,
            }
        } else if show.lines && show.words && !show.chars && !show.max_line_length {
            // Default mode: lines + words + bytes only (skip char counting)
            let (lines, words, bytes) = wc::count_lwb_parallel(&data, utf8_locale);
            wc::WcCounts {
                lines,
                words,
                bytes,
                chars: 0,
                max_line_length: 0,
            }
        } else {
            // Individual parallel passes for specific flags
            wc::WcCounts {
                lines: if show.lines {
                    wc::count_lines_parallel(&data)
                } else {
                    0
                },
                words: if show.words {
                    wc::count_words_parallel(&data, utf8_locale)
                } else {
                    0
                },
                bytes: data.len() as u64,
                chars: if show.chars {
                    wc::count_chars_parallel(&data, utf8_locale)
                } else {
                    0
                },
                max_line_length: if show.max_line_length {
                    wc::max_line_length(&data, utf8_locale)
                } else {
                    0
                },
            }
        };

        total.lines += counts.lines;
        total.words += counts.words;
        total.bytes += counts.bytes;
        total.chars += counts.chars;
        if counts.max_line_length > total.max_line_length {
            total.max_line_length = counts.max_line_length;
        }

        let display_name = if filename == "-" {
            String::new()
        } else {
            filename.clone()
        };
        results.push((counts, display_name));
    }

    // Phase 2: Compute column width
    // GNU wc uses the digit width of the largest value across all computed metrics
    // (including bytes, which is always computed) for column alignment.
    // Special case: single file + single column + no total = natural width.
    // For stdin with no files, GNU uses a default minimum width of 7.
    // --total=only: GNU uses width 1 (natural width, no padding).

    // Determine whether to print total line (needed for width calculation)
    let show_total = match total_mode {
        "always" => true,
        "never" => false,
        "only" => true,
        _ => results.len() > 1, // "auto"
    };

    let num_columns = show.lines as usize
        + show.words as usize
        + show.bytes as usize
        + show.chars as usize
        + show.max_line_length as usize;

    let num_output_rows = if total_mode == "only" {
        if show_total { 1 } else { 0 }
    } else {
        results.len() + if show_total { 1 } else { 0 }
    };

    let min_width = if has_stdin && results.len() == 1 {
        7
    } else {
        1
    };

    let width = if total_mode == "only" {
        // --total=only: GNU uses width 1 (natural width, no padding)
        1
    } else if num_columns <= 1 && num_output_rows <= 1 {
        // Single value output: no alignment needed, use natural width
        // min_width (7 for stdin) only applies to multi-column output
        let single_val = if show.lines {
            total.lines
        } else if show.words {
            total.words
        } else if show.chars {
            total.chars
        } else if show.bytes {
            total.bytes
        } else if show.max_line_length {
            total.max_line_length
        } else {
            0
        };
        num_width(single_val)
    } else {
        // Multiple columns or multiple rows: use max of ALL computed values
        // (including bytes which is always computed) for consistent alignment
        let max_val = [
            total.lines,
            total.words,
            total.bytes,
            total.chars,
            total.max_line_length,
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        num_width(max_val).max(min_width)
    };

    // Phase 3: Print results
    let stdout = io::stdout();
    let mut out = stdout.lock();

    // --total=only: suppress individual file output
    if total_mode != "only" {
        for (counts, name) in &results {
            print_counts_fmt(&mut out, counts, name, width, &show);
        }
    }

    if show_total {
        let label = if total_mode == "only" { "" } else { "total" };
        print_counts_fmt(&mut out, &total, label, width, &show);
    }

    if had_error {
        process::exit(1);
    }
}

/// Print count values in GNU-compatible format.
/// GNU wc order: newline, word, character, byte, maximum line length.
fn print_counts_fmt(
    out: &mut impl Write,
    counts: &wc::WcCounts,
    filename: &str,
    width: usize,
    show: &ShowFlags,
) {
    let mut first = true;

    macro_rules! field {
        ($val:expr) => {
            if first {
                let _ = write!(out, "{:>w$}", $val, w = width);
            } else {
                let _ = write!(out, " {:>w$}", $val, w = width);
            }
            #[allow(unused_assignments)]
            {
                first = false;
            }
        };
    }

    // GNU wc order: lines, words, chars, bytes, max_line_length
    if show.lines {
        field!(counts.lines);
    }
    if show.words {
        field!(counts.words);
    }
    if show.chars {
        field!(counts.chars);
    }
    if show.bytes {
        field!(counts.bytes);
    }
    if show.max_line_length {
        field!(counts.max_line_length);
    }

    if !filename.is_empty() {
        let _ = write!(out, " {}", filename);
    }
    let _ = writeln!(out);
}

/// Read NUL-terminated filenames from a file (or stdin if "-").
fn read_files0_from(path: &str) -> Vec<String> {
    let data = if path == "-" {
        read_stdin().unwrap_or_default()
    } else {
        std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("fwc: cannot open '{}' for reading: {}", path, e);
            process::exit(1);
        })
    };

    data.split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}
