use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::process;

use clap::Parser;
use rayon::prelude::*;

use coreutils_rs::hash::{self, HashAlgorithm};

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

fn main() {
    let cli = Cli::parse();
    let algo = HashAlgorithm::Md5;
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
                        eprintln!("fmd5sum: {}: {}", filename, e);
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
                warn_prefix: format!("fmd5sum: {}", display_name),
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
                }
                Err(e) => {
                    eprintln!("fmd5sum: {}: {}", filename, e);
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
                    "fmd5sum: {}: no properly formatted checksum lines found",
                    name
                );
                had_error = true;
            } else {
                if total_mismatches > 0 {
                    eprintln!(
                        "fmd5sum: WARNING: {} computed checksum did NOT match",
                        total_mismatches
                    );
                }
                if total_read_errors > 0 {
                    eprintln!(
                        "fmd5sum: WARNING: {} listed file could not be read",
                        total_read_errors
                    );
                }
                if total_format_errors > 0 {
                    eprintln!(
                        "fmd5sum: WARNING: {} line is improperly formatted",
                        total_format_errors
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
                        let name = if filename == "-" { "-" } else { filename.as_str() };
                        if cli.tag {
                            write_tag(&mut out, algo, &h, name, cli.zero);
                        } else {
                            write_hash(&mut out, &h, name, cli.binary, cli.zero);
                        }
                    }
                    Err(e) => {
                        eprintln!("fmd5sum: {}: {}", filename, e);
                        had_error = true;
                    }
                }
            }
        } else {
            // Pre-warm page cache for all files before parallel hashing
            let paths: Vec<_> = files.iter().map(|f| Path::new(f.as_str())).collect();
            hash::readahead_files(&paths.iter().copied().collect::<Vec<_>>());

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
                        if cli.tag {
                            write_tag(&mut out, algo, &h, filename, cli.zero);
                        } else {
                            write_hash(&mut out, &h, filename, cli.binary, cli.zero);
                        }
                    }
                    Err(e) => {
                        eprintln!("fmd5sum: {}: {}", filename, e);
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
fn write_hash(out: &mut impl Write, hash: &str, filename: &str, binary: bool, zero: bool) {
    let mode_char = if binary { '*' } else { ' ' };
    if zero {
        let _ = write!(out, "{} {}{}\0", hash, mode_char, filename);
    } else {
        let _ = writeln!(out, "{} {}{}", hash, mode_char, filename);
    }
}

#[inline]
fn write_tag(out: &mut impl Write, algo: HashAlgorithm, hash: &str, filename: &str, zero: bool) {
    if zero {
        let _ = write!(out, "{} ({}) = {}\0", algo.name(), filename, hash);
    } else {
        let _ = writeln!(out, "{} ({}) = {}", algo.name(), filename, hash);
    }
}
