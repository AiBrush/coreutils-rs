use std::io::{self, BufRead, BufReader, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

use coreutils_rs::common::io_error_msg;
use coreutils_rs::hash::{self, HashAlgorithm};

const TOOL_NAME: &str = "sha256sum";
/// SHA256 hex digest is always 64 characters.
const SHA256_HEX_LEN: usize = 64;

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
                        Print or check SHA256 (256-bit) checksums.\n\n\
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

// ── Filename escaping (GNU compat) ─────────────────────────────────

/// Check if a filename needs escaping (contains backslash or newline).
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

/// Unescape a checksum-line filename: `\\` -> `\`, `\n` -> newline.
fn unescape_filename(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn main() {
    coreutils_rs::common::reset_sigpipe();
    let cli = parse_args();
    let algo = HashAlgorithm::Sha256;

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
        run_check_mode(&cli, algo, &files, &mut out, &mut had_error);
    } else {
        run_hash_mode(&cli, algo, &files, &mut out, &mut had_error);
    }

    let _ = out.flush();
    if had_error {
        process::exit(1);
    }
}

fn run_hash_mode(
    cli: &Cli,
    algo: HashAlgorithm,
    files: &[String],
    out: &mut impl Write,
    had_error: &mut bool,
) {
    let has_stdin = files.iter().any(|f| f == "-");

    if has_stdin || files.len() <= 10 {
        // Sequential for stdin or small file counts.
        // For <=10 tiny files, sequential avoids thread::scope spawn overhead
        // (~90µs) which exceeds the parallel speedup on sub-millisecond work.
        // Uses hash_file_nostat to skip fstat (~5µs/file) since the nostat path
        // handles all file sizes via two-tier buffer + streaming fallback.
        for filename in files {
            let hash_result = if filename == "-" {
                hash::hash_stdin(algo)
            } else {
                hash::hash_file_nostat(algo, Path::new(filename))
            };

            match hash_result {
                Ok(h) => {
                    let name = if filename == "-" {
                        "-"
                    } else {
                        filename.as_str()
                    };
                    write_output(out, cli, algo, &h, name);
                }
                Err(e) => {
                    let _ = out.flush();
                    eprintln!("{}: {}: {}", TOOL_NAME, filename, io_error_msg(&e));
                    *had_error = true;
                }
            }
        }
    } else {
        // Multi-file (11+): use parallel hashing with thread::scope.
        let paths: Vec<_> = files.iter().map(|f| Path::new(f.as_str())).collect();
        let results = hash::hash_files_parallel(&paths, algo);

        for (filename, result) in files.iter().zip(results) {
            match result {
                Ok(h) => {
                    write_output(out, cli, algo, &h, filename);
                }
                Err(e) => {
                    let _ = out.flush();
                    eprintln!("{}: {}: {}", TOOL_NAME, filename, io_error_msg(&e));
                    *had_error = true;
                }
            }
        }
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

fn run_check_mode(
    cli: &Cli,
    algo: HashAlgorithm,
    files: &[String],
    out: &mut impl Write,
    had_error: &mut bool,
) {
    let mut _total_ok: usize = 0;
    let mut total_mismatches: usize = 0;
    let mut total_fmt_errors: usize = 0;
    let mut total_read_errors: usize = 0;

    for filename in files {
        let reader: Box<dyn io::BufRead> = if filename == "-" {
            Box::new(BufReader::new(io::stdin().lock()))
        } else {
            match std::fs::File::open(filename) {
                Ok(f) => Box::new(BufReader::new(f)),
                Err(e) => {
                    eprintln!("{}: {}: {}", TOOL_NAME, filename, io_error_msg(&e));
                    *had_error = true;
                    continue;
                }
            }
        };

        let display_name = if filename == "-" {
            "standard input".to_string()
        } else {
            filename.clone()
        };

        let (file_ok, file_fail, file_fmt, file_read, file_ignored) =
            check_one(cli, algo, reader, &display_name, out);

        _total_ok += file_ok;
        total_mismatches += file_fail;
        total_fmt_errors += file_fmt;
        total_read_errors += file_read;

        if file_fail > 0 || file_read > 0 {
            *had_error = true;
        }
        if cli.strict && file_fmt > 0 {
            *had_error = true;
        }

        // "no properly formatted checksum lines found" when no valid lines
        if file_ok == 0 && file_fail == 0 && file_read == 0 && file_ignored == 0 && file_fmt > 0 {
            if !cli.status {
                let _ = out.flush();
                eprintln!(
                    "{}: {}: no properly formatted SHA256 checksum lines found",
                    TOOL_NAME, display_name
                );
            }
            // Don't count these in total_fmt_errors for the summary WARNING
            // since the "no properly formatted" message subsumes it
            total_fmt_errors -= file_fmt;
            *had_error = true;
        }

        // GNU compat: when --ignore-missing is used and no file was verified
        if cli.ignore_missing && file_ok == 0 && file_fail == 0 && file_ignored > 0 {
            if !cli.status {
                let _ = out.flush();
                eprintln!("{}: {}: no file was verified", TOOL_NAME, display_name);
            }
            *had_error = true;
        }
    }

    // Flush stdout before printing stderr warnings
    let _ = out.flush();

    // Print GNU-style summary warnings to stderr
    if !cli.status {
        if total_mismatches > 0 {
            let checksum_word = if total_mismatches == 1 {
                "computed checksum did NOT match"
            } else {
                "computed checksums did NOT match"
            };
            eprintln!(
                "{}: WARNING: {} {}",
                TOOL_NAME, total_mismatches, checksum_word
            );
        }

        if total_read_errors > 0 {
            let word = if total_read_errors == 1 {
                "listed file could not be read"
            } else {
                "listed files could not be read"
            };
            eprintln!("{}: WARNING: {} {}", TOOL_NAME, total_read_errors, word);
        }

        if total_fmt_errors > 0 {
            let line_word = if total_fmt_errors == 1 {
                "line is"
            } else {
                "lines are"
            };
            eprintln!(
                "{}: WARNING: {} {} improperly formatted",
                TOOL_NAME, total_fmt_errors, line_word
            );
        }
    }
}

/// Check checksums from one input source. Returns (ok, fail, fmt_errors, read_errors, ignored_missing).
fn check_one(
    cli: &Cli,
    algo: HashAlgorithm,
    reader: Box<dyn BufRead>,
    display_name: &str,
    out: &mut impl Write,
) -> (usize, usize, usize, usize, usize) {
    let mut ok_count: usize = 0;
    let mut mismatch_count: usize = 0;
    let mut format_errors: usize = 0;
    let mut read_errors: usize = 0;
    let mut ignored_missing: usize = 0;
    let mut line_num: usize = 0;

    for line_result in reader.lines() {
        line_num += 1;
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                eprintln!("{}: {}: {}", TOOL_NAME, display_name, io_error_msg(&e));
                break;
            }
        };
        let line = line.trim_end();

        if line.is_empty() {
            continue;
        }

        // Handle backslash-escaped lines
        let line_content = line.strip_prefix('\\').unwrap_or(line);

        // Parse checksum line
        let (expected_hash, parsed_filename) = match hash::parse_check_line(line_content) {
            Some(v) => v,
            None => {
                format_errors += 1;
                if cli.warn {
                    let _ = out.flush();
                    eprintln!(
                        "{}: {}: {}: improperly formatted SHA256 checksum line",
                        TOOL_NAME, display_name, line_num
                    );
                }
                continue;
            }
        };

        // Validate hash: must be exactly 64 hex characters for SHA256
        if expected_hash.len() != SHA256_HEX_LEN
            || !expected_hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            format_errors += 1;
            if cli.warn {
                let _ = out.flush();
                eprintln!(
                    "{}: {}: {}: improperly formatted SHA256 checksum line",
                    TOOL_NAME, display_name, line_num
                );
            }
            continue;
        }

        // Unescape filename if original line was backslash-prefixed
        let check_filename = if line.starts_with('\\') {
            unescape_filename(parsed_filename)
        } else {
            parsed_filename.to_string()
        };

        // Compute actual hash
        let actual = match hash::hash_file(algo, Path::new(&check_filename)) {
            Ok(h) => h,
            Err(e) => {
                if cli.ignore_missing && e.kind() == io::ErrorKind::NotFound {
                    ignored_missing += 1;
                    continue;
                }
                read_errors += 1;
                if !cli.status {
                    let _ = out.flush();
                    eprintln!("{}: {}: {}", TOOL_NAME, check_filename, io_error_msg(&e));
                    let _ = writeln!(out, "{}: FAILED open or read", check_filename);
                }
                continue;
            }
        };

        if actual.eq_ignore_ascii_case(expected_hash) {
            ok_count += 1;
            if !cli.quiet && !cli.status {
                let _ = writeln!(out, "{}: OK", check_filename);
            }
        } else {
            mismatch_count += 1;
            if !cli.status {
                let _ = writeln!(out, "{}: FAILED", check_filename);
            }
        }
    }

    (
        ok_count,
        mismatch_count,
        format_errors,
        read_errors,
        ignored_missing,
    )
}
