use std::io::{self, BufReader, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

use coreutils_rs::common::io_error_msg;
use coreutils_rs::hash::{self, HashAlgorithm};

const TOOL_NAME: &str = "md5sum";

struct Cli {
    binary: bool,
    check: bool,
    tag: bool,
    text: bool,
    ignore_missing: bool,
    quiet: bool,
    status: bool,
    strict: bool,
    warn: bool,
    zero: bool,
    files: Vec<String>,
}

/// Hand-rolled argument parser — eliminates clap's ~100-200µs initialization.
fn parse_args() -> Cli {
    let mut cli = Cli {
        binary: false,
        check: false,
        tag: false,
        text: false,
        ignore_missing: false,
        quiet: false,
        status: false,
        strict: false,
        warn: false,
        zero: false,
        files: Vec::new(),
    };

    let args = std::env::args_os().skip(1);
    let mut saw_dashdash = false;
    for arg in args {
        let bytes = arg.as_encoded_bytes();
        if saw_dashdash {
            cli.files.push(arg.to_string_lossy().into_owned());
            continue;
        }
        if bytes == b"--" {
            saw_dashdash = true;
            continue;
        }
        if bytes.starts_with(b"--") {
            match bytes {
                b"--binary" => cli.binary = true,
                b"--check" => cli.check = true,
                b"--tag" => cli.tag = true,
                b"--text" => cli.text = true,
                b"--ignore-missing" => cli.ignore_missing = true,
                b"--quiet" => cli.quiet = true,
                b"--status" => cli.status = true,
                b"--strict" => cli.strict = true,
                b"--warn" => cli.warn = true,
                b"--zero" => cli.zero = true,
                b"--help" => {
                    print!(
                        "Usage: {} [OPTION]... [FILE]...\n\
                        Print or check MD5 (128-bit) checksums.\n\n\
                        With no FILE, or when FILE is -, read standard input.\n\n\
                        \x20 -b, --binary         read in binary mode\n\
                        \x20 -c, --check          read checksums from the FILEs and check them\n\
                        \x20     --tag             create a BSD-style checksum\n\
                        \x20 -t, --text           read in text mode (default)\n\
                        \x20 -z, --zero           end each output line with NUL, not newline\n\n\
                        The following five options are useful only when verifying checksums:\n\
                        \x20     --ignore-missing  don't fail or report status for missing files\n\
                        \x20     --quiet           don't print OK for each successfully verified file\n\
                        \x20     --status          don't output anything, status code shows success\n\
                        \x20     --strict          exit non-zero for improperly formatted checksum lines\n\
                        \x20 -w, --warn           warn about improperly formatted checksum lines\n\n\
                        \x20     --help            display this help and exit\n\
                        \x20     --version         output version information and exit\n",
                        TOOL_NAME
                    );
                    process::exit(0);
                }
                b"--version" => {
                    println!("{} (fcoreutils) {}", TOOL_NAME, env!("CARGO_PKG_VERSION"));
                    process::exit(0);
                }
                _ => {
                    eprintln!(
                        "{}: unrecognized option '{}'",
                        TOOL_NAME,
                        arg.to_string_lossy()
                    );
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            for &b in &bytes[1..] {
                match b {
                    b'b' => cli.binary = true,
                    b'c' => cli.check = true,
                    b't' => cli.text = true,
                    b'w' => cli.warn = true,
                    b'z' => cli.zero = true,
                    _ => {
                        eprintln!("{}: invalid option -- '{}'", TOOL_NAME, b as char);
                        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                        process::exit(1);
                    }
                }
            }
        } else {
            cli.files.push(arg.to_string_lossy().into_owned());
        }
    }

    cli
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

fn main() {
    coreutils_rs::common::reset_sigpipe();
    let cli = parse_args();
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

    // Raw fd stdout on Unix for zero-overhead writes
    #[cfg(unix)]
    let mut raw = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(1)) };
    #[cfg(unix)]
    let mut out = BufWriter::new(&mut *raw);
    #[cfg(not(unix))]
    let stdout = io::stdout();
    #[cfg(not(unix))]
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
                        if !cli.status {
                            let _ = out.flush();
                            eprintln!("{}: {}: no file was verified", TOOL_NAME, display_name);
                        }
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

        // "no properly formatted checksum lines found" — always set error,
        // even with --status (GNU compat: exit 1 when no valid lines)
        let checked = total_ok + total_mismatches + total_read_errors;
        if checked == 0 && total_format_errors > 0 {
            if !cli.status {
                let name = if files.len() == 1 && files[0] == "-" {
                    "standard input"
                } else {
                    &files[0]
                };
                eprintln!(
                    "{}: {}: no properly formatted MD5 checksum lines found",
                    TOOL_NAME, name
                );
            }
            had_error = true;
        }

        // Print GNU-compatible warning summaries to stderr
        if !cli.status {
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
    } else {
        // Hash mode
        let has_stdin = files.iter().any(|f| f == "-");

        if has_stdin || files.len() <= 10 {
            // Sequential for stdin or small file count — avoids thread::scope
            // overhead (~120µs) and readahead double-open penalty.
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
            // Multi-file: always use rayon parallel hashing with readahead.
            // Even for many small files, rayon overhead is negligible (~1μs/task)
            // vs per-file I/O (~10-50μs), and 4-thread parallelism gives ~4x.
            let paths: Vec<_> = files.iter().map(|f| Path::new(f.as_str())).collect();
            let results = hash::hash_files_parallel(&paths, algo);

            for (filename, result) in files.iter().zip(results) {
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

/// Write hash output using single-write batched buffer for minimum overhead.
#[inline]
fn write_output(out: &mut impl Write, cli: &Cli, algo: HashAlgorithm, hash: &str, filename: &str) {
    let binary = cli.binary || (!cli.text && cfg!(windows));
    if cli.tag {
        let _ = hash::write_hash_tag_line(out, algo.name(), hash, filename, cli.zero);
    } else if !cli.zero && needs_escape(filename) {
        let escaped = escape_filename(filename);
        let _ = hash::write_hash_line(out, hash, &escaped, binary, cli.zero, true);
    } else {
        let _ = hash::write_hash_line(out, hash, filename, binary, cli.zero, false);
    }
}
