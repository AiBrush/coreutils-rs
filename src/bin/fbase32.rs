// fbase32 — Base32 encode/decode data (GNU base32 replacement, RFC 4648)

use std::io::{self, Read, Write};
use std::process;

const TOOL_NAME: &str = "base32";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Base32 alphabet per RFC 4648
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Build a decoding table: maps ASCII byte -> 5-bit value (0-31), 0xFF for invalid.
const fn build_decode_table() -> [u8; 256] {
    let mut table = [0xFFu8; 256];
    let alpha = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut i = 0;
    while i < 32 {
        table[alpha[i] as usize] = i as u8;
        // Also accept lowercase
        if alpha[i] >= b'A' && alpha[i] <= b'Z' {
            table[(alpha[i] - b'A' + b'a') as usize] = i as u8;
        }
        i += 1;
    }
    table
}

const DECODE_TABLE: [u8; 256] = build_decode_table();

struct Cli {
    decode: bool,
    ignore_garbage: bool,
    wrap: usize,
    file: Option<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        decode: false,
        ignore_garbage: false,
        wrap: 76,
        file: None,
    };

    let mut args = std::env::args_os().skip(1);
    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            if let Some(f) = args.next() {
                cli.file = Some(f.to_string_lossy().into_owned());
            }
            break;
        }
        if bytes.starts_with(b"--") {
            if bytes.starts_with(b"--wrap=") {
                let val = std::str::from_utf8(&bytes[7..]).unwrap_or("76");
                cli.wrap = val.parse().unwrap_or_else(|_| {
                    eprintln!("{}: invalid wrap size: '{}'", TOOL_NAME, val);
                    process::exit(1);
                });
            } else {
                match bytes {
                    b"--decode" => cli.decode = true,
                    b"--ignore-garbage" => cli.ignore_garbage = true,
                    b"--wrap" => {
                        if let Some(v) = args.next() {
                            let s = v.to_string_lossy();
                            cli.wrap = s.parse().unwrap_or_else(|_| {
                                eprintln!("{}: invalid wrap size: '{}'", TOOL_NAME, s);
                                process::exit(1);
                            });
                        } else {
                            eprintln!("{}: option '--wrap' requires an argument", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    b"--help" => {
                        print!(
                            "Usage: {tool} [OPTION]... [FILE]\n\
                             Base32 encode or decode FILE, or standard input, to standard output.\n\n\
                             With no FILE, or when FILE is -, read standard input.\n\n\
                             Mandatory arguments to long options are mandatory for short options too.\n\
                             \x20 -d, --decode          decode data\n\
                             \x20 -i, --ignore-garbage  when decoding, ignore non-alphabet characters\n\
                             \x20 -w, --wrap=COLS       wrap encoded lines after COLS character (default 76).\n\
                             \x20                         Use 0 to disable line wrapping\n\
                             \x20     --help             display this help and exit\n\
                             \x20     --version          output version information and exit\n\n\
                             The data are encoded as described for the base32 alphabet in RFC 4648.\n\
                             When decoding, the input may contain newlines in addition to the bytes of\n\
                             the formal base32 alphabet.  Use --ignore-garbage to attempt to recover\n\
                             from any other non-alphabet bytes in the encoded stream.\n",
                            tool = TOOL_NAME
                        );
                        process::exit(0);
                    }
                    b"--version" => {
                        println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
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
                    b'd' => cli.decode = true,
                    b'i' => cli.ignore_garbage = true,
                    b'w' => {
                        if i + 1 < bytes.len() {
                            let val = std::str::from_utf8(&bytes[i + 1..]).unwrap_or("76");
                            cli.wrap = val.parse().unwrap_or_else(|_| {
                                eprintln!("{}: invalid wrap size: '{}'", TOOL_NAME, val);
                                process::exit(1);
                            });
                            i = bytes.len();
                            continue;
                        } else if let Some(v) = args.next() {
                            let s = v.to_string_lossy();
                            cli.wrap = s.parse().unwrap_or_else(|_| {
                                eprintln!("{}: invalid wrap size: '{}'", TOOL_NAME, s);
                                process::exit(1);
                            });
                        } else {
                            eprintln!("{}: option requires an argument -- 'w'", TOOL_NAME);
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
            cli.file = Some(arg.to_string_lossy().into_owned());
        }
    }

    cli
}

/// Encode binary data to Base32 string.
fn base32_encode(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }

    let mut result = Vec::with_capacity(data.len().div_ceil(5) * 8);

    let chunks = data.chunks(5);
    for chunk in chunks {
        let mut buf = [0u8; 5];
        buf[..chunk.len()].copy_from_slice(chunk);

        // Each group of 5 bytes produces 8 base32 characters
        let b0 = buf[0];
        let b1 = buf[1];
        let b2 = buf[2];
        let b3 = buf[3];
        let b4 = buf[4];

        result.push(BASE32_ALPHABET[(b0 >> 3) as usize]);
        result.push(BASE32_ALPHABET[((b0 & 0x07) << 2 | b1 >> 6) as usize]);

        if chunk.len() > 1 {
            result.push(BASE32_ALPHABET[((b1 >> 1) & 0x1F) as usize]);
            result.push(BASE32_ALPHABET[((b1 & 0x01) << 4 | b2 >> 4) as usize]);
        } else {
            result.push(b'=');
            result.push(b'=');
            result.push(b'=');
            result.push(b'=');
            result.push(b'=');
            result.push(b'=');
            continue;
        }

        if chunk.len() > 2 {
            result.push(BASE32_ALPHABET[((b2 & 0x0F) << 1 | b3 >> 7) as usize]);
        } else {
            result.push(b'=');
            result.push(b'=');
            result.push(b'=');
            result.push(b'=');
            continue;
        }

        if chunk.len() > 3 {
            result.push(BASE32_ALPHABET[((b3 >> 2) & 0x1F) as usize]);
            result.push(BASE32_ALPHABET[((b3 & 0x03) << 3 | b4 >> 5) as usize]);
        } else {
            result.push(b'=');
            result.push(b'=');
            result.push(b'=');
            continue;
        }

        if chunk.len() > 4 {
            result.push(BASE32_ALPHABET[(b4 & 0x1F) as usize]);
        } else {
            result.push(b'=');
        }
    }

    // SAFETY: BASE32_ALPHABET and '=' are all ASCII
    unsafe { String::from_utf8_unchecked(result) }
}

/// Decode Base32 string back to binary data.
/// Returns Err on invalid input (unless ignore_garbage is set).
fn base32_decode(input: &[u8], ignore_garbage: bool) -> Result<Vec<u8>, String> {
    // Filter input: always skip newlines and \r; if ignore_garbage, skip all non-base32 chars
    let mut filtered = Vec::with_capacity(input.len());
    for &b in input {
        if b == b'\n' || b == b'\r' {
            continue;
        }
        if b == b'=' {
            filtered.push(b);
            continue;
        }
        if DECODE_TABLE[b as usize] != 0xFF {
            filtered.push(b);
        } else if !ignore_garbage {
            return Err(format!("{}: invalid input", TOOL_NAME));
        }
    }

    if filtered.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::with_capacity(filtered.len() * 5 / 8);
    let chunks = filtered.chunks(8);

    for chunk in chunks {
        // Pad chunk to 8 characters with '='
        let mut buf = [b'='; 8];
        buf[..chunk.len()].copy_from_slice(chunk);

        // Count non-padding characters
        let mut valid_count = 0;
        let mut vals = [0u8; 8];
        for (i, &b) in buf.iter().enumerate() {
            if b == b'=' {
                break;
            }
            let v = DECODE_TABLE[b as usize];
            if v == 0xFF {
                return Err(format!("{}: invalid input", TOOL_NAME));
            }
            vals[i] = v;
            valid_count += 1;
        }

        if valid_count == 0 {
            continue;
        }

        // Decode based on how many valid chars we have:
        // 2 chars -> 1 byte, 4 chars -> 2 bytes, 5 chars -> 3 bytes,
        // 7 chars -> 4 bytes, 8 chars -> 5 bytes
        if valid_count >= 2 {
            result.push((vals[0] << 3) | (vals[1] >> 2));
        }
        if valid_count >= 4 {
            result.push((vals[1] << 6) | (vals[2] << 1) | (vals[3] >> 4));
        }
        if valid_count >= 5 {
            result.push((vals[3] << 4) | (vals[4] >> 1));
        }
        if valid_count >= 7 {
            result.push((vals[4] << 7) | (vals[5] << 2) | (vals[6] >> 3));
        }
        if valid_count >= 8 {
            result.push((vals[6] << 5) | vals[7]);
        }
    }

    Ok(result)
}

/// Wrap encoded text at the specified column width.
/// Write encoded output with line wrapping directly to writer, avoiding
/// intermediate String allocation.
fn write_wrapped(out: &mut impl Write, encoded: &str, wrap: usize) -> io::Result<()> {
    if encoded.is_empty() {
        return Ok(());
    }
    if wrap == 0 {
        // GNU base32 with -w 0 does NOT add a trailing newline
        return out.write_all(encoded.as_bytes());
    }

    let bytes = encoded.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        let end = (pos + wrap).min(bytes.len());
        out.write_all(&bytes[pos..end])?;
        out.write_all(b"\n")?;
        pos = end;
    }
    Ok(())
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let cli = parse_args();
    let filename = cli.file.as_deref().unwrap_or("-");

    let data = if filename == "-" {
        let mut buf = Vec::new();
        if let Err(e) = io::stdin().lock().read_to_end(&mut buf) {
            eprintln!("{}: {}", TOOL_NAME, coreutils_rs::common::io_error_msg(&e));
            process::exit(1);
        }
        buf
    } else {
        match std::fs::read(filename) {
            Ok(data) => data,
            Err(e) => {
                eprintln!(
                    "{}: {}: {}",
                    TOOL_NAME,
                    filename,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        }
    };

    let stdout = io::stdout();
    let mut out = io::BufWriter::with_capacity(1024 * 1024, stdout.lock());

    if cli.decode {
        match base32_decode(&data, cli.ignore_garbage) {
            Ok(decoded) => {
                if let Err(e) = out.write_all(&decoded) {
                    if e.kind() == io::ErrorKind::BrokenPipe {
                        process::exit(0);
                    }
                    eprintln!("{}: write error: {}", TOOL_NAME, e);
                    process::exit(1);
                }
            }
            Err(msg) => {
                eprintln!("{}", msg);
                process::exit(1);
            }
        }
    } else {
        let encoded = base32_encode(&data);
        if let Err(e) = write_wrapped(&mut out, &encoded, cli.wrap) {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("{}: write error: {}", TOOL_NAME, e);
            process::exit(1);
        }
    }

    if let Err(e) = out.flush()
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("{}: write error: {}", TOOL_NAME, e);
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fbase32");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("--decode"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("base32"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_encode_hello() {
        // "Hello" in base32 is "JBSWY3DP"
        let mut child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"Hello").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "JBSWY3DP");
    }

    #[test]
    fn test_encode_hello_newline() {
        // "Hello\n" in base32 is "JBSWY3DPBI======"
        let mut child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"Hello\n").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "JBSWY3DPBI======");
    }

    #[test]
    fn test_decode() {
        let mut child = cmd()
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"JBSWY3DP\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(&output.stdout, b"Hello");
    }

    #[test]
    fn test_roundtrip() {
        let input = b"The quick brown fox jumps over the lazy dog";

        // Encode
        let mut child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let encode_output = child.wait_with_output().unwrap();
        assert!(encode_output.status.success());

        // Decode
        let mut child = cmd()
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&encode_output.stdout)
            .unwrap();
        let decode_output = child.wait_with_output().unwrap();
        assert!(decode_output.status.success());
        assert_eq!(&decode_output.stdout, input);
    }

    #[test]
    fn test_wrap_flag() {
        // Test -w 10 wraps at 10 columns
        let mut child = cmd()
            .arg("-w")
            .arg("10")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"Hello, World!")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            assert!(line.len() <= 10, "line longer than 10: {}", line);
        }
    }

    #[test]
    fn test_wrap_zero() {
        // -w 0 disables wrapping
        let mut child = cmd()
            .arg("-w")
            .arg("0")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"Hello, World! This is a longer test input to verify no wrapping.")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 1, "should be a single line with -w 0");
    }

    #[test]
    fn test_ignore_garbage() {
        // -i should ignore garbage characters during decode
        let mut child = cmd()
            .arg("-d")
            .arg("-i")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // Insert garbage characters in base32 encoded "Hello"
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"JB!!S##WY3DP\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(&output.stdout, b"Hello");
    }

    #[test]
    fn test_empty_input() {
        let mut child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn test_encode_lib_function() {
        // Unit test encode directly
        assert_eq!(base32_encode(b""), "");
        assert_eq!(base32_encode(b"f"), "MY======");
        assert_eq!(base32_encode(b"fo"), "MZXQ====");
        assert_eq!(base32_encode(b"foo"), "MZXW6===");
        assert_eq!(base32_encode(b"foob"), "MZXW6YQ=");
        assert_eq!(base32_encode(b"fooba"), "MZXW6YTB");
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI======");
    }

    #[test]
    fn test_decode_lib_function() {
        assert_eq!(base32_decode(b"", false).unwrap(), b"");
        assert_eq!(base32_decode(b"MY======", false).unwrap(), b"f");
        assert_eq!(base32_decode(b"MZXQ====", false).unwrap(), b"fo");
        assert_eq!(base32_decode(b"MZXW6===", false).unwrap(), b"foo");
        assert_eq!(base32_decode(b"MZXW6YQ=", false).unwrap(), b"foob");
        assert_eq!(base32_decode(b"MZXW6YTB", false).unwrap(), b"fooba");
        assert_eq!(
            base32_decode(b"MZXW6YTBOI======", false).unwrap(),
            b"foobar"
        );
    }

    #[test]
    fn test_compare_gnu_base32_encode() {
        if Command::new("base32").arg("--version").output().is_err() {
            return;
        }
        let input = b"Hello, World!\n";

        let mut gnu_child = match Command::new("base32")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        gnu_child.stdin.take().unwrap().write_all(input).unwrap();
        let gnu_out = gnu_child.wait_with_output().unwrap();

        let mut our_child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        our_child.stdin.take().unwrap().write_all(input).unwrap();
        let our_out = our_child.wait_with_output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&our_out.stdout),
            String::from_utf8_lossy(&gnu_out.stdout),
            "Encoding mismatch with GNU base32"
        );
    }

    #[test]
    fn test_compare_gnu_base32_decode() {
        if Command::new("base32").arg("--version").output().is_err() {
            return;
        }
        let encoded = b"JBSWY3DPEHPK3PXP\n";

        let mut gnu_child = match Command::new("base32")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        gnu_child.stdin.take().unwrap().write_all(encoded).unwrap();
        let gnu_out = gnu_child.wait_with_output().unwrap();

        let mut our_child = cmd()
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        our_child.stdin.take().unwrap().write_all(encoded).unwrap();
        let our_out = our_child.wait_with_output().unwrap();
        assert_eq!(
            our_out.stdout, gnu_out.stdout,
            "Decoding mismatch with GNU base32"
        );
    }

    #[test]
    fn test_long_decode_flag() {
        let mut child = cmd()
            .arg("--decode")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"JBSWY3DP\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(&output.stdout, b"Hello");
    }
}
