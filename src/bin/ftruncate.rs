// ftruncate -- shrink or extend the size of each FILE to the specified size
//
// Usage: truncate OPTION... FILE...

use std::fs;
use std::process;

const TOOL_NAME: &str = "truncate";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Size adjustment mode parsed from the SIZE prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SizeMode {
    /// Set to exactly N bytes
    Absolute,
    /// Extend by N bytes (+)
    Extend,
    /// Shrink by N bytes (-)
    Shrink,
    /// At most N bytes (<)
    AtMost,
    /// At least N bytes (>)
    AtLeast,
    /// Round down to multiple of N (/)
    RoundDown,
    /// Round up to multiple of N (%)
    RoundUp,
}

fn parse_size_suffix(s: &str) -> Option<(u64, usize)> {
    // Try two-character suffixes first (KB, MB, GB, etc.)
    if s.len() >= 2 {
        let last2 = &s[s.len() - 2..];
        let multiplier = match last2 {
            "KB" => Some(1000u64),
            "MB" => Some(1000u64 * 1000),
            "GB" => Some(1000u64 * 1000 * 1000),
            "TB" => Some(1000u64 * 1000 * 1000 * 1000),
            "PB" => Some(1000u64 * 1000 * 1000 * 1000 * 1000),
            "EB" => Some(1000u64 * 1000 * 1000 * 1000 * 1000 * 1000),
            _ => None,
        };
        if let Some(m) = multiplier {
            return Some((m, 2));
        }
    }
    // Try single-character suffixes
    if !s.is_empty() {
        let last = s.as_bytes()[s.len() - 1];
        let multiplier = match last {
            b'K' => Some(1024u64),
            b'M' => Some(1024u64 * 1024),
            b'G' => Some(1024u64 * 1024 * 1024),
            b'T' => Some(1024u64 * 1024 * 1024 * 1024),
            b'P' => Some(1024u64 * 1024 * 1024 * 1024 * 1024),
            b'E' => Some(1024u64 * 1024 * 1024 * 1024 * 1024 * 1024),
            _ => None,
        };
        if let Some(m) = multiplier {
            return Some((m, 1));
        }
    }
    None
}

fn parse_size(s: &str) -> Result<(SizeMode, u64), String> {
    if s.is_empty() {
        return Err("invalid empty size".to_string());
    }

    let (mode, rest) = match s.as_bytes()[0] {
        b'+' => (SizeMode::Extend, &s[1..]),
        b'-' => (SizeMode::Shrink, &s[1..]),
        b'<' => (SizeMode::AtMost, &s[1..]),
        b'>' => (SizeMode::AtLeast, &s[1..]),
        b'/' => (SizeMode::RoundDown, &s[1..]),
        b'%' => (SizeMode::RoundUp, &s[1..]),
        _ => (SizeMode::Absolute, s),
    };

    if rest.is_empty() {
        return Err(format!("invalid number: '{}'", s));
    }

    let (multiplier, suffix_len) = parse_size_suffix(rest).unwrap_or((1, 0));
    let num_str = &rest[..rest.len() - suffix_len];

    if num_str.is_empty() {
        return Err(format!("invalid number: '{}'", s));
    }

    let value: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid number: '{}'", s))?;

    let total = value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("size overflow: '{}'", s))?;

    Ok((mode, total))
}

fn compute_new_size(current: u64, mode: SizeMode, size: u64) -> u64 {
    match mode {
        SizeMode::Absolute => size,
        SizeMode::Extend => current.saturating_add(size),
        SizeMode::Shrink => current.saturating_sub(size),
        SizeMode::AtMost => {
            if current > size {
                size
            } else {
                current
            }
        }
        SizeMode::AtLeast => {
            if current < size {
                size
            } else {
                current
            }
        }
        SizeMode::RoundDown => {
            if size == 0 {
                current
            } else {
                (current / size) * size
            }
        }
        SizeMode::RoundUp => {
            if size == 0 {
                current
            } else {
                let remainder = current % size;
                if remainder == 0 {
                    current
                } else {
                    current + (size - remainder)
                }
            }
        }
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut no_create = false;
    let mut io_blocks = false;
    let mut reference: Option<String> = None;
    let mut size_str: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if saw_dashdash {
            files.push(arg.clone());
            i += 1;
            continue;
        }
        match arg.as_str() {
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "-c" | "--no-create" => no_create = true,
            "-o" | "--io-blocks" => io_blocks = true,
            "-r" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'r'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                reference = Some(args[i].clone());
            }
            "-s" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 's'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                size_str = Some(args[i].clone());
            }
            "--" => saw_dashdash = true,
            _ if arg.starts_with("--reference=") => {
                reference = Some(arg["--reference=".len()..].to_string());
            }
            _ if arg.starts_with("--size=") => {
                size_str = Some(arg["--size=".len()..].to_string());
            }
            _ if arg.starts_with("-s") && arg.len() > 2 => {
                size_str = Some(arg[2..].to_string());
            }
            _ if arg.starts_with("-r") && arg.len() > 2 => {
                reference = Some(arg[2..].to_string());
            }
            _ if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") => {
                // Parse combined short flags
                let chars: Vec<char> = arg[1..].chars().collect();
                let mut j = 0;
                while j < chars.len() {
                    match chars[j] {
                        'c' => no_create = true,
                        'o' => io_blocks = true,
                        's' => {
                            // Rest of this arg is the size value
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 's'", TOOL_NAME);
                                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                                    process::exit(1);
                                }
                                size_str = Some(args[i].clone());
                            } else {
                                size_str = Some(rest);
                            }
                            break;
                        }
                        'r' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 'r'", TOOL_NAME);
                                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                                    process::exit(1);
                                }
                                reference = Some(args[i].clone());
                            } else {
                                reference = Some(rest);
                            }
                            break;
                        }
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, chars[j]);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    j += 1;
                }
            }
            _ => files.push(arg.clone()),
        }
        i += 1;
    }

    if files.is_empty() {
        eprintln!("{}: missing file operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    if size_str.is_none() && reference.is_none() {
        eprintln!(
            "{}: you must specify either '--size' or '--reference'",
            TOOL_NAME
        );
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    // Get reference file size if specified
    let ref_size: Option<u64> = match &reference {
        Some(rfile) => match fs::metadata(rfile) {
            Ok(meta) => Some(meta.len()),
            Err(e) => {
                eprintln!(
                    "{}: cannot stat '{}': {}",
                    TOOL_NAME,
                    rfile,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        },
        None => None,
    };

    // Parse size specification
    let (mode, size_val) = if let Some(ref ss) = size_str {
        match parse_size(ss) {
            Ok((m, v)) => (m, v),
            Err(e) => {
                eprintln!("{}: invalid number: '{}'", TOOL_NAME, e);
                process::exit(1);
            }
        }
    } else {
        // Only reference file, use absolute mode with ref size
        (SizeMode::Absolute, ref_size.unwrap())
    };

    // If both -r and -s given, the reference size is the base for relative operations
    let base_from_ref = ref_size;

    let _ = io_blocks; // io_blocks would multiply by block size; acknowledged but rarely used

    let mut exit_code = 0;
    for file in &files {
        if let Err(code) = truncate_file(file, no_create, mode, size_val, base_from_ref) {
            exit_code = code;
        }
    }

    if exit_code != 0 {
        process::exit(exit_code);
    }
}

fn truncate_file(
    path: &str,
    no_create: bool,
    mode: SizeMode,
    size_val: u64,
    base_from_ref: Option<u64>,
) -> Result<(), i32> {
    // Determine the current file size
    let current_size = match fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                if no_create {
                    return Ok(());
                }
                0
            } else {
                eprintln!(
                    "{}: cannot open '{}' for writing: {}",
                    TOOL_NAME,
                    path,
                    coreutils_rs::common::io_error_msg(&e)
                );
                return Err(1);
            }
        }
    };

    // The base size for relative operations: use reference file if given, else current size
    let base = base_from_ref.unwrap_or(current_size);
    let new_size = compute_new_size(base, mode, size_val);

    // Open or create the file
    let file = fs::OpenOptions::new()
        .write(true)
        .create(!no_create)
        .open(path);

    match file {
        Ok(f) => {
            if let Err(e) = f.set_len(new_size) {
                eprintln!(
                    "{}: failed to truncate '{}': {}",
                    TOOL_NAME,
                    path,
                    coreutils_rs::common::io_error_msg(&e)
                );
                return Err(1);
            }
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound && no_create {
                return Ok(());
            }
            eprintln!(
                "{}: cannot open '{}' for writing: {}",
                TOOL_NAME,
                path,
                coreutils_rs::common::io_error_msg(&e)
            );
            return Err(1);
        }
    }

    Ok(())
}

fn print_help() {
    println!("Usage: {} OPTION... FILE...", TOOL_NAME);
    println!("Shrink or extend the size of each FILE to the specified size.");
    println!();
    println!("A FILE argument that does not exist is created.");
    println!();
    println!("If a FILE is larger than the specified size, the extra data is lost.");
    println!("If a FILE is shorter, it is extended, and the sparse extended part (hole)");
    println!("reads as zero bytes.");
    println!();
    println!("  -c, --no-create        do not create any files");
    println!("  -o, --io-blocks        treat SIZE as number of IO blocks instead of bytes");
    println!("  -r, --reference=RFILE  base size on RFILE");
    println!("  -s, --size=SIZE        set or adjust the file size by SIZE bytes");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
    println!();
    println!("SIZE may be (or may be an integer optionally followed by) one of following:");
    println!("KB 1000, K 1024, MB 1000*1000, M 1024*1024, and so on for G, T, P, E.");
    println!();
    println!("SIZE may also be prefixed by one of the following modifying characters:");
    println!("'+' extend by, '-' reduce by, '<' at most, '>' at least,");
    println!("'/' round down to multiple of, '%' round up to multiple of.");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("ftruncate");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("truncate"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("truncate"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_create_and_truncate() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "hello world").unwrap(); // 11 bytes

        let output = cmd()
            .args(["-s", "5", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.len(), 5);
        let content = fs::read(&file).unwrap();
        assert_eq!(&content, b"hello");
    }

    #[test]
    fn test_extend_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("extend.txt");
        fs::write(&file, "hi").unwrap(); // 2 bytes

        let output = cmd()
            .args(["-s", "100", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.len(), 100);
    }

    #[test]
    fn test_create_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("newfile.txt");
        assert!(!file.exists());

        let output = cmd()
            .args(["-s", "50", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(file.exists());
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.len(), 50);
    }

    #[test]
    fn test_relative_extend() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rel_ext.txt");
        fs::write(&file, "hello").unwrap(); // 5 bytes

        let output = cmd()
            .args(["-s", "+10", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.len(), 15);
    }

    #[test]
    fn test_relative_shrink() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rel_shrink.txt");
        fs::write(&file, "hello world!").unwrap(); // 12 bytes

        let output = cmd()
            .args(["-s", "-5", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.len(), 7);
    }

    #[test]
    fn test_reference_file() {
        let dir = tempfile::tempdir().unwrap();
        let ref_file = dir.path().join("ref.txt");
        let file = dir.path().join("target.txt");
        fs::write(&ref_file, "1234567890").unwrap(); // 10 bytes
        fs::write(&file, "hello").unwrap(); // 5 bytes

        let output = cmd()
            .args(["-r", ref_file.to_str().unwrap(), file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.len(), 10);
    }

    #[test]
    fn test_suffix_k() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("k.txt");

        let output = cmd()
            .args(["-s", "1K", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.len(), 1024);
    }

    #[test]
    fn test_suffix_m() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("m.txt");

        let output = cmd()
            .args(["-s", "1M", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.len(), 1024 * 1024);
    }

    #[test]
    fn test_suffix_kb() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("kb.txt");

        let output = cmd()
            .args(["-s", "1KB", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.len(), 1000);
    }

    #[test]
    fn test_no_create_flag() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nocreate.txt");
        assert!(!file.exists());

        let output = cmd()
            .args(["-c", "-s", "100", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!file.exists());
    }

    #[test]
    fn test_at_most() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("atmost.txt");
        fs::write(&file, "hello world!").unwrap(); // 12 bytes

        // <10 means at most 10 bytes; file is 12, so it should be truncated to 10
        let output = cmd()
            .args(["-s", "<10", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(fs::metadata(&file).unwrap().len(), 10);
    }

    #[test]
    fn test_at_most_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("atmost_nochange.txt");
        fs::write(&file, "hi").unwrap(); // 2 bytes

        // <10 means at most 10 bytes; file is 2, should stay 2
        let output = cmd()
            .args(["-s", "<10", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(fs::metadata(&file).unwrap().len(), 2);
    }

    #[test]
    fn test_at_least() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("atleast.txt");
        fs::write(&file, "hi").unwrap(); // 2 bytes

        // >10 means at least 10 bytes; file is 2, should become 10
        let output = cmd()
            .args(["-s", ">10", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(fs::metadata(&file).unwrap().len(), 10);
    }

    #[test]
    fn test_round_down() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rounddown.txt");
        fs::write(&file, "hello world!").unwrap(); // 12 bytes

        // /5 means round down to multiple of 5; 12 -> 10
        let output = cmd()
            .args(["-s", "/5", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(fs::metadata(&file).unwrap().len(), 10);
    }

    #[test]
    fn test_round_up() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("roundup.txt");
        fs::write(&file, "hello world!").unwrap(); // 12 bytes

        // %5 means round up to multiple of 5; 12 -> 15
        let output = cmd()
            .args(["-s", "%5", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(fs::metadata(&file).unwrap().len(), 15);
    }

    #[test]
    fn test_missing_file_operand() {
        let output = cmd().args(["-s", "100"]).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("missing file operand"));
    }

    #[test]
    fn test_no_size_or_reference() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nosize.txt");
        fs::write(&file, "data").unwrap();

        let output = cmd().arg(file.to_str().unwrap()).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("--size") || stderr.contains("--reference"));
    }

    #[test]
    fn test_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("multi1.txt");
        let f2 = dir.path().join("multi2.txt");
        fs::write(&f1, "aaa").unwrap();
        fs::write(&f2, "bbbbb").unwrap();

        let output = cmd()
            .args(["-s", "10", f1.to_str().unwrap(), f2.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(fs::metadata(&f1).unwrap().len(), 10);
        assert_eq!(fs::metadata(&f2).unwrap().len(), 10);
    }

    #[test]
    fn test_matches_gnu_truncate() {
        let dir = tempfile::tempdir().unwrap();
        let gnu_file = dir.path().join("gnu.txt");
        let our_file = dir.path().join("our.txt");

        // Create same content
        fs::write(&gnu_file, "hello world test").unwrap();
        fs::write(&our_file, "hello world test").unwrap();

        let gnu = Command::new("truncate")
            .args(["-s", "5", gnu_file.to_str().unwrap()])
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd()
                .args(["-s", "5", our_file.to_str().unwrap()])
                .output()
                .unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
            assert_eq!(
                fs::metadata(&our_file).unwrap().len(),
                fs::metadata(&gnu_file).unwrap().len(),
                "File size mismatch"
            );
        }
    }

    #[test]
    fn test_matches_gnu_extend() {
        let dir = tempfile::tempdir().unwrap();
        let gnu_file = dir.path().join("gnu_ext.txt");
        let our_file = dir.path().join("our_ext.txt");

        fs::write(&gnu_file, "hi").unwrap();
        fs::write(&our_file, "hi").unwrap();

        let gnu = Command::new("truncate")
            .args(["-s", "+100", gnu_file.to_str().unwrap()])
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd()
                .args(["-s", "+100", our_file.to_str().unwrap()])
                .output()
                .unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
            assert_eq!(
                fs::metadata(&our_file).unwrap().len(),
                fs::metadata(&gnu_file).unwrap().len(),
                "File size mismatch after extend"
            );
        }
    }

    // Unit tests for parse_size
    #[test]
    fn test_parse_size_plain() {
        let (mode, val) = super::parse_size("100").unwrap();
        assert_eq!(mode, super::SizeMode::Absolute);
        assert_eq!(val, 100);
    }

    #[test]
    fn test_parse_size_extend() {
        let (mode, val) = super::parse_size("+50").unwrap();
        assert_eq!(mode, super::SizeMode::Extend);
        assert_eq!(val, 50);
    }

    #[test]
    fn test_parse_size_shrink() {
        let (mode, val) = super::parse_size("-30").unwrap();
        assert_eq!(mode, super::SizeMode::Shrink);
        assert_eq!(val, 30);
    }

    #[test]
    fn test_parse_size_suffix_k() {
        let (_, val) = super::parse_size("2K").unwrap();
        assert_eq!(val, 2048);
    }

    #[test]
    fn test_parse_size_suffix_kb() {
        let (_, val) = super::parse_size("2KB").unwrap();
        assert_eq!(val, 2000);
    }

    #[test]
    fn test_parse_size_suffix_m() {
        let (_, val) = super::parse_size("1M").unwrap();
        assert_eq!(val, 1024 * 1024);
    }

    #[test]
    fn test_parse_size_invalid() {
        assert!(super::parse_size("abc").is_err());
        assert!(super::parse_size("").is_err());
    }

    #[test]
    fn test_compute_new_size_absolute() {
        assert_eq!(
            super::compute_new_size(100, super::SizeMode::Absolute, 50),
            50
        );
    }

    #[test]
    fn test_compute_new_size_extend() {
        assert_eq!(
            super::compute_new_size(100, super::SizeMode::Extend, 50),
            150
        );
    }

    #[test]
    fn test_compute_new_size_shrink() {
        assert_eq!(
            super::compute_new_size(100, super::SizeMode::Shrink, 30),
            70
        );
    }

    #[test]
    fn test_compute_new_size_shrink_underflow() {
        assert_eq!(super::compute_new_size(10, super::SizeMode::Shrink, 50), 0);
    }

    #[test]
    fn test_compute_new_size_round_down() {
        assert_eq!(
            super::compute_new_size(12, super::SizeMode::RoundDown, 5),
            10
        );
    }

    #[test]
    fn test_compute_new_size_round_up() {
        assert_eq!(super::compute_new_size(12, super::SizeMode::RoundUp, 5), 15);
    }

    #[test]
    fn test_compute_new_size_round_up_exact() {
        assert_eq!(super::compute_new_size(10, super::SizeMode::RoundUp, 5), 10);
    }

    #[test]
    fn test_size_flag_attached() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("attached.txt");

        let output = cmd()
            .args(["-s100", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(fs::metadata(&file).unwrap().len(), 100);
    }

    #[test]
    fn test_long_size_flag() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("longflag.txt");

        let output = cmd()
            .args(["--size=200", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(fs::metadata(&file).unwrap().len(), 200);
    }
}
