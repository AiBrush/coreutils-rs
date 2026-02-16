// fbasename — strip directory and suffix from filenames
//
// Usage: basename NAME [SUFFIX]
//   or:  basename OPTION... NAME...

use std::process;

const TOOL_NAME: &str = "basename";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut multiple = false;
    let mut suffix: Option<String> = None;
    let mut zero = false;
    let mut names: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if saw_dashdash {
            names.push(arg.clone());
            i += 1;
            continue;
        }
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} NAME [SUFFIX]", TOOL_NAME);
                println!("  or:  {} OPTION... NAME...", TOOL_NAME);
                println!();
                println!("Print NAME with any leading directory components removed.");
                println!("If specified, also remove a trailing SUFFIX.");
                println!();
                println!(
                    "Mandatory arguments to long options are mandatory for short options too."
                );
                println!(
                    "  -a, --multiple       support multiple arguments and treat each as a NAME"
                );
                println!("  -s, --suffix=SUFFIX  remove a trailing SUFFIX; implies -a");
                println!("  -z, --zero           end each output line with NUL, not newline");
                println!("      --help           display this help and exit");
                println!("      --version        output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "-a" | "--multiple" => multiple = true,
            "--zero" | "-z" => zero = true,
            "--" => saw_dashdash = true,
            s if s.starts_with("--suffix=") => {
                suffix = Some(s["--suffix=".len()..].to_string());
                multiple = true;
            }
            "-s" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 's'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                suffix = Some(args[i].clone());
                multiple = true;
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                let chars: Vec<char> = s[1..].chars().collect();
                let mut j = 0;
                while j < chars.len() {
                    match chars[j] {
                        'a' => multiple = true,
                        'z' => zero = true,
                        's' => {
                            // Rest of this arg is the suffix, or next arg
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 's'", TOOL_NAME);
                                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                                    process::exit(1);
                                }
                                suffix = Some(args[i].clone());
                            } else {
                                suffix = Some(rest);
                            }
                            multiple = true;
                            j = chars.len(); // consume rest
                            continue;
                        }
                        c => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, c);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    j += 1;
                }
            }
            _ => names.push(arg.clone()),
        }
        i += 1;
    }

    if names.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let terminator = if zero { '\0' } else { '\n' };

    if multiple || suffix.is_some() {
        // -a mode: all names are treated as NAMEs
        for name in &names {
            let result = basename(name, suffix.as_deref());
            print!("{}{}", result, terminator);
        }
    } else if names.len() == 1 {
        // basename NAME
        let result = basename(&names[0], None);
        print!("{}{}", result, terminator);
    } else if names.len() == 2 {
        // basename NAME SUFFIX
        let result = basename(&names[0], Some(&names[1]));
        print!("{}{}", result, terminator);
    } else {
        eprintln!("{}: extra operand '{}'", TOOL_NAME, names[2]);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }
}

/// Compute the basename of `name`, optionally stripping `suffix`.
/// Follows GNU coreutils behavior:
/// - Strip trailing slashes (unless the entire string is slashes)
/// - Return the last component
/// - Strip suffix if it matches and doesn't consume the entire basename
fn basename(name: &str, suffix: Option<&str>) -> String {
    // Empty string → empty string
    if name.is_empty() {
        return String::new();
    }

    let bytes = name.as_bytes();

    // Find the end: skip trailing slashes, but if everything is slashes, return "/"
    let mut end = bytes.len();
    while end > 1 && bytes[end - 1] == b'/' {
        end -= 1;
    }

    // If the entire string was slashes, return "/"
    if end == 1 && bytes[0] == b'/' {
        return "/".to_string();
    }

    // Find the start of the last component
    let slice = &name[..end];
    let base = match slice.rfind('/') {
        Some(pos) => &slice[pos + 1..],
        None => slice,
    };

    // Strip suffix if applicable
    if let Some(suf) = suffix
        && !suf.is_empty()
        && base.len() > suf.len()
        && base.ends_with(suf)
    {
        return base[..base.len() - suf.len()].to_string();
    }

    base.to_string()
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fbasename");
        Command::new(path)
    }

    #[test]
    fn test_basename_simple() {
        let output = cmd().arg("/usr/bin/sort").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "sort"
        );
    }

    #[test]
    fn test_basename_suffix() {
        let output = cmd().args(["/foo/bar.txt", ".txt"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "bar"
        );
    }

    #[test]
    fn test_basename_root() {
        let output = cmd().arg("/").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "/"
        );
    }

    #[test]
    fn test_basename_trailing_slash() {
        let output = cmd().arg("/usr/bin/").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "bin"
        );
    }

    #[test]
    fn test_basename_multiple() {
        let output = cmd()
            .args(["-a", "/usr/bin/sort", "/foo/bar", "hello"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines, vec!["sort", "bar", "hello"]);
    }

    #[test]
    fn test_basename_zero() {
        let output = cmd().args(["-z", "a/b"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = &output.stdout;
        // Should end with NUL, not newline
        assert!(stdout.ends_with(&[0u8]), "Should end with NUL byte");
        assert!(!stdout.ends_with(b"\n"), "Should not end with newline");
        // The actual value before the NUL should be "b"
        let text = String::from_utf8_lossy(&stdout[..stdout.len() - 1]);
        assert_eq!(text, "b");
    }

    #[test]
    fn test_basename_no_directory() {
        let output = cmd().arg("hello").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "hello"
        );
    }

    #[test]
    fn test_basename_suffix_flag() {
        let output = cmd()
            .args(["-s", ".txt", "/foo/bar.txt", "/baz/qux.txt"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines, vec!["bar", "qux"]);
    }

    #[test]
    fn test_basename_double_slash() {
        let output = cmd().arg("//").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end_matches('\n'),
            "/"
        );
    }

    #[test]
    fn test_basename_matches_gnu() {
        let test_cases = vec![
            vec!["/usr/bin/sort"],
            vec!["/foo/bar.txt", ".txt"],
            vec!["/"],
            vec!["//"],
            vec!["/usr/bin/"],
            vec!["hello"],
        ];
        for args in &test_cases {
            let gnu = Command::new("basename").args(args).output();
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
