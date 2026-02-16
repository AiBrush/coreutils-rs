#[cfg(not(unix))]
fn main() {
    eprintln!("mkfifo: only available on Unix");
    std::process::exit(1);
}

// fmkfifo — make FIFOs (named pipes)
//
// Usage: mkfifo [OPTION]... NAME...

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "mkfifo";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut mode: libc::mode_t = 0o666;
    let mut names: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        if saw_dashdash {
            names.push(arg);
            continue;
        }
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]... NAME...", TOOL_NAME);
                println!("Create named pipes (FIFOs) with the given NAMEs.");
                println!();
                println!("  -m, --mode=MODE  set file permission bits to MODE, not a=rw - umask");
                println!("      --help       display this help and exit");
                println!("      --version    output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--" => saw_dashdash = true,
            s if s.starts_with("--mode=") => {
                let val = &s["--mode=".len()..];
                mode = parse_octal_mode(val);
            }
            "--mode" | "-m" => {
                if let Some(val) = args.next() {
                    mode = parse_octal_mode(&val);
                } else {
                    eprintln!("{}: option '{}' requires an argument", TOOL_NAME, arg);
                    process::exit(1);
                }
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                let rest = &s[1..];
                if let Some(stripped) = rest.strip_prefix('m') {
                    // -mMODE or -m MODE
                    if stripped.is_empty() {
                        if let Some(val) = args.next() {
                            mode = parse_octal_mode(&val);
                        } else {
                            eprintln!("{}: option requires an argument -- 'm'", TOOL_NAME);
                            process::exit(1);
                        }
                    } else {
                        mode = parse_octal_mode(stripped);
                    }
                } else {
                    eprintln!("{}: invalid option -- '{}'", TOOL_NAME, &rest[..1]);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
            }
            _ => names.push(arg),
        }
    }

    if names.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let mut exit_code = 0;
    for name in &names {
        let c_name = match CString::new(name.as_str()) {
            Ok(c) => c,
            Err(_) => {
                eprintln!("{}: invalid name '{}'", TOOL_NAME, name);
                exit_code = 1;
                continue;
            }
        };
        // SAFETY: c_name is a valid null-terminated C string, mode is a valid mode_t
        let ret = unsafe { libc::mkfifo(c_name.as_ptr(), mode) };
        if ret != 0 {
            let e = std::io::Error::last_os_error();
            eprintln!(
                "{}: cannot create fifo '{}': {}",
                TOOL_NAME,
                name,
                coreutils_rs::common::io_error_msg(&e)
            );
            exit_code = 1;
        }
    }

    if exit_code != 0 {
        process::exit(exit_code);
    }
}

#[cfg(unix)]
fn parse_octal_mode(s: &str) -> libc::mode_t {
    libc::mode_t::from_str_radix(s, 8).unwrap_or_else(|_| {
        eprintln!("{}: invalid mode: '{}'", TOOL_NAME, s);
        process::exit(1);
    })
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fmkfifo");
        Command::new(path)
    }

    #[test]
    fn test_mkfifo_creates_fifo() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("testfifo");
        let output = cmd().arg(fifo.to_str().unwrap()).output().unwrap();
        assert_eq!(output.status.code(), Some(0));

        // Verify it's a FIFO
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            let meta = std::fs::symlink_metadata(&fifo).unwrap();
            assert!(meta.file_type().is_fifo(), "should be a FIFO");
        }
    }

    #[test]
    fn test_mkfifo_custom_mode() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("modefifo");
        let output = cmd()
            .args(["-m", "0644", fifo.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::symlink_metadata(&fifo).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o644, "mode should be 0644, got {:o}", mode);
        }
    }

    #[test]
    fn test_mkfifo_existing_name() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("existfifo");
        // Create it first
        cmd().arg(fifo.to_str().unwrap()).output().unwrap();
        // Try again — should fail
        let output = cmd().arg(fifo.to_str().unwrap()).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_mkfifo_multiple() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("fifo1");
        let f2 = dir.path().join("fifo2");
        let output = cmd()
            .args([f1.to_str().unwrap(), f2.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(f1.exists());
        assert!(f2.exists());
    }

    #[test]
    fn test_mkfifo_matches_gnu() {
        // Test error on existing file
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("existing");
        std::fs::write(&existing, "data").unwrap();

        let gnu = Command::new("mkfifo")
            .arg(existing.to_str().unwrap())
            .output();
        if let Ok(gnu) = gnu {
            // Recreate for our test
            let existing2 = dir.path().join("existing2");
            std::fs::write(&existing2, "data").unwrap();
            let ours = cmd().arg(existing2.to_str().unwrap()).output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
