#[cfg(not(unix))]
fn main() {
    eprintln!("tty: only available on Unix");
    std::process::exit(1);
}

// ftty — print the file name of the terminal connected to standard input
//
// If stdin is not a terminal, prints "not a tty" and exits 1.

#[cfg(unix)]
use std::ffi::CStr;
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "tty";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut silent = false;
    let args: Vec<String> = std::env::args().skip(1).collect();

    for arg in &args {
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]...", TOOL_NAME);
                println!("Print the file name of the terminal connected to standard input.");
                println!();
                println!("  -s, --silent, --quiet   print nothing, only return an exit status");
                println!("      --help              display this help and exit");
                println!("      --version           output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "-s" | "--silent" | "--quiet" => silent = true,
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                for ch in s[1..].chars() {
                    match ch {
                        's' => silent = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(2);
                        }
                    }
                }
            }
            _ => {
                eprintln!("{}: extra operand '{}'", TOOL_NAME, arg);
                eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                process::exit(2);
            }
        }
    }

    if unsafe { libc::isatty(0) } == 1 {
        if !silent {
            let name = unsafe { libc::ttyname(0) };
            if name.is_null() {
                // Shouldn't happen if isatty returned 1, but handle gracefully
                if !silent {
                    println!("not a tty");
                }
                process::exit(1);
            }
            // SAFETY: ttyname returned a valid non-null pointer
            let cstr = unsafe { CStr::from_ptr(name) };
            println!("{}", cstr.to_string_lossy());
        }
    } else {
        if !silent {
            println!("not a tty");
        }
        process::exit(1);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::{Command, Stdio};

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("ftty");
        Command::new(path)
    }

    #[test]
    fn test_tty_on_pipe() {
        // When stdin is a pipe (not a tty), should print "not a tty" and exit 1
        let output = cmd().stdin(Stdio::piped()).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "not a tty");
    }

    #[test]
    fn test_tty_silent_on_pipe() {
        let output = cmd().arg("-s").stdin(Stdio::piped()).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(
            output.stdout.is_empty(),
            "silent mode should produce no output"
        );
    }

    #[test]
    fn test_tty_matches_gnu() {
        let gnu = Command::new("tty").stdin(Stdio::piped()).output();
        if let Ok(gnu) = gnu {
            let ours = cmd().stdin(Stdio::piped()).output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
