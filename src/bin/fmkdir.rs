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

    let parsed_mode = mode.as_ref().map(|m| parse_mode(m));

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
                println!("{}: created directory '{}'", TOOL_NAME, dir);
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
                    println!("{}: created directory '{}'", TOOL_NAME, p_str);
                }
                if *p == path {
                    // Final target directory: apply specified mode
                    if let Some(m) = mode {
                        let _ = apply_mode(&p_str, m);
                    }
                } else if mode.is_some() {
                    // Intermediate directory: ensure u+wx for traversal
                    // GNU mkdir does this so parent dirs are usable even with restrictive -m
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = std::fs::metadata(&**p) {
                        let current = meta.permissions().mode() & 0o7777;
                        let needed = current | 0o300; // u+wx
                        if needed != current {
                            let _ = apply_mode(&p_str, needed as libc::mode_t);
                        }
                    }
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
fn parse_mode(s: &str) -> libc::mode_t {
    // Try octal first: non-empty, all chars are octal digits
    if !s.is_empty() && s.bytes().all(|c| matches!(c, b'0'..=b'7')) {
        return libc::mode_t::from_str_radix(s, 8).unwrap_or_else(|_| {
            eprintln!("{}: invalid mode: \u{2018}{}\u{2019}", TOOL_NAME, s);
            process::exit(1);
        });
    }

    // Parse symbolic mode, starting from base 0o777 (default for mkdir)
    parse_symbolic_mode(s, 0o777)
}

#[cfg(unix)]
fn parse_symbolic_mode(s: &str, base: libc::mode_t) -> libc::mode_t {
    let mut mode = base;

    // Get current umask
    let umask_val = unsafe {
        let m = libc::umask(0);
        libc::umask(m);
        m
    };

    for clause in s.split(',') {
        if clause.is_empty() {
            eprintln!("{}: invalid mode: \u{2018}{}\u{2019}", TOOL_NAME, s);
            process::exit(1);
        }

        let bytes = clause.as_bytes();
        let mut pos = 0;

        // Parse who: [ugoa]*
        let mut who_u = false;
        let mut who_g = false;
        let mut who_o = false;
        let mut explicit_who = false;

        while pos < bytes.len() {
            match bytes[pos] {
                b'u' => {
                    who_u = true;
                    explicit_who = true;
                }
                b'g' => {
                    who_g = true;
                    explicit_who = true;
                }
                b'o' => {
                    who_o = true;
                    explicit_who = true;
                }
                b'a' => {
                    who_u = true;
                    who_g = true;
                    who_o = true;
                    explicit_who = true;
                }
                _ => break,
            }
            pos += 1;
        }

        // If no explicit who, default to all
        if !explicit_who {
            who_u = true;
            who_g = true;
            who_o = true;
        }

        // Must have at least one operation
        if pos >= bytes.len() || !matches!(bytes[pos], b'+' | b'-' | b'=') {
            eprintln!("{}: invalid mode: \u{2018}{}\u{2019}", TOOL_NAME, s);
            process::exit(1);
        }

        // Parse one or more operations: [+-=][rwxXst]*
        while pos < bytes.len() && matches!(bytes[pos], b'+' | b'-' | b'=') {
            let op = bytes[pos];
            pos += 1;

            // Parse permission chars
            let mut perm_rwx: libc::mode_t = 0;
            let mut has_s = false;
            let mut has_t = false;

            while pos < bytes.len() {
                match bytes[pos] {
                    b'r' => perm_rwx |= 4,
                    b'w' => perm_rwx |= 2,
                    b'x' => perm_rwx |= 1,
                    b'X' => {
                        // For mkdir (always directory), X = x
                        perm_rwx |= 1;
                    }
                    b's' => has_s = true,
                    b't' => has_t = true,
                    b'u' => {
                        // Copy user bits
                        perm_rwx |= ((mode >> 6) & 7) as libc::mode_t;
                        pos += 1;
                        break;
                    }
                    b'g' => {
                        // Copy group bits
                        perm_rwx |= ((mode >> 3) & 7) as libc::mode_t;
                        pos += 1;
                        break;
                    }
                    b'o' => {
                        // Copy other bits
                        perm_rwx |= (mode & 7) as libc::mode_t;
                        pos += 1;
                        break;
                    }
                    _ => break,
                }
                pos += 1;
            }

            // Build the full bits to apply
            let mut bits: libc::mode_t = 0;
            if who_u {
                bits |= perm_rwx << 6;
            }
            if who_g {
                bits |= perm_rwx << 3;
            }
            if who_o {
                bits |= perm_rwx;
            }
            if has_s {
                if who_u {
                    bits |= 0o4000;
                }
                if who_g {
                    bits |= 0o2000;
                }
            }
            if has_t {
                bits |= 0o1000;
            }

            match op {
                b'+' => {
                    if !explicit_who {
                        bits &= !umask_val;
                    }
                    mode |= bits;
                }
                b'-' => {
                    if !explicit_who {
                        bits &= !umask_val;
                    }
                    mode &= !bits;
                }
                b'=' => {
                    // Clear bits for specified who classes
                    let mut clear_mask: libc::mode_t = 0;
                    if who_u {
                        clear_mask |= 0o4700;
                    }
                    if who_g {
                        clear_mask |= 0o2070;
                    }
                    if who_o {
                        clear_mask |= 0o1007;
                    }
                    if !explicit_who {
                        bits &= !umask_val;
                    }
                    mode = (mode & !clear_mask) | bits;
                }
                _ => unreachable!(),
            }
        }
    }

    mode
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
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("created directory"),
            "verbose should report creation: {}",
            stdout
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
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("created directory"),
            "verbose should report parent creation: {}",
            stdout
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

    #[test]
    fn test_symbolic_mode_a_equals_rx() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sym_arx");
        let output = cmd()
            .args(["-m", "a=rx", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&target).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o555, "a=rx should give 0555, got {:o}", mode);
        }
    }

    #[test]
    fn test_symbolic_mode_u_equals_rwx_go_none() {
        // u=rwx,go= means: user rwx, clear group and other
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sym_urwx");
        let output = cmd()
            .args(["-m", "u=rwx,go=", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&target).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "u=rwx,go= should give 0700, got {:o}", mode);
        }
    }

    #[test]
    fn test_symbolic_mode_comma_separated() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sym_comma");
        let output = cmd()
            .args(["-m", "u=rwx,g=rx,o=rx", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&target).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o755,
                "u=rwx,g=rx,o=rx should give 0755, got {:o}",
                mode
            );
        }
    }

    #[test]
    fn test_symbolic_mode_go_minus_w() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sym_gow");
        let output = cmd()
            .args(["-m", "go-w", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&target).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "go-w should give 0755, got {:o}", mode);
        }
    }

    #[test]
    fn test_symbolic_mode_matches_gnu() {
        // Test that symbolic modes produce the same result as GNU mkdir
        let dir = tempfile::tempdir().unwrap();

        let test_cases = [
            ("a=rwx", 0o777),
            ("a=rx", 0o555),
            ("u=rwx,go=rx", 0o755),
            ("755", 0o755),
            ("700", 0o700),
            ("a=r", 0o444),
        ];

        for (mode_str, expected) in &test_cases {
            let target = dir.path().join(format!("gnu_sym_{}", mode_str.replace(',', "_")));
            let output = cmd()
                .args(["-m", mode_str, target.to_str().unwrap()])
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(0),
                "failed for mode '{}'",
                mode_str
            );

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let meta = std::fs::metadata(&target).unwrap();
                let mode = meta.permissions().mode() & 0o777;
                assert_eq!(
                    mode, *expected,
                    "mode '{}' should give {:o}, got {:o}",
                    mode_str, expected, mode
                );
            }
        }
    }
}
