#[cfg(not(unix))]
fn main() {
    eprintln!("nice: only available on Unix");
    std::process::exit(1);
}

// fnice — run a program with modified scheduling priority
//
// Usage: nice [OPTION] [COMMAND [ARG]...]

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::process;

/// Clear errno to 0 (portable across Unix platforms)
#[cfg(unix)]
fn clear_errno() {
    unsafe {
        *errno_ptr() = 0;
    }
}

/// Get current errno value (portable across Unix platforms)
#[cfg(unix)]
fn get_errno() -> i32 {
    unsafe { *errno_ptr() }
}

#[cfg(unix)]
unsafe fn errno_ptr() -> *mut i32 {
    #[cfg(target_os = "linux")]
    {
        unsafe { libc::__errno_location() }
    }
    #[cfg(target_os = "macos")]
    {
        unsafe { libc::__error() }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        unsafe { libc::__errno_location() }
    }
}

#[cfg(unix)]
const TOOL_NAME: &str = "nice";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut adjustment: i32 = 10;
    let mut command_start = None;
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION] [COMMAND [ARG]...]", TOOL_NAME);
                println!(
                    "Run COMMAND with an adjusted niceness, which affects process scheduling."
                );
                println!("With no COMMAND, print the current niceness.");
                println!();
                println!("  -n, --adjustment=N   add integer N to the niceness (default 10)");
                println!("      --help           display this help and exit");
                println!("      --version        output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            s if s.starts_with("--adjustment=") => {
                let val = &s["--adjustment=".len()..];
                adjustment = val.parse().unwrap_or_else(|_| {
                    eprintln!("{}: invalid adjustment '{}'", TOOL_NAME, val);
                    process::exit(125);
                });
            }
            "--adjustment" | "-n" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'n'", TOOL_NAME);
                    process::exit(125);
                }
                adjustment = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("{}: invalid adjustment '{}'", TOOL_NAME, args[i]);
                    process::exit(125);
                });
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                // Could be -n N or -N (numeric)
                let rest = &s[1..];
                if let Some(after_n) = rest.strip_prefix('n') {
                    if !after_n.is_empty() {
                        // -nN
                        adjustment = after_n.parse().unwrap_or_else(|_| {
                            eprintln!("{}: invalid adjustment '{}'", TOOL_NAME, after_n);
                            process::exit(125);
                        });
                    } else {
                        // -n N
                        i += 1;
                        if i >= args.len() {
                            eprintln!("{}: option requires an argument -- 'n'", TOOL_NAME);
                            process::exit(125);
                        }
                        adjustment = args[i].parse().unwrap_or_else(|_| {
                            eprintln!("{}: invalid adjustment '{}'", TOOL_NAME, args[i]);
                            process::exit(125);
                        });
                    }
                } else if let Ok(n) = rest.parse::<i32>() {
                    // -N (numeric adjustment shorthand, deprecated but GNU supports it)
                    adjustment = n;
                } else {
                    // Not a flag, start of command
                    command_start = Some(i);
                    break;
                }
            }
            "--" => {
                command_start = Some(i + 1);
                break;
            }
            _ => {
                command_start = Some(i);
                break;
            }
        }
        i += 1;
    }

    if command_start.is_none() || command_start.unwrap() >= args.len() {
        // No command — print current niceness + adjustment
        // SAFETY: getpriority with PRIO_PROCESS and 0 (current process) is always valid.
        // getpriority() can legitimately return -1, so we must clear errno first
        // and check it after the call to detect real errors.
        clear_errno();
        let current = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
        if current == -1 && get_errno() != 0 {
            eprintln!("{}: cannot get niceness", TOOL_NAME);
            process::exit(125);
        }
        println!("{}", current);
        return;
    }

    let cmd_start = command_start.unwrap();
    let command = &args[cmd_start];
    let command_args: Vec<&str> = args[cmd_start + 1..].iter().map(|s| s.as_str()).collect();

    // Apply niceness adjustment
    // SAFETY: nice() is safe to call with any integer
    let ret = unsafe { libc::nice(adjustment) };
    if ret == -1 && get_errno() != 0 {
        eprintln!(
            "{}: cannot set niceness: {}",
            TOOL_NAME,
            coreutils_rs::common::io_error_msg(&std::io::Error::last_os_error())
        );
        // Continue anyway — GNU nice still tries to exec
    }

    // Exec the command
    let err = std::process::Command::new(command)
        .args(&command_args)
        .exec();

    // If we get here, exec failed
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
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fnice");
        Command::new(path)
    }

    #[test]
    fn test_nice_default_adjustment() {
        // nice without command prints current niceness + 10
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let _n: i32 = stdout.trim().parse().expect("should output a number");
    }

    #[test]
    fn test_nice_custom_adjustment() {
        let output = cmd().args(["-n", "5"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
    }

    #[test]
    fn test_nice_runs_command() {
        let output = cmd().args(["echo", "hello"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "hello");
    }

    #[test]
    fn test_nice_matches_gnu() {
        let gnu = Command::new("nice").args(["echo", "test"]).output();
        if let Ok(gnu) = gnu {
            let ours = cmd().args(["echo", "test"]).output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
