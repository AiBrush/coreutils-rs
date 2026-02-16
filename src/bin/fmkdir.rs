#[cfg(not(unix))]
fn main() {
    eprintln!("mkdir: only available on Unix");
    std::process::exit(1);
}

// fmkdir — make directories
//
// Usage: mkdir [OPTION]... DIRECTORY...

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "mkdir";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut parents = false;
    let mut verbose = false;
    let mut mode: Option<String> = None;
    let mut dirs: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        if saw_dashdash {
            dirs.push(arg);
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
            "-p" | "--parents" => parents = true,
            "-v" | "--verbose" => verbose = true,
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
            s if s.starts_with("--context") => {
                // SELinux context: accept and ignore
                // --context or --context=CTX
                if s == "--context" {
                    // consume next arg if present
                    let _ = args.next();
                }
            }
            "-Z" => {
                // SELinux context shorthand: accept and ignore
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                let chars: Vec<char> = s[1..].chars().collect();
                let mut i = 0;
                while i < chars.len() {
                    match chars[i] {
                        'p' => {}
                        'v' => verbose = true,
                        'Z' => {}
                        'm' => {
                            // Remaining chars after 'm' are the mode, or next arg
                            let rest: String = chars[i + 1..].iter().collect();
                            if rest.is_empty() {
                                if let Some(val) = args.next() {
                                    mode = Some(val);
                                } else {
                                    eprintln!("{}: option requires an argument -- 'm'", TOOL_NAME);
                                    process::exit(1);
                                }
                            } else {
                                mode = Some(rest);
                            }
                            // Set parents here since 'p' might have been processed
                            if chars.contains(&'p') {
                                parents = true;
                            }
                            break;
                        }
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, chars[i]);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    if chars[i] == 'p' {
                        parents = true;
                    }
                    i += 1;
                }
            }
            _ => dirs.push(arg),
        }
    }

    if dirs.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let parsed_mode = mode.as_ref().map(|m| parse_octal_mode(m));

    let mut exit_code = 0;
    for dir in &dirs {
        if let Err(code) = create_directory(dir, parents, verbose, parsed_mode) {
            exit_code = code;
        }
    }

    if exit_code != 0 {
        process::exit(exit_code);
    }
}

#[cfg(unix)]
fn create_directory(
    dir: &str,
    parents: bool,
    verbose: bool,
    mode: Option<libc::mode_t>,
) -> Result<(), i32> {
    if parents {
        create_with_parents(dir, verbose, mode)
    } else {
        create_single(dir, verbose, mode)
    }
}

#[cfg(unix)]
fn create_single(dir: &str, verbose: bool, mode: Option<libc::mode_t>) -> Result<(), i32> {
    match std::fs::create_dir(dir) {
        Ok(()) => {
            if verbose {
                eprintln!("{}: created directory '{}'", TOOL_NAME, dir);
            }
            if let Some(m) = mode {
                apply_mode(dir, m)?;
            }
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "{}: cannot create directory '{}': {}",
                TOOL_NAME,
                dir,
                coreutils_rs::common::io_error_msg(&e)
            );
            Err(1)
        }
    }
}

#[cfg(unix)]
fn create_with_parents(dir: &str, verbose: bool, mode: Option<libc::mode_t>) -> Result<(), i32> {
    let path = std::path::Path::new(dir);

    // Collect all ancestors that need to be created
    let mut to_create: Vec<&std::path::Path> = Vec::new();
    let mut current = path;
    while !current.exists() {
        to_create.push(current);
        match current.parent() {
            Some(p) if !p.as_os_str().is_empty() => current = p,
            _ => break,
        }
    }

    // Create from outermost to innermost
    to_create.reverse();

    for p in &to_create {
        let p_str = p.to_string_lossy();
        match std::fs::create_dir(p) {
            Ok(()) => {
                if verbose {
                    eprintln!("{}: created directory '{}'", TOOL_NAME, p_str);
                }
                // For parent directories (not the final target), set mode 0777 modified by umask
                // unless explicitly specified. For the final target, use specified mode.
                if *p == path
                    && let Some(m) = mode
                {
                    let _ = apply_mode(&p_str, m);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // With -p, this is not an error
            }
            Err(e) => {
                eprintln!(
                    "{}: cannot create directory '{}': {}",
                    TOOL_NAME,
                    p_str,
                    coreutils_rs::common::io_error_msg(&e)
                );
                return Err(1);
            }
        }
    }

    // If directory already existed and nothing needed creating, that's fine with -p
    Ok(())
}

#[cfg(unix)]
fn apply_mode(path: &str, mode: libc::mode_t) -> Result<(), i32> {
    let c_path = match CString::new(path) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("{}: invalid path '{}'", TOOL_NAME, path);
            return Err(1);
        }
    };
    // SAFETY: c_path is a valid null-terminated C string, mode is a valid mode_t
    let ret = unsafe { libc::chmod(c_path.as_ptr(), mode) };
    if ret != 0 {
        let e = std::io::Error::last_os_error();
        eprintln!(
            "{}: cannot set permissions on '{}': {}",
            TOOL_NAME,
            path,
            coreutils_rs::common::io_error_msg(&e)
        );
        return Err(1);
    }
    Ok(())
}

#[cfg(unix)]
fn parse_octal_mode(s: &str) -> libc::mode_t {
    libc::mode_t::from_str_radix(s, 8).unwrap_or_else(|_| {
        eprintln!("{}: invalid mode: '{}'", TOOL_NAME, s);
        process::exit(1);
    })
}

#[cfg(unix)]
fn print_help() {
    println!("Usage: {} [OPTION]... DIRECTORY...", TOOL_NAME);
    println!("Create the DIRECTORY(ies), if they do not already exist.");
    println!();
    println!("Mandatory arguments to long options are mandatory for short options too.");
    println!("  -m, --mode=MODE   set file mode (as in chmod), not a=rwx - umask");
    println!("  -p, --parents     no error if existing, make parent directories as needed");
    println!("  -v, --verbose     print a message for each created directory");
    println!("  -Z, --context=CTX set SELinux security context of each created directory");
    println!("                    to CTX");
    println!("      --help        display this help and exit");
    println!("      --version     output version information and exit");
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fmkdir");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("mkdir"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("mkdir (fcoreutils)"));
    }

    #[test]
    fn test_create_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("newdir");
        let output = cmd().arg(target.to_str().unwrap()).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(target.is_dir(), "directory should be created");
    }

    #[test]
    fn test_create_multiple_directories() {
        let dir = tempfile::tempdir().unwrap();
        let d1 = dir.path().join("dir1");
        let d2 = dir.path().join("dir2");
        let output = cmd()
            .args([d1.to_str().unwrap(), d2.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(d1.is_dir());
        assert!(d2.is_dir());
    }

    #[test]
    fn test_parents_creates_nested() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("c");
        let output = cmd()
            .args(["-p", nested.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(nested.is_dir(), "nested directories should be created");
    }

    #[test]
    fn test_parents_no_error_on_existing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("existing");
        std::fs::create_dir(&target).unwrap();
        let output = cmd()
            .args(["-p", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "-p should not error on existing"
        );
    }

    #[test]
    fn test_error_on_existing_without_parents() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("exists");
        std::fs::create_dir(&target).unwrap();
        let output = cmd().arg(target.to_str().unwrap()).output().unwrap();
        assert_ne!(
            output.status.code(),
            Some(0),
            "should error on existing without -p"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot create directory"),
            "should report error: {}",
            stderr
        );
    }

    #[test]
    fn test_mode_flag() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("modedir");
        let output = cmd()
            .args(["-m", "0755", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&target).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "mode should be 0755, got {:o}", mode);
        }
    }

    #[test]
    fn test_mode_flag_long() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("modelong");
        let output = cmd()
            .args(["--mode=0700", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&target).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "mode should be 0700, got {:o}", mode);
        }
    }

    #[test]
    fn test_verbose_flag() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("verbosedir");
        let output = cmd()
            .args(["-v", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("created directory"),
            "verbose should report creation: {}",
            stderr
        );
    }

    #[test]
    fn test_verbose_with_parents() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("x").join("y");
        let output = cmd()
            .args(["-pv", nested.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("created directory"),
            "verbose should report parent creation: {}",
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
    fn test_selinux_context_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("zdir");
        let output = cmd()
            .args(["-Z", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "-Z should be accepted (no-op)"
        );
        assert!(target.is_dir());
    }

    #[test]
    fn test_parents_mode_on_target() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("pm").join("child");
        let output = cmd()
            .args(["-p", "-m", "0750", nested.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&nested).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o750, "target mode should be 0750, got {:o}", mode);
        }
    }

    #[test]
    fn test_matches_gnu_exit_codes() {
        let gnu = Command::new("mkdir")
            .arg("/nonexistent_parent_12345/child")
            .output();
        if let Ok(gnu_out) = gnu {
            let ours = cmd()
                .arg("/nonexistent_parent_12345/child")
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
    fn test_matches_gnu_existing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gnu_existing");
        std::fs::create_dir(&target).unwrap();

        let gnu = Command::new("mkdir").arg(target.to_str().unwrap()).output();
        if let Ok(gnu_out) = gnu {
            // Re-create for our test (it already exists)
            let ours = cmd().arg(target.to_str().unwrap()).output().unwrap();
            assert_eq!(
                ours.status.code(),
                gnu_out.status.code(),
                "Exit code should match GNU on existing dir"
            );
        }
    }

    #[test]
    fn test_dashdash_separator() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("--weird-name");
        let output = cmd()
            .args(["--", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(target.is_dir(), "should create dir with -- separator");
    }
}
