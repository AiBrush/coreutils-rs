#[cfg(not(unix))]
fn main() {
    eprintln!("env: only available on Unix");
    std::process::exit(1);
}

// fenv -- run a program in a modified environment
//
// Usage: env [OPTION]... [-] [NAME=VALUE]... [COMMAND [ARG]...]

#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "env";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut ignore_env = false;
    let mut unsets: Vec<String> = Vec::new();
    let mut sets: Vec<(String, String)> = Vec::new();
    let mut null_terminated = false;
    let mut chdir: Option<String> = None;
    let mut command_start: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        // Once we find a command, everything after is command + args
        if command_start.is_some() {
            break;
        }

        match arg.as_str() {
            "--help" => {
                println!(
                    "Usage: {} [OPTION]... [-] [NAME=VALUE]... [COMMAND [ARG]...]",
                    TOOL_NAME
                );
                println!("Set each NAME to VALUE in the environment and run COMMAND.");
                println!();
                println!("  -i, --ignore-environment  start with an empty environment");
                println!("  -0, --null           end each output line with NUL, not newline");
                println!("  -u, --unset=NAME     remove variable from the environment");
                println!("  -C, --chdir=DIR      change working directory to DIR");
                println!("      --help           display this help and exit");
                println!("      --version        output version information and exit");
                println!();
                println!("A mere - implies -i.  If no COMMAND, print the resulting environment.");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "-i" | "--ignore-environment" => ignore_env = true,
            "-" => ignore_env = true,
            "-0" | "--null" => null_terminated = true,
            "--" => {
                i += 1;
                if i < args.len() {
                    command_start = Some(i);
                }
                break;
            }
            s if s.starts_with("--unset=") => {
                let name = &s["--unset=".len()..];
                unsets.push(name.to_string());
            }
            s if s.starts_with("--chdir=") => {
                let dir = &s["--chdir=".len()..];
                chdir = Some(dir.to_string());
            }
            "-u" | "--unset" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'u'", TOOL_NAME);
                    process::exit(125);
                }
                unsets.push(args[i].clone());
            }
            "-C" | "--chdir" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'C'", TOOL_NAME);
                    process::exit(125);
                }
                chdir = Some(args[i].clone());
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                // Combined short flags like -i0u
                let chars: Vec<char> = s[1..].chars().collect();
                let mut j = 0;
                while j < chars.len() {
                    match chars[j] {
                        'i' => ignore_env = true,
                        '0' => null_terminated = true,
                        'u' => {
                            // Rest of this arg or next arg is the name
                            if j + 1 < chars.len() {
                                let name: String = chars[j + 1..].iter().collect();
                                unsets.push(name);
                                j = chars.len(); // consumed rest
                                continue;
                            } else {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 'u'", TOOL_NAME);
                                    process::exit(125);
                                }
                                unsets.push(args[i].clone());
                            }
                        }
                        'C' => {
                            if j + 1 < chars.len() {
                                let dir: String = chars[j + 1..].iter().collect();
                                chdir = Some(dir);
                                j = chars.len();
                                continue;
                            } else {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 'C'", TOOL_NAME);
                                    process::exit(125);
                                }
                                chdir = Some(args[i].clone());
                            }
                        }
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, chars[j]);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(125);
                        }
                    }
                    j += 1;
                }
            }
            s if s.contains('=') => {
                // NAME=VALUE
                if let Some(pos) = s.find('=') {
                    let name = &s[..pos];
                    let value = &s[pos + 1..];
                    sets.push((name.to_string(), value.to_string()));
                }
            }
            _ => {
                // This is the start of COMMAND
                command_start = Some(i);
                break;
            }
        }
        i += 1;
    }

    // Apply environment modifications
    if ignore_env {
        // Clear environment
        let keys: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
        for k in keys {
            // SAFETY: we are clearing environment variables by name; name is valid
            unsafe { std::env::remove_var(&k) };
        }
    }

    for name in &unsets {
        // SAFETY: we are unsetting the environment variable by name; name is valid
        unsafe { std::env::remove_var(name) };
    }

    for (name, value) in &sets {
        // SAFETY: we control both name and value, and neither is empty or contains NUL
        unsafe { std::env::set_var(name, value) };
    }

    // Change directory if requested
    if let Some(ref dir) = chdir
        && let Err(e) = std::env::set_current_dir(dir)
    {
        eprintln!(
            "{}: cannot change directory to '{}': {}",
            TOOL_NAME,
            dir,
            coreutils_rs::common::io_error_msg(&e)
        );
        process::exit(125);
    }

    if let Some(start) = command_start {
        // Execute command
        let command = &args[start];
        let command_args = &args[start + 1..];

        let err = std::process::Command::new(command)
            .args(command_args)
            .exec();

        // exec() only returns on failure
        let code = if err.kind() == std::io::ErrorKind::NotFound {
            127
        } else {
            126
        };
        eprintln!(
            "{}: '{}': {}",
            TOOL_NAME,
            command,
            coreutils_rs::common::io_error_msg(&err)
        );
        process::exit(code);
    } else {
        // No command: print environment
        let terminator = if null_terminated { '\0' } else { '\n' };
        for (key, value) in std::env::vars() {
            print!("{}={}{}", key, value, terminator);
        }
    }
}

// Import CommandExt for exec()
#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fenv");
        Command::new(path)
    }

    #[test]
    fn test_print_env() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Should contain at least PATH
        assert!(stdout.contains("PATH="), "Should print env vars");
    }

    #[test]
    fn test_ignore_environment() {
        let output = cmd().args(["-i", "TEST_VAR=hello"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        // With -i, only TEST_VAR should be set
        assert!(stdout.contains("TEST_VAR=hello"));
        assert!(!stdout.contains("PATH="), "PATH should be cleared with -i");
    }

    #[test]
    fn test_dash_alias_for_i() {
        let output = cmd().args(["-", "MY_VAR=world"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("MY_VAR=world"));
        assert!(!stdout.contains("PATH="), "PATH should be cleared with -");
    }

    #[test]
    fn test_unset() {
        let output = cmd().args(["-u", "PATH"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Check that no line starts with "PATH=" (other vars may contain PATH in their names)
        let has_path = stdout.lines().any(|line| line.starts_with("PATH="));
        assert!(!has_path, "PATH should be unset");
    }

    #[test]
    fn test_set_var() {
        let output = cmd().args(["MY_TEST_VAR=12345"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("MY_TEST_VAR=12345"));
    }

    #[test]
    fn test_run_command() {
        let output = cmd().args(["echo", "hello"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "hello");
    }

    #[test]
    fn test_run_command_with_var() {
        let output = cmd()
            .args(["MY_VAR=test", "sh", "-c", "echo $MY_VAR"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "test");
    }

    #[test]
    fn test_null_terminator() {
        let output = cmd().args(["-i", "-0", "A=1", "B=2"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = &output.stdout;
        assert!(stdout.contains(&0u8), "Should contain NUL bytes");
        // Should not end with newline between entries
        let s = String::from_utf8_lossy(stdout);
        assert!(s.contains("A=1\0"));
        assert!(s.contains("B=2\0"));
    }

    #[test]
    fn test_chdir() {
        let output = cmd().args(["-C", "/tmp", "pwd"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        // On macOS, /tmp is a symlink to /private/tmp
        let expected =
            std::fs::canonicalize("/tmp").unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
        assert_eq!(stdout.trim(), expected.to_str().unwrap());
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("(fcoreutils)"));
    }

    #[test]
    fn test_matches_gnu_run_command() {
        let gnu = Command::new("env").args(["echo", "test"]).output();
        if let Ok(gnu) = gnu {
            let ours = cmd().args(["echo", "test"]).output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }

    #[test]
    fn test_nonexistent_command() {
        let output = cmd().arg("nonexistent_cmd_999").output().unwrap();
        assert_eq!(output.status.code(), Some(127));
    }
}
