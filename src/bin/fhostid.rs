#[cfg(not(unix))]
fn main() {
    eprintln!("hostid: only available on Unix");
    std::process::exit(1);
}

// fhostid — print the numeric identifier for the current host
//
// Prints the host identifier as an 8-character lowercase hexadecimal number.

#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "hostid";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(arg) = args.first() {
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]", TOOL_NAME);
                println!("Print the numeric identifier (in hexadecimal) for the current host.");
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
                eprintln!("{}: unrecognized option '{}'", TOOL_NAME, arg);
                eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                process::exit(1);
            }
        }
    }

    let id = unsafe { libc::gethostid() };
    println!("{:08x}", id as u32);
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fhostid");
        Command::new(path)
    }

    #[test]
    fn test_hostid_format() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let trimmed = stdout.trim();
        assert_eq!(
            trimmed.len(),
            8,
            "hostid should be 8 hex chars, got: '{}'",
            trimmed
        );
        assert!(
            trimmed.chars().all(|c| c.is_ascii_hexdigit()),
            "hostid should be hex, got: '{}'",
            trimmed
        );
        // Verify lowercase
        assert_eq!(trimmed, trimmed.to_lowercase());
    }

    #[test]
    fn test_hostid_matches_gnu() {
        let gnu = Command::new("hostid").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
