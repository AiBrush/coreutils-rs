use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::process;

use clap::Parser;
use rayon::prelude::*;

use coreutils_rs::hash::{self, HashAlgorithm};

const TOOL_NAME: &str = "fmd5sum";

#[derive(Parser)]
#[command(name = "fmd5sum", about = "Compute and check MD5 message digest")]
struct Cli {
    /// Read in binary mode
    #[arg(short = 'b', long = "binary")]
    binary: bool,

    /// Read checksums from the FILEs and check them
    #[arg(short = 'c', long = "check")]
    check: bool,

    /// Create a BSD-style checksum
    #[arg(long = "tag")]
    tag: bool,

    /// Read in text mode (default)
    #[arg(short = 't', long = "text")]
    text: bool,

    /// Don't fail or report status for missing files
    #[arg(long = "ignore-missing")]
    ignore_missing: bool,

    /// Don't print OK for each successfully verified file
    #[arg(long = "quiet")]
    quiet: bool,

    /// Don't output anything, status code shows success
    #[arg(long = "status")]
    status: bool,

    /// Exit non-zero for improperly formatted checksum lines
    #[arg(long = "strict")]
    strict: bool,

    /// Warn about improperly formatted checksum lines
    #[arg(short = 'w', long = "warn")]
    warn: bool,

    /// End each output line with NUL, not newline
    #[arg(short = 'z', long = "zero")]
    zero: bool,

    /// Files to process
    files: Vec<String>,
}

/// Check if a filename needs escaping (contains backslash or newline).
#[inline]
fn needs_escape(name: &str) -> bool {
    name.bytes().any(|b| b == b'\\' || b == b'\n')
}

/// Escape a filename: replace `\` with `\\` and newline with `\n` (literal).
fn escape_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 8);
    for b in name.bytes() {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            _ => out.push(b as char),
        }
    }
    out
}

/// Format an IO error message without the "(os error N)" suffix.
fn io_error_msg(e: &io::Error) -> String {
    if let Some(raw) = e.raw_os_error() {
        let os_err = io::Error::from_raw_os_error(raw);
        format!("{}", os_err).replace(&format!(" (os error {})", raw), "")
    } else {
        format!("{}", e)
    }
}

fn main() {
    let cli = Cli::parse();
    let algo = HashAlgorithm::Md5;

    // Validate flag combinations
    if cli.tag && cli.check {
        eprintln!(
            "{}: the --tag option is meaningless when verifying checksums",
            TOOL_NAME
        );
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let files = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
    };

    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let mut had_error = false;

    if cli.check {
        // Check mode - sequential (reads from check files/stdin)
        let mut total_ok = 0usize;
        let mut total_mismatches = 0usize;
        let mut total_format_errors = 0usize;
        let mut total_read_errors = 0usize;

        for filename in &files {
            let reader: Box<dyn io::BufRead> = if filename == "-" {
                Box::new(BufReader::new(io::stdin().lock()))
            } else {
                match std::fs::File::open(filename) {
                    Ok(f) => Box::new(BufReader::new(f)),
                    Err(e) => {
                        eprintln!("{}: {}: {}", TOOL_NAME, filename, io_error_msg(&e));
                        had_error = true;
                        continue;
                    }
                }
            };
            let mut err_out = io::stderr();
            let display_name = if filename == "-" {
                "standard input".to_string()
            } else {
                filename.clone()
            };
            let opts = hash::CheckOptions {
                quiet: cli.quiet,
                status_only: cli.status,
                strict: cli.strict,
                warn: cli.warn,
                ignore_missing: cli.ignore_missing,
                warn_prefix: format!("{}: {}", TOOL_NAME, display_name),
            };
            match hash::check_file(algo, reader, &opts, &mut out, &mut err_out) {
                Ok(r) => {
                    total_ok += r.ok;
                    total_mismatches += r.mismatches;
                    total_format_errors += r.format_errors;
                    total_read_errors += r.read_errors;
                    if r.mismatches > 0 || r.read_errors > 0 {
                        had_error = true;
                    }
                    if cli.strict && r.format_errors > 0 {
                        had_error = true;
                    }

                    // GNU compat: when --ignore-missing is used and no file was verified
                    // for this checkfile, print warning and set error
                    if cli.ignore_missing && r.ok == 0 && r.mismatches == 0 && r.ignored_missing > 0
                    {
                        let _ = out.flush();
                        eprintln!("{}: {}: no file was verified", TOOL_NAME, display_name);
                        had_error = true;
                    }
                }
                Err(e) => {
                    eprintln!("{}: {}: {}", TOOL_NAME, filename, io_error_msg(&e));
                    had_error = true;
                }
            }
        }

        // Flush stdout before printing stderr warnings (ordering matters)
        let _ = out.flush();

        // Print GNU-compatible warning summaries to stderr
        if !cli.status {
            let checked = total_ok + total_mismatches + total_read_errors;
            if checked == 0 && total_format_errors > 0 {
                let name = if files.len() == 1 && files[0] == "-" {
                    "standard input"
                } else {
                    &files[0]
                };
                eprintln!(
                    "{}: {}: no properly formatted MD5 checksum lines found",
                    TOOL_NAME, name
                );
                had_error = true;
            } else {
                if total_mismatches > 0 {
                    let word = if total_mismatches == 1 {
                        "computed checksum did NOT match"
                    } else {
                        "computed checksums did NOT match"
                    };
                    eprintln!("{}: WARNING: {} {}", TOOL_NAME, total_mismatches, word);
                }
                if total_read_errors > 0 {
                    let word = if total_read_errors == 1 {
                        "listed file could not be read"
                    } else {
                        "listed files could not be read"
                    };
                    eprintln!("{}: WARNING: {} {}", TOOL_NAME, total_read_errors, word);
                }
                if total_format_errors > 0 {
                    let line_word = if total_format_errors == 1 {
                        "line is"
                    } else {
                        "lines are"
                    };
                    eprintln!(
                        "{}: WARNING: {} {} improperly formatted",
                        TOOL_NAME, total_format_errors, line_word
                    );
                }
            }
        }
    } else {
        // Hash mode
        let has_stdin = files.iter().any(|f| f == "-");

        if has_stdin || files.len() == 1 {
            // Sequential for stdin or single file
            for filename in &files {
                let hash_result = if filename == "-" {
                    hash::hash_stdin(algo)
                } else {
                    hash::hash_file(algo, Path::new(filename))
                };
                match hash_result {
                    Ok(h) => {
                        let name = if filename == "-" {
                            "-"
                        } else {
                            filename.as_str()
                        };
                        write_output(&mut out, &cli, algo, &h, name);
                    }
                    Err(e) => {
                        let _ = out.flush();
                        eprintln!("{}: {}: {}", TOOL_NAME, filename, io_error_msg(&e));
                        had_error = true;
                    }
                }
            }
        } else {
            // Pre-warm page cache for all files before parallel hashing
            let paths: Vec<_> = files.iter().map(|f| Path::new(f.as_str())).collect();
            hash::readahead_files(&paths.to_vec());

            // Parallel processing for multiple files
            let results: Vec<(&str, Result<String, io::Error>)> = files
                .par_iter()
                .map(|filename| {
                    let result = hash::hash_file(algo, Path::new(filename));
                    (filename.as_str(), result)
                })
                .collect();

            for (filename, result) in results {
                match result {
                    Ok(h) => {
                        write_output(&mut out, &cli, algo, &h, filename);
                    }
                    Err(e) => {
                        let _ = out.flush();
                        eprintln!("{}: {}: {}", TOOL_NAME, filename, io_error_msg(&e));
                        had_error = true;
                    }
                }
            }
        }
    }

    let _ = out.flush();
    if had_error {
        process::exit(1);
    }
}

#[inline]
fn write_output(out: &mut impl Write, cli: &Cli, algo: HashAlgorithm, hash: &str, filename: &str) {
    if cli.tag {
        if cli.zero {
            let _ = write!(out, "{} ({}) = {}\0", algo.name(), filename, hash);
        } else {
            let _ = writeln!(out, "{} ({}) = {}", algo.name(), filename, hash);
        }
    } else if cli.zero {
        let mode_char = if cli.binary { '*' } else { ' ' };
        let _ = write!(out, "{} {}{}\0", hash, mode_char, filename);
    } else if needs_escape(filename) {
        let escaped = escape_filename(filename);
        let mode_char = if cli.binary { '*' } else { ' ' };
        let _ = writeln!(out, "\\{} {}{}", hash, mode_char, escaped);
    } else {
        let mode_char = if cli.binary { '*' } else { ' ' };
        let _ = writeln!(out, "{} {}{}", hash, mode_char, filename);
    }
}
