// fshuf -- generate random permutations
//
// Usage: shuf [OPTION]... [FILE]
//        shuf -e [OPTION]... [ARG]...
//        shuf -i LO-HI [OPTION]...

use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::process;

const TOOL_NAME: &str = "shuf";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("Usage: {} [OPTION]... [FILE]", TOOL_NAME);
    println!("  or:  {} -e [OPTION]... [ARG]...", TOOL_NAME);
    println!("  or:  {} -i LO-HI [OPTION]...", TOOL_NAME);
    println!("Write a random permutation of the input lines to standard output.");
    println!();
    println!("With no FILE, or when FILE is -, read standard input.");
    println!();
    println!("Mandatory arguments to long options are mandatory for short options too.");
    println!("  -e, --echo                treat each ARG as an input line");
    println!("  -i, --input-range=LO-HI   treat each number LO through HI as an input line");
    println!("  -n, --head-count=COUNT     output at most COUNT lines");
    println!("  -o, --output=FILE          write result to FILE instead of standard output");
    println!("  -r, --repeat              output lines can be repeated");
    println!("  -z, --zero-terminated      line delimiter is NUL, not newline");
    println!("      --random-source=FILE   get random bytes from FILE");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
}

fn print_version() {
    println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
}

/// Random number generator matching GNU's randint algorithm.
/// When using --random-source, reads bytes from the file on each generation.
/// When no source, uses xorshift64 PRNG seeded from /dev/urandom.
enum RandGen {
    /// GNU-compatible: read bytes from file, maintain running state
    FileSource {
        reader: io::BufReader<fs::File>,
        source_path: String,
        randnum: u64,
        randmax: u64,
    },
    /// Fast PRNG for when no --random-source given
    Xorshift { state: u64 },
}

impl RandGen {
    fn from_file(path: &str) -> Self {
        let f = match fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "{}: {}: {}",
                    TOOL_NAME,
                    path,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        };
        RandGen::FileSource {
            reader: io::BufReader::new(f),
            source_path: path.to_string(),
            randnum: 0,
            randmax: 0,
        }
    }

    fn from_urandom() -> Self {
        let mut f = match fs::File::open("/dev/urandom") {
            Ok(f) => f,
            Err(_) => {
                let t = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let mut state = t as u64;
                if state == 0 {
                    state = 0x12345678_9abcdef0;
                }
                return RandGen::Xorshift { state };
            }
        };
        let mut buf = [0u8; 8];
        let _ = f.read_exact(&mut buf);
        let mut state = u64::from_le_bytes(buf);
        if state == 0 {
            state = 0x12345678_9abcdef0;
        }
        RandGen::Xorshift { state }
    }

    /// Generate a random number in [0, genmax] matching GNU's randint_genmax.
    fn gen_max(&mut self, genmax: u64) -> u64 {
        if genmax == 0 {
            return 0;
        }
        match self {
            RandGen::FileSource {
                reader,
                source_path,
                randnum,
                randmax,
            } => loop {
                if *randmax < genmax {
                    let mut buf = [0u8; 1];
                    if reader.read_exact(&mut buf).is_err() {
                        eprintln!("{}: {}: end of file", TOOL_NAME, source_path);
                        process::exit(1);
                    }
                    *randnum = randnum.wrapping_mul(256).wrapping_add(buf[0] as u64);
                    *randmax = randmax.wrapping_mul(256).wrapping_add(255);
                } else {
                    let excess_max = *randmax - genmax;
                    let excess = excess_max % (genmax + 1);
                    if excess <= *randmax - *randnum {
                        let result = *randnum % (genmax + 1);
                        *randnum /= genmax + 1;
                        *randmax = excess_max / (genmax + 1);
                        return result;
                    }
                    // Rejection: need more bytes
                    let mut buf = [0u8; 1];
                    if reader.read_exact(&mut buf).is_err() {
                        eprintln!("{}: {}: end of file", TOOL_NAME, source_path);
                        process::exit(1);
                    }
                    *randnum = randnum.wrapping_mul(256).wrapping_add(buf[0] as u64);
                    *randmax = randmax.wrapping_mul(256).wrapping_add(255);
                }
            },
            RandGen::Xorshift { state } => {
                let n = genmax + 1;
                let threshold = u64::MAX - (u64::MAX % n);
                loop {
                    *state ^= *state << 13;
                    *state ^= *state >> 7;
                    *state ^= *state << 17;
                    let r = *state;
                    if r < threshold {
                        return r % n;
                    }
                }
            }
        }
    }

    /// Return a random index in [0, n)
    fn gen_range(&mut self, n: usize) -> usize {
        if n <= 1 {
            return 0;
        }
        self.gen_max((n - 1) as u64) as usize
    }
}

/// Top-down Fisher-Yates shuffle (matches GNU's algorithm)
fn shuffle(items: &mut [String], rng: &mut RandGen) {
    let n = items.len();
    if n <= 1 {
        return;
    }
    for i in 0..n {
        let j = i + rng.gen_range(n - i);
        items.swap(i, j);
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut echo_mode = false;
    let mut input_range: Option<(u64, u64)> = None;
    let mut head_count: Option<usize> = None;
    let mut output_file: Option<String> = None;
    let mut repeat = false;
    let mut zero_terminated = false;
    let mut random_source: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut echo_args: Vec<String> = Vec::new();

    let mut i = 0;
    // First pass: check for -e/--echo to determine positional arg handling
    let has_echo = args.iter().any(|a| a == "-e" || a == "--echo");

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                print_version();
                return;
            }
            "-e" | "--echo" => {
                echo_mode = true;
            }
            "-r" | "--repeat" => {
                repeat = true;
            }
            "-z" | "--zero-terminated" => {
                zero_terminated = true;
            }
            "-i" | "--input-range" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'i'", TOOL_NAME);
                    process::exit(1);
                }
                input_range = Some(parse_range(&args[i]));
            }
            "-n" | "--head-count" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'n'", TOOL_NAME);
                    process::exit(1);
                }
                head_count = Some(parse_count(&args[i]));
            }
            "-o" | "--output" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'o'", TOOL_NAME);
                    process::exit(1);
                }
                output_file = Some(args[i].clone());
            }
            "--random-source" => {
                i += 1;
                if i >= args.len() {
                    eprintln!(
                        "{}: option requires an argument -- 'random-source'",
                        TOOL_NAME
                    );
                    process::exit(1);
                }
                random_source = Some(args[i].clone());
            }
            _ => {
                if let Some(rest) = arg.strip_prefix("--input-range=") {
                    input_range = Some(parse_range(rest));
                } else if let Some(rest) = arg.strip_prefix("--head-count=") {
                    head_count = Some(parse_count(rest));
                } else if let Some(rest) = arg.strip_prefix("--output=") {
                    output_file = Some(rest.to_string());
                } else if let Some(rest) = arg.strip_prefix("--random-source=") {
                    random_source = Some(rest.to_string());
                } else if let Some(rest) = arg.strip_prefix("-i") {
                    input_range = Some(parse_range(rest));
                } else if let Some(rest) = arg.strip_prefix("-n") {
                    head_count = Some(parse_count(rest));
                } else if let Some(rest) = arg.strip_prefix("-o") {
                    output_file = Some(rest.to_string());
                } else if has_echo {
                    echo_args.push(arg.clone());
                } else {
                    positional.push(arg.clone());
                }
            }
        }
        i += 1;
    }

    // Build the lines to shuffle
    let mut lines: Vec<String> = if echo_mode {
        echo_args
    } else if let Some((lo, hi)) = input_range {
        (lo..=hi).map(|n| n.to_string()).collect()
    } else {
        // Read from file or stdin
        let filename = positional.first().map(|s| s.as_str());
        read_lines(filename, zero_terminated)
    };

    if lines.is_empty() && !repeat {
        // Nothing to shuffle
        return;
    }

    let mut rng = if let Some(ref source) = random_source {
        RandGen::from_file(source)
    } else {
        RandGen::from_urandom()
    };

    // Determine output destination
    let stdout = io::stdout();
    let mut out: Box<dyn Write> = if let Some(ref outfile) = output_file {
        match fs::File::create(outfile) {
            Ok(f) => Box::new(io::BufWriter::new(f)),
            Err(e) => {
                eprintln!(
                    "{}: {}: {}",
                    TOOL_NAME,
                    outfile,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        }
    } else {
        Box::new(io::BufWriter::new(stdout.lock()))
    };

    let delimiter = if zero_terminated { b'\0' } else { b'\n' };

    if repeat {
        // Repeat mode: output random selections indefinitely (or up to head_count)
        if lines.is_empty() {
            eprintln!("{}: no lines to repeat", TOOL_NAME);
            process::exit(1);
        }
        let count = head_count.unwrap_or(usize::MAX);
        for _ in 0..count {
            let idx = rng.gen_range(lines.len());
            let _ = out.write_all(lines[idx].as_bytes());
            let _ = out.write_all(&[delimiter]);
        }
    } else {
        // Normal shuffle mode
        shuffle(&mut lines, &mut rng);

        let count = head_count.unwrap_or(lines.len()).min(lines.len());
        for line in lines.iter().take(count) {
            let _ = out.write_all(line.as_bytes());
            let _ = out.write_all(&[delimiter]);
        }
    }

    let _ = out.flush();
}

fn parse_range(s: &str) -> (u64, u64) {
    // Find the separator '-' that's not part of a negative number sign
    // Format: LO-HI where LO and HI are non-negative integers
    let sep_pos = if let Some(rest) = s.strip_prefix('-') {
        // First char is '-' (could be negative, which is invalid for u64 range)
        // Look for next '-' as separator
        match rest.find('-') {
            Some(p) => p + 1,
            None => {
                eprintln!("{}: invalid input range: \u{2018}{}\u{2019}", TOOL_NAME, s);
                process::exit(1);
            }
        }
    } else {
        match s.find('-') {
            Some(p) => p,
            None => {
                eprintln!("{}: invalid input range: \u{2018}{}\u{2019}", TOOL_NAME, s);
                process::exit(1);
            }
        }
    };

    let lo_str = &s[..sep_pos];
    let hi_str = &s[sep_pos + 1..];

    let lo: u64 = match lo_str.parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!("{}: invalid input range: \u{2018}{}\u{2019}", TOOL_NAME, s);
            process::exit(1);
        }
    };
    let hi: u64 = match hi_str.parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!("{}: invalid input range: \u{2018}{}\u{2019}", TOOL_NAME, s);
            process::exit(1);
        }
    };
    if lo > hi {
        eprintln!("{}: invalid input range: \u{2018}{}\u{2019}", TOOL_NAME, s);
        process::exit(1);
    }
    (lo, hi)
}

fn parse_count(s: &str) -> usize {
    match s.parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!("{}: invalid line count: '{}'", TOOL_NAME, s);
            process::exit(1);
        }
    }
}

fn read_lines(filename: Option<&str>, zero_terminated: bool) -> Vec<String> {
    let mut lines = Vec::new();
    match filename {
        Some("-") | None => {
            let stdin = io::stdin();
            if zero_terminated {
                let mut buf = Vec::new();
                stdin.lock().read_to_end(&mut buf).unwrap_or(0);
                for chunk in buf.split(|&b| b == 0) {
                    if !chunk.is_empty() {
                        lines.push(String::from_utf8_lossy(chunk).to_string());
                    }
                }
            } else {
                for line in stdin.lock().lines() {
                    match line {
                        Ok(l) => lines.push(l),
                        Err(_) => break,
                    }
                }
            }
        }
        Some(file) => {
            let contents = match fs::read(file) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "{}: {}: {}",
                        TOOL_NAME,
                        file,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                    process::exit(1);
                }
            };
            let delimiter = if zero_terminated { 0u8 } else { b'\n' };
            for chunk in contents.split(|&b| b == delimiter) {
                if !chunk.is_empty() {
                    lines.push(String::from_utf8_lossy(chunk).to_string());
                }
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::io::Write;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fshuf");
        Command::new(path)
    }

    #[test]
    fn test_basic_shuffle() {
        let mut child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(b"a\nb\nc\nd\ne\n").unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: HashSet<&str> = stdout.trim().lines().collect();
        let expected: HashSet<&str> = ["a", "b", "c", "d", "e"].iter().copied().collect();
        assert_eq!(
            lines, expected,
            "All elements should be present after shuffle"
        );
    }

    #[test]
    fn test_echo_mode() {
        let output = cmd().args(["-e", "x", "y", "z"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: HashSet<&str> = stdout.trim().lines().collect();
        let expected: HashSet<&str> = ["x", "y", "z"].iter().copied().collect();
        assert_eq!(lines, expected);
    }

    #[test]
    fn test_input_range() {
        let output = cmd().args(["-i", "1-5"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines: Vec<i32> = stdout.trim().lines().map(|l| l.parse().unwrap()).collect();
        lines.sort();
        assert_eq!(lines, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_head_count() {
        let output = cmd().args(["-i", "1-100", "-n", "5"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn test_repeat() {
        let output = cmd()
            .args(["-e", "-r", "-n", "10", "a", "b"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert_eq!(lines.len(), 10);
        for line in &lines {
            assert!(
                *line == "a" || *line == "b",
                "Expected 'a' or 'b', got '{}'",
                line
            );
        }
    }

    #[test]
    fn test_zero_terminated() {
        let mut child = cmd()
            .args(["-z"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(b"a\0b\0c\0").unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = &output.stdout;
        // Output should be NUL-terminated
        let items: HashSet<&[u8]> = stdout
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(items.len(), 3);
        assert!(items.contains(&b"a"[..]));
        assert!(items.contains(&b"b"[..]));
        assert!(items.contains(&b"c"[..]));
    }

    #[test]
    fn test_output_file() {
        let dir = std::env::temp_dir();
        let outpath = dir.join("fshuf_test_output.txt");
        let output = cmd()
            .args(["-e", "hello", "world", "-o", outpath.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let contents = std::fs::read_to_string(&outpath).unwrap();
        let lines: HashSet<&str> = contents.trim().lines().collect();
        assert!(lines.contains("hello"));
        assert!(lines.contains("world"));
        let _ = std::fs::remove_file(&outpath);
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("shuf"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("shuf"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_match_gnu_format() {
        // GNU shuf -i 1-5 output should have exactly 5 lines, each a number 1-5
        let gnu = Command::new("shuf").args(["-i", "1-5"]).output();
        if let Ok(gnu) = gnu {
            // Both should produce 5 lines with numbers 1-5
            let gnu_lines: Vec<i32> = String::from_utf8_lossy(&gnu.stdout)
                .trim()
                .lines()
                .map(|l| l.parse().unwrap())
                .collect();
            assert_eq!(gnu_lines.len(), 5);

            let ours = cmd().args(["-i", "1-5"]).output().unwrap();
            let our_lines: Vec<i32> = String::from_utf8_lossy(&ours.stdout)
                .trim()
                .lines()
                .map(|l| l.parse().unwrap())
                .collect();
            assert_eq!(our_lines.len(), 5);

            let mut gnu_sorted = gnu_lines;
            gnu_sorted.sort();
            let mut our_sorted = our_lines;
            our_sorted.sort();
            assert_eq!(gnu_sorted, our_sorted, "Same set of numbers");
        }
    }

    #[test]
    fn test_input_range_single() {
        let output = cmd().args(["-i", "5-5"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "5");
    }

    #[test]
    fn test_head_count_zero() {
        let output = cmd().args(["-i", "1-10", "-n", "0"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "");
    }
}
