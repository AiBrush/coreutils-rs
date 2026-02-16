// fpwd — print name of current/working directory
//
// -L: use PWD from environment (logical, default)
// -P: avoid all symlinks (physical)

use std::path::PathBuf;
use std::process;

const TOOL_NAME: &str = "pwd";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut physical = false;

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]...", TOOL_NAME);
                println!("Print the full filename of the current working directory.");
                println!();
                println!(
                    "  -L, --logical    use PWD from environment, even if it contains symlinks"
                );
                println!("  -P, --physical   avoid all symlinks");
                println!("      --help       display this help and exit");
                println!("      --version    output version information and exit");
                println!();
                println!("If no option is specified, -L is assumed.");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "-L" | "--logical" => physical = false,
            "-P" | "--physical" => physical = true,
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                // Parse combined short flags
                for ch in s[1..].chars() {
                    match ch {
                        'L' => physical = false,
                        'P' => physical = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            _ => {
                eprintln!("{}: ignoring non-option arguments", TOOL_NAME);
            }
        }
    }

    let path = if physical {
        std::env::current_dir().unwrap_or_else(|e| {
            eprintln!("{}: {}", TOOL_NAME, coreutils_rs::common::io_error_msg(&e));
            process::exit(1);
        })
    } else {
        // Logical: try PWD env var first, verify it matches current dir's inode
        logical_pwd().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|e| {
                eprintln!("{}: {}", TOOL_NAME, coreutils_rs::common::io_error_msg(&e));
                process::exit(1);
            })
        })
    };

    println!("{}", path.display());
}

/// Get the logical working directory from $PWD, verifying it points to the same inode.
fn logical_pwd() -> Option<PathBuf> {
    let pwd = std::env::var("PWD").ok()?;
    let pwd_path = PathBuf::from(&pwd);

    // PWD must be absolute
    if !pwd_path.is_absolute() {
        return None;
    }

    // Verify PWD and current_dir point to the same file
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let pwd_meta = std::fs::metadata(&pwd_path).ok()?;
        let cwd_meta = std::fs::metadata(".").ok()?;
        if pwd_meta.dev() == cwd_meta.dev() && pwd_meta.ino() == cwd_meta.ino() {
            Some(pwd_path)
        } else {
            None
        }
    }

    #[cfg(not(unix))]
    {
        Some(pwd_path)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fpwd");
        Command::new(path)
    }

    #[test]
    fn test_pwd_prints_directory() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = stdout.trim();
        assert!(
            path.starts_with('/'),
            "Should print absolute path, got: {}",
            path
        );
    }

    #[test]
    fn test_pwd_physical() {
        let output = cmd().arg("-P").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.trim().is_empty());
    }

    #[test]
    fn test_pwd_logical() {
        let output = cmd().arg("-L").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.trim().is_empty());
    }

    #[test]
    fn test_pwd_matches_gnu() {
        let gnu = Command::new("pwd").arg("-P").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("-P").output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch for -P");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
