#[cfg(not(unix))]
fn main() {
    eprintln!("nohup: only available on Unix");
    std::process::exit(1);
}

// fnohup — run a command immune to hangups, with output to a non-tty
//
// Usage: nohup COMMAND [ARG]...

#[cfg(unix)]
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "nohup";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(125);
    }

    match args[0].as_str() {
        "--help" => {
            println!("Usage: {} COMMAND [ARG]...", TOOL_NAME);
            println!("  or:  {} OPTION", TOOL_NAME);
            println!("Run COMMAND, ignoring hangup signals.");
            println!();
            println!("If standard output is a terminal, append output to 'nohup.out' if possible,");
            println!("'$HOME/nohup.out' otherwise.");
            println!("If standard error is a terminal, redirect it to standard output.");
            println!(
                "To save output to FILE, use '{} COMMAND > FILE'.",
                TOOL_NAME
            );
            println!();
            println!("      --help     display this help and exit");
            println!("      --version  output version information and exit");
            return;
        }
        "--version" => {
            println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
            return;
        }
        _ => {}
    }

    // Ignore SIGHUP
    // SAFETY: setting SIGHUP to SIG_IGN is safe
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    let command = &args[0];
    let command_args: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();

    // If stdout is a terminal, redirect to nohup.out
    let _stdout_file: Option<File> = if unsafe { libc::isatty(1) } == 1 {
        let file = open_nohup_out();
        match file {
            Some(f) => {
                // Redirect stdout to this file
                // SAFETY: dup2 with valid fds
                unsafe {
                    libc::dup2(f.as_raw_fd(), 1);
                }
                eprintln!(
                    "{}: ignoring input and appending output to 'nohup.out'",
                    TOOL_NAME
                );
                Some(f)
            }
            None => {
                eprintln!(
                    "{}: failed to open 'nohup.out': Permission denied or no suitable path",
                    TOOL_NAME
                );
                process::exit(127);
            }
        }
    } else {
        None
    };

    // If stderr is a terminal, redirect to stdout
    if unsafe { libc::isatty(2) } == 1 {
        // SAFETY: dup2 with valid fds
        unsafe {
            libc::dup2(1, 2);
        }
    }

    // Exec the command
    let err = std::process::Command::new(command)
        .args(&command_args)
        .exec();

    let code = if err.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    };
    eprintln!(
        "{}: failed to run command '{}': {}",
        TOOL_NAME,
        command,
        coreutils_rs::common::io_error_msg(&err)
    );
    process::exit(code);
}

#[cfg(unix)]
fn open_nohup_out() -> Option<File> {
    // Try current directory first
    if let Ok(f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("nohup.out")
    {
        return Some(f);
    }

    // Try $HOME/nohup.out
    if let Ok(home) = std::env::var("HOME") {
        let path = std::path::Path::new(&home).join("nohup.out");
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(path) {
            return Some(f);
        }
    }

    None
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fnohup");
        Command::new(path)
    }

    #[test]
    fn test_nohup_runs_command() {
        let output = cmd().args(["echo", "hello"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "hello");
    }

    #[test]
    fn test_nohup_missing_command() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(125));
    }

    #[test]
    fn test_nohup_nonexistent_command() {
        let output = cmd().arg("nonexistent_cmd_12345").output().unwrap();
        assert_eq!(output.status.code(), Some(127));
    }

    #[test]
    fn test_nohup_matches_gnu() {
        let gnu = Command::new("nohup").args(["echo", "test"]).output();
        if let Ok(gnu) = gnu {
            let ours = cmd().args(["echo", "test"]).output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
            // stdout content should match (both echo "test\n")
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
        }
    }
}
