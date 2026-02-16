#[cfg(not(unix))]
fn main() {
    eprintln!("timeout: only available on Unix");
    std::process::exit(1);
}

// ftimeout -- run a command with a time limit
//
// Usage: timeout [OPTION] DURATION COMMAND [ARG]...

#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "timeout";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Exit code when the command times out.
#[cfg(unix)]
const EXIT_TIMEOUT: i32 = 124;
/// Exit code when timeout itself fails.
#[cfg(unix)]
const EXIT_FAILURE: i32 = 125;
/// Exit code when the command cannot be executed.
#[cfg(unix)]
const EXIT_CANNOT_INVOKE: i32 = 126;
/// Exit code when the command is not found.
#[cfg(unix)]
const EXIT_ENOENT: i32 = 127;

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut signal_name = "TERM".to_string();
    let mut kill_after: Option<f64> = None;
    let mut foreground = false;
    let mut preserve_status = false;
    let mut verbose = false;
    let mut positional_start: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION] DURATION COMMAND [ARG]...", TOOL_NAME);
                println!("Start COMMAND, and kill it if still running after DURATION.");
                println!();
                println!("  -s, --signal=SIGNAL    specify the signal to be sent on timeout;");
                println!("                           SIGNAL may be a name like 'HUP' or a number;");
                println!("                           see 'kill -l' for a list of signals");
                println!("  -k, --kill-after=DURATION");
                println!(
                    "                         also send a KILL signal if COMMAND is still running"
                );
                println!("                           this long after the initial signal was sent");
                println!(
                    "      --foreground       when not running timeout directly from a shell prompt,"
                );
                println!(
                    "                           allow COMMAND to read from the TTY and get TTY signals"
                );
                println!(
                    "      --preserve-status  exit with the same status as COMMAND, even when the"
                );
                println!("                           command times out");
                println!(
                    "  -v, --verbose          diagnose to stderr any signal sent upon timeout"
                );
                println!("      --help             display this help and exit");
                println!("      --version          output version information and exit");
                println!();
                println!("DURATION is a floating point number with an optional suffix:");
                println!(
                    "'s' for seconds (the default), 'm' for minutes, 'h' for hours or 'd' for days."
                );
                println!("A duration of 0 disables the associated timeout.");
                println!();
                println!(
                    "If the command times out, and --preserve-status is not set, then exit with"
                );
                println!("status 124.  Otherwise, exit with the status of COMMAND.  If no signal");
                println!("is specified, send the TERM signal upon timeout.  The TERM signal kills");
                println!(
                    "any process that does not block or catch that signal.  It may be necessary"
                );
                println!(
                    "to use the KILL (9) signal, since this signal cannot be caught, in which"
                );
                println!("case the exit status is 128+9 rather than 124.");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--foreground" => foreground = true,
            "--preserve-status" => preserve_status = true,
            "-v" | "--verbose" => verbose = true,
            s if s.starts_with("--signal=") => {
                signal_name = s["--signal=".len()..].to_string();
            }
            s if s.starts_with("--kill-after=") => {
                let val = &s["--kill-after=".len()..];
                kill_after = Some(parse_duration(val).unwrap_or_else(|| {
                    eprintln!("{}: invalid time interval '{}'", TOOL_NAME, val);
                    process::exit(EXIT_FAILURE);
                }));
            }
            "-s" | "--signal" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 's'", TOOL_NAME);
                    process::exit(EXIT_FAILURE);
                }
                signal_name = args[i].clone();
            }
            "-k" | "--kill-after" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'k'", TOOL_NAME);
                    process::exit(EXIT_FAILURE);
                }
                kill_after = Some(parse_duration(&args[i]).unwrap_or_else(|| {
                    eprintln!("{}: invalid time interval '{}'", TOOL_NAME, args[i]);
                    process::exit(EXIT_FAILURE);
                }));
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                let rest = &s[1..];
                let chars: Vec<char> = rest.chars().collect();
                let mut j = 0;
                while j < chars.len() {
                    match chars[j] {
                        'v' => verbose = true,
                        's' => {
                            if j + 1 < chars.len() {
                                signal_name = chars[j + 1..].iter().collect();
                                j = chars.len();
                                continue;
                            } else {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 's'", TOOL_NAME);
                                    process::exit(EXIT_FAILURE);
                                }
                                signal_name = args[i].clone();
                            }
                        }
                        'k' => {
                            if j + 1 < chars.len() {
                                let val: String = chars[j + 1..].iter().collect();
                                kill_after = Some(parse_duration(&val).unwrap_or_else(|| {
                                    eprintln!("{}: invalid time interval '{}'", TOOL_NAME, val);
                                    process::exit(EXIT_FAILURE);
                                }));
                                j = chars.len();
                                continue;
                            } else {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 'k'", TOOL_NAME);
                                    process::exit(EXIT_FAILURE);
                                }
                                kill_after = Some(parse_duration(&args[i]).unwrap_or_else(|| {
                                    eprintln!("{}: invalid time interval '{}'", TOOL_NAME, args[i]);
                                    process::exit(EXIT_FAILURE);
                                }));
                            }
                        }
                        _ => {
                            // This might be start of positional args
                            positional_start = Some(i);
                            break;
                        }
                    }
                    j += 1;
                }
                if positional_start.is_some() {
                    break;
                }
            }
            "--" => {
                i += 1;
                if i < args.len() {
                    positional_start = Some(i);
                }
                break;
            }
            _ => {
                positional_start = Some(i);
                break;
            }
        }
        i += 1;
    }

    let start = positional_start.unwrap_or_else(|| {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(EXIT_FAILURE);
    });

    if start >= args.len() {
        eprintln!("{}: missing operand", TOOL_NAME);
        process::exit(EXIT_FAILURE);
    }

    let duration = parse_duration(&args[start]).unwrap_or_else(|| {
        eprintln!("{}: invalid time interval '{}'", TOOL_NAME, args[start]);
        process::exit(EXIT_FAILURE);
    });

    if start + 1 >= args.len() {
        eprintln!("{}: missing operand", TOOL_NAME);
        process::exit(EXIT_FAILURE);
    }

    let command = &args[start + 1];
    let command_args: Vec<&str> = args[start + 2..].iter().map(|s| s.as_str()).collect();

    let sig = parse_signal(&signal_name).unwrap_or_else(|| {
        eprintln!("{}: invalid signal '{}'", TOOL_NAME, signal_name);
        process::exit(EXIT_FAILURE);
    });

    // Fork
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        eprintln!("{}: fork: {}", TOOL_NAME, std::io::Error::last_os_error());
        process::exit(EXIT_FAILURE);
    }

    if pid == 0 {
        // Child: exec the command
        if !foreground {
            // Put child in its own process group
            unsafe {
                libc::setpgid(0, 0);
            }
        }

        // Close inherited file descriptors to prevent leaks
        for fd in 3..1024 {
            unsafe {
                libc::close(fd);
            }
        }

        let c_command =
            std::ffi::CString::new(command.as_str()).unwrap_or_else(|_| process::exit(EXIT_ENOENT));
        let mut c_args: Vec<std::ffi::CString> = Vec::with_capacity(command_args.len() + 1);
        c_args.push(c_command.clone());
        for a in &command_args {
            c_args.push(std::ffi::CString::new(*a).unwrap_or_else(|_| process::exit(EXIT_ENOENT)));
        }
        let c_argv: Vec<*const libc::c_char> = c_args
            .iter()
            .map(|s| s.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();

        unsafe {
            libc::execvp(c_command.as_ptr(), c_argv.as_ptr());
        }

        // If execvp returns, it failed
        let err = std::io::Error::last_os_error();
        let code = if err.kind() == std::io::ErrorKind::NotFound {
            EXIT_ENOENT
        } else {
            EXIT_CANNOT_INVOKE
        };
        eprintln!(
            "{}: failed to run command '{}': {}",
            TOOL_NAME,
            command,
            coreutils_rs::common::io_error_msg(&err)
        );
        process::exit(code);
    }

    // Parent: set up timeout
    let child_pid = pid;
    let target_pid = if foreground { child_pid } else { -child_pid };

    // Install signal handlers to forward signals to child
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    // Wait for child with timeout using a polling approach
    let duration_nanos = (duration * 1_000_000_000.0) as u128;
    let start_time = std::time::Instant::now();
    let mut timed_out = false;
    let mut status: libc::c_int = 0;

    loop {
        let ret = unsafe { libc::waitpid(child_pid, &mut status, libc::WNOHANG) };
        if ret == child_pid {
            // Child exited
            break;
        }
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            // Child no longer exists
            break;
        }

        // Check timeout (duration of 0 means no timeout)
        if duration > 0.0 && start_time.elapsed().as_nanos() >= duration_nanos {
            timed_out = true;
            if verbose {
                eprintln!(
                    "{}: sending signal {} to command '{}'",
                    TOOL_NAME, signal_name, command
                );
            }
            let ret = unsafe { libc::kill(target_pid, sig) };
            if ret != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ESRCH) {
                    // Process already exited before we could signal it
                    timed_out = false;
                }
            }
            break;
        }

        // Sleep briefly to avoid busy-wait
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    if timed_out {
        // Wait for child to die after the initial signal
        if let Some(kill_secs) = kill_after {
            // Poll until child exits or kill-after period elapses
            let kill_nanos = (kill_secs * 1_000_000_000.0) as u128;
            let kill_start = std::time::Instant::now();
            loop {
                let ret = unsafe { libc::waitpid(child_pid, &mut status, libc::WNOHANG) };
                if ret == child_pid || ret < 0 {
                    break;
                }
                if kill_start.elapsed().as_nanos() >= kill_nanos {
                    if verbose {
                        eprintln!(
                            "{}: sending signal KILL to command '{}'",
                            TOOL_NAME, command
                        );
                    }
                    let kill_ret = unsafe { libc::kill(target_pid, libc::SIGKILL) };
                    if kill_ret == 0 {
                        unsafe {
                            libc::waitpid(child_pid, &mut status, 0);
                        }
                    }
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        } else {
            // No kill-after: blocking wait for child to exit after signal
            // This matches GNU timeout behavior
            unsafe {
                libc::waitpid(child_pid, &mut status, 0);
            }
        }

        if preserve_status {
            process::exit(status_to_code(status));
        } else {
            if sig != libc::SIGTERM {
                unsafe {
                    libc::signal(sig, libc::SIG_DFL);
                    libc::raise(sig);
                }
                // If raise didn't kill us (shouldn't happen for uncatchable signals)
                process::exit(128 + sig as i32);
            }
            process::exit(EXIT_TIMEOUT);
        }
    }

    // Child exited normally (before timeout)
    process::exit(status_to_code(status));
}

#[cfg(unix)]
fn status_to_code(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        EXIT_FAILURE
    }
}

#[cfg(unix)]
fn parse_duration(s: &str) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    let (num, suffix) = if let Some(stripped) = s.strip_suffix('s') {
        (stripped, 's')
    } else if let Some(stripped) = s.strip_suffix('m') {
        (stripped, 'm')
    } else if let Some(stripped) = s.strip_suffix('h') {
        (stripped, 'h')
    } else if let Some(stripped) = s.strip_suffix('d') {
        (stripped, 'd')
    } else {
        (s, 's')
    };

    let value: f64 = num.parse().ok()?;
    if value < 0.0 {
        return None;
    }

    let multiplier = match suffix {
        's' => 1.0,
        'm' => 60.0,
        'h' => 3600.0,
        'd' => 86400.0,
        _ => return None,
    };

    Some(value * multiplier)
}

#[cfg(unix)]
fn parse_signal(name: &str) -> Option<libc::c_int> {
    // Try numeric first
    if let Ok(n) = name.parse::<libc::c_int>() {
        return Some(n);
    }

    // Strip SIG prefix if present
    let upper = name.to_uppercase();
    let sig_name = if let Some(stripped) = upper.strip_prefix("SIG") {
        stripped
    } else {
        &upper
    };

    match sig_name {
        "HUP" => Some(libc::SIGHUP),
        "INT" => Some(libc::SIGINT),
        "QUIT" => Some(libc::SIGQUIT),
        "ILL" => Some(libc::SIGILL),
        "TRAP" => Some(libc::SIGTRAP),
        "ABRT" | "IOT" => Some(libc::SIGABRT),
        "BUS" => Some(libc::SIGBUS),
        "FPE" => Some(libc::SIGFPE),
        "KILL" => Some(libc::SIGKILL),
        "USR1" => Some(libc::SIGUSR1),
        "SEGV" => Some(libc::SIGSEGV),
        "USR2" => Some(libc::SIGUSR2),
        "PIPE" => Some(libc::SIGPIPE),
        "ALRM" => Some(libc::SIGALRM),
        "TERM" => Some(libc::SIGTERM),
        "CHLD" => Some(libc::SIGCHLD),
        "CONT" => Some(libc::SIGCONT),
        "STOP" => Some(libc::SIGSTOP),
        "TSTP" => Some(libc::SIGTSTP),
        "TTIN" => Some(libc::SIGTTIN),
        "TTOU" => Some(libc::SIGTTOU),
        "URG" => Some(libc::SIGURG),
        "XCPU" => Some(libc::SIGXCPU),
        "XFSZ" => Some(libc::SIGXFSZ),
        "VTALRM" => Some(libc::SIGVTALRM),
        "PROF" => Some(libc::SIGPROF),
        "WINCH" => Some(libc::SIGWINCH),
        "IO" | "POLL" => Some(libc::SIGIO),
        "SYS" => Some(libc::SIGSYS),
        _ => None,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("ftimeout");
        Command::new(path)
    }

    #[test]
    fn test_command_completes_before_timeout() {
        let output = cmd().args(["10", "echo", "hello"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "hello");
    }

    #[test]
    fn test_command_times_out() {
        let output = cmd().args(["0.1", "sleep", "10"]).output().unwrap();
        // Exit code should be 124 (timed out)
        assert_eq!(output.status.code(), Some(124));
    }

    #[test]
    fn test_kill_after() {
        let start = std::time::Instant::now();
        let output = cmd()
            .args(["-k", "0.1", "0.1", "sleep", "100"])
            .output()
            .unwrap();
        let elapsed = start.elapsed();
        // Should complete relatively quickly (timeout + kill_after)
        assert!(elapsed.as_secs() < 5, "Should not hang");
        // Exit code is 124 (timed out) or 137 (killed by SIGKILL = 128+9)
        let code = output.status.code().unwrap();
        assert!(
            code == 124 || code == 137,
            "Expected 124 or 137, got {}",
            code
        );
    }

    #[test]
    fn test_preserve_status() {
        let output = cmd()
            .args(["--preserve-status", "0.1", "sleep", "10"])
            .output()
            .unwrap();
        let code = output.status.code().unwrap();
        // With --preserve-status, should get the signal exit code, not 124
        // SIGTERM = 15, so 128 + 15 = 143
        assert_ne!(code, 124, "Should NOT be 124 with --preserve-status");
    }

    #[test]
    fn test_signal_flag() {
        let output = cmd()
            .args(["-s", "KILL", "0.1", "sleep", "10"])
            .output()
            .unwrap();
        // ftimeout raises SIGKILL on itself, so the process dies by signal
        // status.code() returns None for signal deaths on Unix
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            let sig = output.status.signal();
            assert_eq!(sig, Some(9), "Expected signal 9 (SIGKILL), got {:?}", sig);
        }
        #[cfg(not(unix))]
        {
            let code = output.status.code().unwrap();
            assert_eq!(code, 137, "Expected 137 (128+SIGKILL), got {}", code);
        }
    }

    #[test]
    fn test_duration_with_suffix() {
        // 0.1s should work the same as 0.1
        let output = cmd().args(["0.1s", "sleep", "10"]).output().unwrap();
        assert_eq!(output.status.code(), Some(124));
    }

    #[test]
    fn test_zero_duration() {
        // Duration of 0 means no timeout
        let output = cmd().args(["0", "echo", "no timeout"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "no timeout");
    }

    #[test]
    fn test_command_not_found() {
        let output = cmd()
            .args(["10", "nonexistent_cmd_xyz_999"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(127));
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
    fn test_missing_operand() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(125));
    }

    #[test]
    fn test_matches_gnu_exit_codes_success() {
        let gnu = Command::new("timeout")
            .args(["10", "echo", "test"])
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd().args(["10", "echo", "test"]).output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }

    #[test]
    fn test_matches_gnu_exit_codes_timeout() {
        let gnu = Command::new("timeout")
            .args(["0.1", "sleep", "10"])
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd().args(["0.1", "sleep", "10"]).output().unwrap();
            assert_eq!(
                ours.status.code(),
                gnu.status.code(),
                "Exit code mismatch on timeout"
            );
        }
    }

    #[test]
    fn test_matches_gnu_exit_codes_not_found() {
        let gnu = Command::new("timeout")
            .args(["10", "nonexistent_cmd_xyz_999"])
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd()
                .args(["10", "nonexistent_cmd_xyz_999"])
                .output()
                .unwrap();
            assert_eq!(
                ours.status.code(),
                gnu.status.code(),
                "Exit code mismatch for not found"
            );
        }
    }
}
