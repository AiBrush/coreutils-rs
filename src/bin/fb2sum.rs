use std::io::{self, BufRead, BufReader, BufWriter, Write};
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process;

use coreutils_rs::common::io_error_msg;
use coreutils_rs::hash;

const TOOL_NAME: &str = "b2sum";

struct Cli {
    binary: bool,
    check: bool,
    ignore_missing: bool,
    length: usize,
    quiet: bool,
    status: bool,
    strict: bool,
    text: bool,
    tag: bool,
    warn: bool,
    zero: bool,
    files: Vec<String>,
}

/// Hand-rolled argument parser — eliminates clap's ~100-200µs initialization.
fn parse_args() -> Cli {
    let mut cli = Cli {
        binary: false,
        check: false,
        ignore_missing: false,
        length: 0,
        quiet: false,
        status: false,
        strict: false,
        text: false,
        tag: false,
        warn: false,
        zero: false,
        files: Vec::new(),
    };

    let mut args = std::env::args_os().skip(1);
    let mut saw_dashdash = false;
    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
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
            if bytes.starts_with(b"--length=") {
                let val = std::str::from_utf8(&bytes[9..]).unwrap_or("0");
                cli.length = val.parse().unwrap_or_else(|_| {
                    eprintln!("{}: invalid length: '{}'", TOOL_NAME, val);
                    process::exit(1);
                });
            } else {
                match bytes {
                    b"--binary" => cli.binary = true,
                    b"--check" => cli.check = true,
                    b"--ignore-missing" => cli.ignore_missing = true,
                    b"--length" => {
                        if let Some(v) = args.next() {
                            let s = v.to_string_lossy();
                            cli.length = s.parse().unwrap_or_else(|_| {
                                eprintln!("{}: invalid length: '{}'", TOOL_NAME, s);
                                process::exit(1);
                            });
                        } else {
                            eprintln!("{}: option '--length' requires an argument", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    b"--quiet" => cli.quiet = true,
                    b"--status" => cli.status = true,
                    b"--strict" => cli.strict = true,
                    b"--text" => cli.text = true,
                    b"--tag" => cli.tag = true,
                    b"--warn" => cli.warn = true,
                    b"--zero" => cli.zero = true,
                    b"--help" => {
                        print!(
                            "Usage: {} [OPTION]... [FILE]...\n\
                            Print or check BLAKE2b (512-bit) checksums.\n\n\
                            With no FILE, or when FILE is -, read standard input.\n\n\
                            \x20 -b, --binary         read in binary mode\n\
                            \x20 -c, --check          read checksums from the FILEs and check them\n\
                            \x20 -l, --length=BITS    digest length in bits; must not exceed 512\n\
                            \x20                        and must be a multiple of 8\n\
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
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            let mut i = 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'b' => cli.binary = true,
                    b'c' => cli.check = true,
                    b't' => cli.text = true,
                    b'w' => cli.warn = true,
                    b'z' => cli.zero = true,
                    b'l' => {
                        if i + 1 < bytes.len() {
                            let val = std::str::from_utf8(&bytes[i + 1..]).unwrap_or("0");
                            cli.length = val.parse().unwrap_or_else(|_| {
                                eprintln!("{}: invalid length: '{}'", TOOL_NAME, val);
                                process::exit(1);
                            });
                            i = bytes.len();
                            continue;
                        } else if let Some(v) = args.next() {
                            let s = v.to_string_lossy();
                            cli.length = s.parse().unwrap_or_else(|_| {
                                eprintln!("{}: invalid length: '{}'", TOOL_NAME, s);
                                process::exit(1);
                            });
                        } else {
                            eprintln!("{}: option requires an argument -- 'l'", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    _ => {
                        eprintln!("{}: invalid option -- '{}'", TOOL_NAME, bytes[i] as char);
                        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                        process::exit(1);
                    }
                }
                i += 1;
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

/// Escape a filename: replace `\` with `\\` and `\n` with `\n` (literal).
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

/// Enlarge pipe buffers on Linux for higher throughput.
#[cfg(target_os = "linux")]
fn enlarge_pipes() {
    const PIPE_SIZE: i32 = 8 * 1024 * 1024;
    unsafe {
        libc::fcntl(0, libc::F_SETPIPE_SZ, PIPE_SIZE);
        libc::fcntl(1, libc::F_SETPIPE_SZ, PIPE_SIZE);
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    #[cfg(target_os = "linux")]
    enlarge_pipes();

    let cli = parse_args();

    // -l 0 means use default (512), matching GNU behavior
    let length = if cli.length == 0 { 512 } else { cli.length };

    // GNU caps at 512 silently for values > 512
    let length = if length > 512 { 512 } else { length };

    if length % 8 != 0 {
        eprintln!("{}: invalid length: '{}'", TOOL_NAME, cli.length);
        eprintln!("{}: length is not a multiple of 8", TOOL_NAME);
        process::exit(1);
    }

    // Validate flag combinations
    if cli.tag && cli.check {
        eprintln!(
            "{}: the --tag option is meaningless when verifying checksums",
            TOOL_NAME
        );
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let output_bytes = length / 8;
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

    let had_error = if cli.check {
        run_check_mode(&cli, &files, &mut out)
    } else {
        run_hash_mode(&cli, &files, output_bytes, &mut out)
    };

    let _ = out.flush();
    if had_error {
        process::exit(1);
    }
}

fn run_hash_mode(cli: &Cli, files: &[String], output_bytes: usize, out: &mut impl Write) -> bool {
    let mut had_error = false;
    let has_stdin = files.iter().any(|f| f == "-");

    if has_stdin || files.len() == 1 {
        // Sequential for stdin or single file
        for filename in files {
            let hash_result = if filename == "-" {
                hash::blake2b_hash_stdin(output_bytes)
            } else {
                hash::blake2b_hash_file(Path::new(filename), output_bytes)
            };

            match hash_result {
                Ok(h) => {
                    let name = if filename == "-" {
                        "-"
                    } else {
                        filename.as_str()
                    };
                    write_output(out, cli, &h, name, output_bytes);
                }
                Err(e) => {
                    let _ = out.flush();
                    eprintln!("{}: {}: {}", TOOL_NAME, filename, io_error_msg(&e));
                    had_error = true;
                }
            }
        }
    } else {
        // Multi-file: use multi-core parallel hashing with work-stealing.
        // Each worker uses blake2b_hash_file which includes I/O pipelining
        // for large files on Linux.
        let paths: Vec<_> = files.iter().map(|f| Path::new(f.as_str())).collect();
        let results = hash::blake2b_hash_files_parallel(&paths, output_bytes);

        for (filename, result) in files.iter().zip(results) {
            match result {
                Ok(h) => {
                    write_output(out, cli, &h, filename, output_bytes);
                }
                Err(e) => {
                    let _ = out.flush();
                    eprintln!("{}: {}: {}", TOOL_NAME, filename, io_error_msg(&e));
                    had_error = true;
                }
            }
        }
    }

    had_error
}

#[inline]
fn write_output(
    out: &mut impl Write,
    cli: &Cli,
    hash_hex: &str,
    filename: &str,
    output_bytes: usize,
) {
    let bits = output_bytes * 8;
    if cli.tag {
        if cli.zero {
            let _ = hash::print_hash_tag_b2sum_zero(out, hash_hex, filename, bits);
        } else {
            let _ = hash::print_hash_tag_b2sum(out, hash_hex, filename, bits);
        }
    } else if cli.zero {
        // GNU defaults to binary mode on Linux; only -t (text) uses space
        let _ = hash::print_hash_zero(
            out,
            hash_hex,
            filename,
            cli.binary || (!cli.text && cfg!(windows)),
        );
    } else if needs_escape(filename) {
        let escaped = escape_filename(filename);
        let mode_char = if cli.binary || (!cli.text && cfg!(windows)) {
            '*'
        } else {
            ' '
        };
        let _ = writeln!(out, "\\{} {}{}", hash_hex, mode_char, escaped);
    } else {
        let _ = hash::print_hash(
            out,
            hash_hex,
            filename,
            cli.binary || (!cli.text && cfg!(windows)),
        );
    }
}

fn run_check_mode(cli: &Cli, files: &[String], out: &mut impl Write) -> bool {
    let mut had_error = false;
    let mut _total_ok: usize = 0;
    let mut total_fail: usize = 0;
    let mut total_fmt_errors: usize = 0;
    let mut total_read_errors: usize = 0;

    for filename in files {
        let reader: Box<dyn BufRead> = if filename == "-" {
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

        let display_name = if filename == "-" {
            "standard input".to_string()
        } else {
            filename.clone()
        };

        let (file_ok, file_fail, file_fmt, file_read, file_ignored) =
            check_one(cli, reader, &display_name, out);

        _total_ok += file_ok;
        total_fail += file_fail;
        total_fmt_errors += file_fmt;
        total_read_errors += file_read;

        if file_fail > 0 || file_read > 0 {
            had_error = true;
        }
        if cli.strict && file_fmt > 0 {
            had_error = true;
        }

        // "no properly formatted checksum lines found"
        if file_ok == 0 && file_fail == 0 && file_read == 0 && file_ignored == 0 && file_fmt > 0 {
            if !cli.status {
                let _ = out.flush();
                eprintln!(
                    "{}: {}: no properly formatted BLAKE2b checksum lines found",
                    TOOL_NAME, display_name
                );
            }
            // Subtract these from total so summary doesn't double-count
            total_fmt_errors -= file_fmt;
            had_error = true;
        }

        // GNU compat: when --ignore-missing is used and no file was verified
        if cli.ignore_missing && file_ok == 0 && file_fail == 0 && file_ignored > 0 {
            if !cli.status {
                let _ = out.flush();
                eprintln!("{}: {}: no file was verified", TOOL_NAME, display_name);
            }
            had_error = true;
        }
    }

    // Flush stdout before printing stderr warnings
    let _ = out.flush();

    // Print GNU-style summary warnings to stderr
    if !cli.status {
        if total_fail > 0 {
            let word = if total_fail == 1 {
                "computed checksum did NOT match"
            } else {
                "computed checksums did NOT match"
            };
            eprintln!("{}: WARNING: {} {}", TOOL_NAME, total_fail, word);
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
            let word = if total_fmt_errors == 1 {
                "line is"
            } else {
                "lines are"
            };
            eprintln!(
                "{}: WARNING: {} {} improperly formatted",
                TOOL_NAME, total_fmt_errors, word
            );
        }
    }

    if total_fail > 0 {
        had_error = true;
    }
    if cli.strict && total_fmt_errors > 0 {
        had_error = true;
    }

    had_error
}

/// Check checksums from one input source. Returns (ok, fail, fmt_errors, read_errors, ignored_missing).
fn check_one(
    cli: &Cli,
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

        // Try parsing: standard format, then BSD tag format
        let (expected_hash, check_filename) =
            if let Some((h, f)) = hash::parse_check_line(line_content) {
                (h.to_string(), f.to_string())
            } else if let Some((h, f, _bits)) = hash::parse_check_line_tag(line_content) {
                (h.to_string(), f.to_string())
            } else {
                format_errors += 1;
                if cli.warn {
                    let _ = out.flush();
                    eprintln!(
                        "{}: {}: {}: improperly formatted BLAKE2b checksum line",
                        TOOL_NAME, display_name, line_num
                    );
                }
                continue;
            };

        // Validate hash: must be valid hex, even length, max 128 hex chars (64 bytes = 512 bits)
        if expected_hash.is_empty()
            || expected_hash.len() % 2 != 0
            || expected_hash.len() > 128
            || !expected_hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            format_errors += 1;
            if cli.warn {
                let _ = out.flush();
                eprintln!(
                    "{}: {}: {}: improperly formatted BLAKE2b checksum line",
                    TOOL_NAME, display_name, line_num
                );
            }
            continue;
        }
        let hash_bytes = expected_hash.len() / 2;

        // Unescape filename if original line was backslash-prefixed
        let check_filename = if line.starts_with('\\') {
            unescape_filename(&check_filename)
        } else {
            check_filename
        };

        // Hash the file with inferred length
        let actual = match hash::blake2b_hash_file(Path::new(&check_filename), hash_bytes) {
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

        if actual.eq_ignore_ascii_case(&expected_hash) {
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
