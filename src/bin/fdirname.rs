// fdirname — strip last component from file name
//
// Usage: dirname [OPTION] NAME...
// Output each NAME with its last non-slash component and trailing slashes removed;
// if NAME contains no /'s, output '.' (meaning the current directory).

use std::io::Write;
use std::process;

const TOOL_NAME: &str = "dirname";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut zero = false;
    let mut names: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    for arg in std::env::args().skip(1) {
        if saw_dashdash {
            names.push(arg);
            continue;
        }
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION] NAME...", TOOL_NAME);
                println!("Output each NAME with its last non-slash component and trailing slashes");
                println!("removed; if NAME contains no /'s, output '.' (meaning the current");
                println!("directory).");
                println!();
                println!("  -z, --zero    end each output line with NUL, not newline");
                println!("      --help    display this help and exit");
                println!("      --version output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--zero" | "-z" => zero = true,
            "--" => saw_dashdash = true,
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                for ch in s[1..].chars() {
                    match ch {
                        'z' => zero = true,
                        c => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, c);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            _ => names.push(arg),
        }
    }

    if names.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let terminator = if zero { '\0' } else { '\n' };
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for name in &names {
        let result = dirname(name);
        let _ = write!(out, "{}{}", result, terminator);
    }
}

/// Compute the dirname of `name`. Follows GNU coreutils behavior:
/// 1. Strip trailing slashes (unless the whole string is slashes)
/// 2. If no slash remains, return "."
/// 3. Strip the trailing non-slash component
/// 4. Strip trailing slashes from the result (unless it's all slashes)
/// 5. If empty, return "/"? No — if we got here there was a slash.
fn dirname(name: &str) -> &str {
    // Empty string → "."
    if name.is_empty() {
        return ".";
    }

    let bytes = name.as_bytes();
    let len = bytes.len();

    // Step 1: Find end — skip trailing slashes
    let mut end = len;
    while end > 0 && bytes[end - 1] == b'/' {
        end -= 1;
    }

    // If the entire string is slashes, dirname is "/"
    if end == 0 {
        return "/";
    }

    // Step 2: Skip over the last component (non-slash characters)
    while end > 0 && bytes[end - 1] != b'/' {
        end -= 1;
    }

    // If no slash was found, dirname is "."
    if end == 0 {
        return ".";
    }

    // Step 3: Strip trailing slashes from what remains
    while end > 1 && bytes[end - 1] == b'/' {
        end -= 1;
    }

    &name[..end]
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fdirname");
        Command::new(path)
    }

    #[test]
    fn test_dirname_simple() {
        let output = cmd().arg("/usr/bin").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "/usr"
        );
    }

    #[test]
    fn test_dirname_root() {
        let output = cmd().arg("/").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "/"
        );
    }

    #[test]
    fn test_dirname_no_slash() {
        let output = cmd().arg("hello").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "."
        );
    }

    #[test]
    fn test_dirname_trailing_slash() {
        let output = cmd().arg("/usr/bin/").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "/usr"
        );
    }

    #[test]
    fn test_dirname_multiple_args() {
        let output = cmd()
            .args(["/usr/bin", "/foo/bar/baz", "hello", "/"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines, vec!["/usr", "/foo/bar", ".", "/"]);
    }

    #[test]
    fn test_dirname_zero() {
        let output = cmd().args(["-z", "/usr/bin"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = &output.stdout;
        assert!(stdout.ends_with(&[0u8]), "Should end with NUL byte");
        assert!(!stdout.ends_with(b"\n"), "Should not end with newline");
        let text = String::from_utf8_lossy(&stdout[..stdout.len() - 1]);
        assert_eq!(text, "/usr");
    }

    #[test]
    fn test_dirname_dot() {
        let output = cmd().arg(".").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "."
        );
    }

    #[test]
    fn test_dirname_dotdot() {
        let output = cmd().arg("..").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "."
        );
    }

    #[test]
    fn test_dirname_double_slash() {
        let output = cmd().arg("//").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "/"
        );
    }

    #[test]
    fn test_dirname_matches_gnu() {
        let test_cases = vec![
            vec!["/usr/bin"],
            vec!["/"],
            vec!["hello"],
            vec!["/usr/bin/"],
            vec!["."],
            vec![".."],
            vec!["//"],
            vec!["/a/b/c/d"],
        ];
        for args in &test_cases {
            let gnu = Command::new("dirname").args(args).output();
            if let Ok(gnu) = gnu {
                let ours = cmd().args(args).output().unwrap();
                assert_eq!(
                    String::from_utf8_lossy(&ours.stdout),
                    String::from_utf8_lossy(&gnu.stdout),
                    "Output mismatch for args {:?}",
                    args
                );
                assert_eq!(
                    ours.status.code(),
                    gnu.status.code(),
                    "Exit code mismatch for args {:?}",
                    args
                );
            }
        }
    }
}
