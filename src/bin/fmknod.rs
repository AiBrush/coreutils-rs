#[cfg(not(unix))]
fn main() {
    eprintln!("mknod: only available on Unix");
    std::process::exit(1);
}

// fmknod — make block or character special files, or FIFOs
//
// Usage: mknod [OPTION]... NAME TYPE [MAJOR MINOR]

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "mknod";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut mode: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        if saw_dashdash {
            positional.push(arg);
            continue;
        }
        match arg.as_str() {
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--" => saw_dashdash = true,
            s if s.starts_with("--mode=") => {
                let val = &s["--mode=".len()..];
                mode = Some(val.to_string());
            }
            "--mode" | "-m" => {
                if let Some(val) = args.next() {
                    mode = Some(val);
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
                            mode = Some(val);
                        } else {
                            eprintln!("{}: option requires an argument -- 'm'", TOOL_NAME);
                            process::exit(1);
                        }
                    } else {
                        mode = Some(stripped.to_string());
                    }
                } else {
                    eprintln!("{}: invalid option -- '{}'", TOOL_NAME, &rest[..1]);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
            }
            _ => positional.push(arg),
        }
    }

    if positional.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    if positional.len() < 2 {
        eprintln!("{}: missing operand after '{}'", TOOL_NAME, positional[0]);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let name = &positional[0];
    let node_type = &positional[1];

    match node_type.as_str() {
        "p" => {
            // FIFO: no major/minor allowed
            if positional.len() > 2 {
                eprintln!(
                    "{}: '{}: Superfluous argument '{}'",
                    TOOL_NAME, name, positional[2]
                );
                process::exit(1);
            }
            create_fifo(name, &mode);
        }
        "b" | "c" | "u" => {
            // Block or character special: need major and minor
            if positional.len() < 4 {
                eprintln!(
                    "{}: missing operand after '{}'",
                    TOOL_NAME,
                    positional.last().unwrap()
                );
                eprintln!("Special files require major and minor device numbers.");
                process::exit(1);
            }
            if positional.len() > 4 {
                eprintln!("{}: too many arguments after major/minor", TOOL_NAME);
                process::exit(1);
            }
            let major = parse_device_number(&positional[2], "major");
            let minor = parse_device_number(&positional[3], "minor");
            create_special(name, node_type, major, minor, &mode);
        }
        _ => {
            eprintln!("{}: invalid device type '{}'", TOOL_NAME, node_type);
            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
            process::exit(1);
        }
    }
}

#[cfg(unix)]
fn parse_device_number(s: &str, label: &str) -> u64 {
    // Support hex (0x), octal (0), and decimal
    let val = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
    } else if s.starts_with('0') && s.len() > 1 {
        u64::from_str_radix(&s[1..], 8)
    } else {
        s.parse::<u64>()
    };

    val.unwrap_or_else(|_| {
        eprintln!("{}: invalid {} device number '{}'", TOOL_NAME, label, s);
        process::exit(1);
    })
}

#[cfg(unix)]
fn parse_mode_str(mode_str: &str) -> libc::mode_t {
    libc::mode_t::from_str_radix(mode_str, 8).unwrap_or_else(|_| {
        eprintln!("{}: invalid mode: '{}'", TOOL_NAME, mode_str);
        process::exit(1);
    })
}

#[cfg(unix)]
fn create_fifo(name: &str, mode: &Option<String>) {
    let file_mode = match mode {
        Some(m) => parse_mode_str(m),
        None => 0o666,
    };

    let c_name = match CString::new(name) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("{}: invalid name '{}'", TOOL_NAME, name);
            process::exit(1);
        }
    };

    // SAFETY: c_name is a valid null-terminated C string, file_mode is a valid mode_t
    let ret = unsafe { libc::mkfifo(c_name.as_ptr(), file_mode) };
    if ret != 0 {
        let e = std::io::Error::last_os_error();
        eprintln!(
            "{}: {}: {}",
            TOOL_NAME,
            name,
            coreutils_rs::common::io_error_msg(&e)
        );
        process::exit(1);
    }
}

#[cfg(unix)]
fn create_special(name: &str, node_type: &str, major: u64, minor: u64, mode: &Option<String>) {
    let file_mode = match mode {
        Some(m) => parse_mode_str(m),
        None => 0o666,
    };

    let type_flag: libc::mode_t = match node_type {
        "b" => libc::S_IFBLK,
        "c" | "u" => libc::S_IFCHR,
        _ => unreachable!(),
    };

    // macOS makedev uses i32 and is safe, Linux uses c_uint
    #[cfg(target_vendor = "apple")]
    let dev = libc::makedev(major as i32, minor as i32);
    #[cfg(not(target_vendor = "apple"))]
    let dev = libc::makedev(major as libc::c_uint, minor as libc::c_uint);

    let c_name = match CString::new(name) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("{}: invalid name '{}'", TOOL_NAME, name);
            process::exit(1);
        }
    };

    // SAFETY: c_name is a valid null-terminated C string, file_mode | type_flag is a valid mode_t
    let ret = unsafe { libc::mknod(c_name.as_ptr(), file_mode | type_flag, dev) };
    if ret != 0 {
        let e = std::io::Error::last_os_error();
        eprintln!(
            "{}: {}: {}",
            TOOL_NAME,
            name,
            coreutils_rs::common::io_error_msg(&e)
        );
        process::exit(1);
    }
}

#[cfg(unix)]
fn print_help() {
    println!("Usage: {} [OPTION]... NAME TYPE [MAJOR MINOR]", TOOL_NAME);
    println!("Create the special file NAME of the given TYPE.");
    println!();
    println!("Mandatory arguments to long options are mandatory for short options too.");
    println!("  -m, --mode=MODE    set file permission bits to MODE, not a=rw - umask");
    println!("      --help         display this help and exit");
    println!("      --version      output version information and exit");
    println!();
    println!("Both MAJOR and MINOR must be specified when TYPE is b, c, or u, and they");
    println!("must be omitted when TYPE is p.  If MAJOR or MINOR begins with 0x or 0X,");
    println!("it is interpreted as hexadecimal; otherwise, if it begins with 0, as octal;");
    println!("otherwise, as decimal.  TYPE may be:");
    println!();
    println!("  b      create a block (buffered) special file");
    println!("  c, u   create a character (unbuffered) special file");
    println!("  p      create a FIFO");
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fmknod");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("mknod"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("mknod (fcoreutils)"));
    }

    #[test]
    fn test_create_fifo() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("testfifo");
        let output = cmd().args([fifo.to_str().unwrap(), "p"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));

        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            let meta = std::fs::symlink_metadata(&fifo).unwrap();
            assert!(meta.file_type().is_fifo(), "should be a FIFO");
        }
    }

    #[test]
    fn test_create_fifo_with_mode() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("modefifo");
        let output = cmd()
            .args(["-m", "0644", fifo.to_str().unwrap(), "p"])
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
    fn test_invalid_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("badtype");
        let output = cmd().args([path.to_str().unwrap(), "x"]).output().unwrap();
        assert_ne!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("invalid device type"),
            "should report invalid type: {}",
            stderr
        );
    }

    #[test]
    fn test_missing_major_minor_for_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blockdev");
        let output = cmd().args([path.to_str().unwrap(), "b"]).output().unwrap();
        assert_ne!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("missing operand") || stderr.contains("major and minor"),
            "should report missing major/minor: {}",
            stderr
        );
    }

    #[test]
    fn test_missing_major_minor_for_char() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chardev");
        let output = cmd().args([path.to_str().unwrap(), "c"]).output().unwrap();
        assert_ne!(output.status.code(), Some(0));
    }

    #[test]
    fn test_fifo_extra_args() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fifoextra");
        let output = cmd()
            .args([path.to_str().unwrap(), "p", "1", "2"])
            .output()
            .unwrap();
        assert_ne!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Superfluous"),
            "should report superfluous arg: {}",
            stderr
        );
    }

    #[test]
    fn test_missing_operand() {
        let output = cmd().output().unwrap();
        assert_ne!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("missing operand"),
            "should report missing operand: {}",
            stderr
        );
    }

    #[test]
    fn test_missing_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notype");
        let output = cmd().arg(path.to_str().unwrap()).output().unwrap();
        assert_ne!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("missing operand"));
    }

    #[test]
    fn test_fifo_existing_fails() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("existfifo");
        // Create it first
        cmd().args([fifo.to_str().unwrap(), "p"]).output().unwrap();
        // Try again
        let output = cmd().args([fifo.to_str().unwrap(), "p"]).output().unwrap();
        assert_ne!(output.status.code(), Some(0));
    }

    #[test]
    fn test_mode_flag_long() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("modelong");
        let output = cmd()
            .args(["--mode=0600", fifo.to_str().unwrap(), "p"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::symlink_metadata(&fifo).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "mode should be 0600, got {:o}", mode);
        }
    }

    #[test]
    fn test_matches_gnu_fifo() {
        let dir = tempfile::tempdir().unwrap();
        let gnu_path = dir.path().join("gnu_fifo");
        let our_path = dir.path().join("our_fifo");

        let gnu = Command::new("mknod")
            .args([gnu_path.to_str().unwrap(), "p"])
            .output();
        if let Ok(gnu_out) = gnu {
            let ours = cmd()
                .args([our_path.to_str().unwrap(), "p"])
                .output()
                .unwrap();
            assert_eq!(
                ours.status.code(),
                gnu_out.status.code(),
                "Exit code mismatch"
            );
        }
    }

    #[test]
    fn test_matches_gnu_invalid_type() {
        let dir = tempfile::tempdir().unwrap();
        let gnu_path = dir.path().join("gnu_bad");
        let our_path = dir.path().join("our_bad");

        let gnu = Command::new("mknod")
            .args([gnu_path.to_str().unwrap(), "z"])
            .output();
        if let Ok(gnu_out) = gnu {
            let ours = cmd()
                .args([our_path.to_str().unwrap(), "z"])
                .output()
                .unwrap();
            assert_eq!(
                ours.status.code(),
                gnu_out.status.code(),
                "Exit code mismatch on invalid type"
            );
        }
    }
}
