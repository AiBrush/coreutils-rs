#[cfg(not(unix))]
fn main() {
    eprintln!("logname: only available on Unix");
    std::process::exit(1);
}

// flogname — print the user's login name
//
// Uses getlogin() to retrieve the login name from utmp.

#[cfg(unix)]
use std::ffi::CStr;
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "logname";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(arg) = args.first() {
        match arg.as_str() {
            "--help" => {
                println!("Usage: {}", TOOL_NAME);
                println!("  or:  {} OPTION", TOOL_NAME);
                println!("Print the name of the current user.");
                println!();
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            _ => {
                eprintln!("{}: extra operand '{}'", TOOL_NAME, arg);
                eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                process::exit(1);
            }
        }
    }

    let login = unsafe { libc::getlogin() };
    if login.is_null() {
        // GNU logname only uses getlogin(), does not fall back to LOGNAME
        eprintln!("{}: no login name", TOOL_NAME);
        process::exit(1);
    } else {
        // SAFETY: getlogin() returned a valid non-null pointer
        let name = unsafe { CStr::from_ptr(login) };
        println!("{}", name.to_string_lossy());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("flogname");
        Command::new(path)
    }

    #[test]
    fn test_logname_prints_name() {
        let output = cmd().output().unwrap();
        // May fail in some CI environments without a terminal
        if output.status.code() == Some(0) {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let name = stdout.trim();
            assert!(!name.is_empty(), "logname should output a non-empty name");
        }
    }

    #[test]
    fn test_logname_exit_0_or_1() {
        let output = cmd().output().unwrap();
        let code = output.status.code().unwrap();
        assert!(
            code == 0 || code == 1,
            "exit code should be 0 or 1, got {}",
            code
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_logname_matches_gnu() {
        let gnu = Command::new("logname").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().output().unwrap();
            // Both should succeed or both fail
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
            if gnu.status.success() {
                assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            }
        }
    }
}
