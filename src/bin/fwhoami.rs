#[cfg(not(unix))]
fn main() {
    eprintln!("whoami: only available on Unix");
    std::process::exit(1);
}

// fwhoami — print effective user name
//
// Uses geteuid() + getpwuid() to get the effective user's name.

#[cfg(unix)]
use std::ffi::CStr;
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "whoami";
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
                println!("Print the user name associated with the current effective user ID.");
                println!("Same as id -un.");
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

    let uid = unsafe { libc::geteuid() };
    let pw = unsafe { libc::getpwuid(uid) };
    if pw.is_null() {
        eprintln!("{}: cannot find name for user ID {}", TOOL_NAME, uid);
        process::exit(1);
    }

    // SAFETY: getpwuid returned a valid non-null pointer, pw_name is a valid C string
    let name = unsafe { CStr::from_ptr((*pw).pw_name) };
    println!("{}", name.to_string_lossy());
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fwhoami");
        Command::new(path)
    }

    #[test]
    fn test_whoami_prints_username() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let name = stdout.trim();
        assert!(!name.is_empty(), "whoami should output a non-empty name");
    }

    #[test]
    fn test_whoami_matches_gnu() {
        let gnu = Command::new("whoami").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }

    #[test]
    fn test_whoami_no_args_accepted() {
        let output = cmd().arg("extra").output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }
}
