#[cfg(not(unix))]
fn main() {
    eprintln!("mktemp: only available on Unix");
    std::process::exit(1);
}

// fmktemp — create a temporary file or directory
//
// Usage: mktemp [OPTION]... [TEMPLATE]

#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "mktemp";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut make_dir = false;
    let mut dry_run = false;
    let mut quiet = false;
    let mut use_tmpdir: Option<Option<String>> = None; // None=not set, Some(None)=set w/o value, Some(Some(d))=set with value
    let mut suffix: Option<String> = None;
    let mut use_t_flag = false;
    let mut template: Option<String> = None;
    let mut saw_dashdash = false;

    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        if saw_dashdash {
            if template.is_some() {
                eprintln!("{}: too many templates", TOOL_NAME);
                eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                process::exit(1);
            }
            template = Some(arg);
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
            "-d" | "--directory" => make_dir = true,
            "-u" | "--dry-run" => dry_run = true,
            "-q" | "--quiet" => quiet = true,
            "-t" => use_t_flag = true,
            s if s.starts_with("--tmpdir=") => {
                let val = &s["--tmpdir=".len()..];
                use_tmpdir = Some(Some(val.to_string()));
            }
            "--tmpdir" => {
                // --tmpdir without = means use TMPDIR or /tmp
                use_tmpdir = Some(None);
            }
            s if s.starts_with("--suffix=") => {
                let val = &s["--suffix=".len()..];
                suffix = Some(val.to_string());
            }
            "--suffix" => {
                if let Some(val) = args.next() {
                    suffix = Some(val);
                } else {
                    eprintln!("{}: option '--suffix' requires an argument", TOOL_NAME);
                    process::exit(1);
                }
            }
            s if s.starts_with("-p") => {
                let rest = &s[2..];
                if rest.is_empty() {
                    if let Some(val) = args.next() {
                        use_tmpdir = Some(Some(val));
                    } else {
                        eprintln!("{}: option requires an argument -- 'p'", TOOL_NAME);
                        process::exit(1);
                    }
                } else {
                    use_tmpdir = Some(Some(rest.to_string()));
                }
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                // Parse combined short flags like -duq
                for ch in s[1..].chars() {
                    match ch {
                        'd' => make_dir = true,
                        'u' => dry_run = true,
                        'q' => quiet = true,
                        't' => use_t_flag = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            _ => {
                if template.is_some() {
                    eprintln!("{}: too many templates", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                template = Some(arg);
            }
        }
    }

    // Determine the effective template and directory
    let default_template = "tmp.XXXXXXXXXX";
    let tmpl = template.unwrap_or_else(|| default_template.to_string());

    // Determine base directory
    let tmpdir_env = std::env::var("TMPDIR").ok();
    let base_dir = if use_t_flag {
        // -t: interpret template as a filename component; prepend TMPDIR (or -p dir, or /tmp)
        if let Some(ref dir_opt) = use_tmpdir {
            match dir_opt {
                Some(d) => d.clone(),
                None => tmpdir_env.unwrap_or_else(|| "/tmp".to_string()),
            }
        } else {
            tmpdir_env.unwrap_or_else(|| "/tmp".to_string())
        }
    } else if let Some(ref dir_opt) = use_tmpdir {
        match dir_opt {
            Some(d) => d.clone(),
            None => tmpdir_env.unwrap_or_else(|| "/tmp".to_string()),
        }
    } else if !tmpl.contains('/') {
        // No directory separator in template and no -p/--tmpdir: use TMPDIR or /tmp
        tmpdir_env.unwrap_or_else(|| "/tmp".to_string())
    } else {
        // Template contains a path; use it as-is (base_dir not needed)
        String::new()
    };

    // Build the full template path
    let full_template = if !base_dir.is_empty() {
        format!("{}/{}", base_dir, tmpl)
    } else {
        tmpl.clone()
    };

    // Append suffix if given
    let full_template = if let Some(ref suf) = suffix {
        format!("{}{}", full_template, suf)
    } else {
        full_template
    };

    // Validate template: must have at least 3 consecutive X's before suffix
    let (prefix, x_count, suf_part) = parse_template(&full_template, &suffix);
    if x_count < 3 {
        eprintln!("{}: too few X's in template '{}'", TOOL_NAME, full_template);
        process::exit(1);
    }

    match create_temp(&prefix, x_count, &suf_part, make_dir, dry_run, quiet) {
        Ok(path) => {
            println!("{}", path);
        }
        Err(msg) => {
            if !quiet {
                eprintln!("{}: {}", TOOL_NAME, msg);
            }
            process::exit(1);
        }
    }
}

/// Parse a template into (prefix, x_count, suffix).
/// The X's are the trailing X's before the suffix.
#[cfg(unix)]
fn parse_template(template: &str, suffix: &Option<String>) -> (String, usize, String) {
    let suf_len = suffix.as_ref().map_or(0, |s| s.len());
    let base = &template[..template.len() - suf_len];
    let suf_part = &template[template.len() - suf_len..];

    // Count trailing X's in base
    let x_count = base.chars().rev().take_while(|&c| c == 'X').count();
    let prefix = &base[..base.len() - x_count];

    (prefix.to_string(), x_count, suf_part.to_string())
}

#[cfg(unix)]
fn generate_random_name(x_count: usize) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

    let mut buf = vec![0u8; x_count];

    // Use getrandom via /dev/urandom for randomness
    let fd = unsafe { libc::open(c"/dev/urandom".as_ptr(), libc::O_RDONLY) };
    if fd >= 0 {
        unsafe {
            libc::read(fd, buf.as_mut_ptr().cast::<libc::c_void>(), x_count);
            libc::close(fd);
        }
    } else {
        // Fallback: use time + pid based seed
        let seed = unsafe {
            let mut tv: libc::timeval = std::mem::zeroed();
            libc::gettimeofday(&mut tv, std::ptr::null_mut());
            (tv.tv_sec as u64)
                .wrapping_mul(1_000_000)
                .wrapping_add(tv.tv_usec as u64)
                .wrapping_add(libc::getpid() as u64)
        };
        let mut state = seed;
        for byte in &mut buf {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *byte = (state >> 33) as u8;
        }
    }

    buf.iter()
        .map(|&b| CHARSET[(b as usize) % CHARSET.len()] as char)
        .collect()
}

#[cfg(unix)]
fn create_temp(
    prefix: &str,
    x_count: usize,
    suffix: &str,
    make_dir: bool,
    dry_run: bool,
    quiet: bool,
) -> Result<String, String> {
    // Try up to 100 times to avoid collisions
    for _ in 0..100 {
        let random_part = generate_random_name(x_count);
        let path = format!("{}{}{}", prefix, random_part, suffix);

        if dry_run {
            return Ok(path);
        }

        if make_dir {
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    // Set permissions to 0700 for directories (private)
                    #[cfg(unix)]
                    {
                        let c_path =
                            std::ffi::CString::new(path.as_str()).map_err(|e| e.to_string())?;
                        unsafe {
                            libc::chmod(c_path.as_ptr(), 0o700);
                        }
                    }
                    return Ok(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    let msg = format!(
                        "failed to create directory via template '{}{}{}': {}",
                        prefix,
                        "X".repeat(x_count),
                        suffix,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                    if !quiet {
                        return Err(msg);
                    }
                    return Err(msg);
                }
            }
        } else {
            // Create file exclusively
            use std::fs::OpenOptions;
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_file) => {
                    // Set permissions to 0600 for files (private)
                    #[cfg(unix)]
                    {
                        let c_path =
                            std::ffi::CString::new(path.as_str()).map_err(|e| e.to_string())?;
                        unsafe {
                            libc::chmod(c_path.as_ptr(), 0o600);
                        }
                    }
                    return Ok(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    let msg = format!(
                        "failed to create file via template '{}{}{}': {}",
                        prefix,
                        "X".repeat(x_count),
                        suffix,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                    return Err(msg);
                }
            }
        }
    }

    Err(format!(
        "failed to create {} via template '{}{}{}': too many retries",
        if make_dir { "directory" } else { "file" },
        prefix,
        "X".repeat(x_count),
        suffix
    ))
}

#[cfg(unix)]
fn print_help() {
    println!("Usage: {} [OPTION]... [TEMPLATE]", TOOL_NAME);
    println!("Create a temporary file or directory, safely, and print its name.");
    println!("TEMPLATE must contain at least 3 consecutive 'X's in last component.");
    println!("If TEMPLATE is not specified, use tmp.XXXXXXXXXX, and --tmpdir is implied.");
    println!();
    println!("  -d, --directory     create a directory, not a file");
    println!("  -u, --dry-run       do not create anything; merely print a name (unsafe)");
    println!("  -q, --quiet         suppress diagnostics about file/dir-creation failure");
    println!("  -p DIR, --tmpdir[=DIR]  interpret TEMPLATE relative to DIR; if DIR is not");
    println!("                      specified, use $TMPDIR if set, else /tmp.  With this option,");
    println!("                      TEMPLATE must not be an absolute pathname; unlike with -t,");
    println!("                      TEMPLATE may contain slashes, but mktemp creates only the");
    println!("                      final component");
    println!("  -t                  interpret TEMPLATE as a single file name component,");
    println!("                      relative to a directory: $TMPDIR, if set; else the");
    println!("                      directory specified via -p; else /tmp [deprecated]");
    println!("      --suffix=SUFF   append SUFF to TEMPLATE; SUFF must not contain a slash.");
    println!("                      This option is implied if TEMPLATE does not end in X");
    println!("      --help          display this help and exit");
    println!("      --version       output version information and exit");
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fmktemp");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("mktemp"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("mktemp (fcoreutils)"));
    }

    #[test]
    fn test_default_template() {
        let dir = tempfile::tempdir().unwrap();
        let output = cmd()
            .args(["-p", dir.path().to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = stdout.trim();
        assert!(
            path.starts_with(dir.path().to_str().unwrap()),
            "path should start with tmpdir: {}",
            path
        );
        assert!(
            std::path::Path::new(path).exists(),
            "created file should exist"
        );
        // Default template is tmp.XXXXXXXXXX so filename should start with tmp.
        let filename = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            filename.starts_with("tmp."),
            "default filename should start with 'tmp.': {}",
            filename
        );
        assert_eq!(
            filename.len(),
            "tmp.".len() + 10,
            "default template should produce 14-char filename"
        );
    }

    #[test]
    fn test_custom_template() {
        let dir = tempfile::tempdir().unwrap();
        let template = format!("{}/myapp.XXXXX", dir.path().display());
        let output = cmd().arg(&template).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = stdout.trim();
        let filename = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            filename.starts_with("myapp."),
            "should start with myapp.: {}",
            filename
        );
        assert!(std::path::Path::new(path).exists());
    }

    #[test]
    fn test_directory_flag() {
        let dir = tempfile::tempdir().unwrap();
        let output = cmd()
            .args(["-d", "-p", dir.path().to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = stdout.trim();
        let meta = std::fs::metadata(path).unwrap();
        assert!(meta.is_dir(), "should create a directory");
    }

    #[test]
    fn test_tmpdir_flag() {
        let dir = tempfile::tempdir().unwrap();
        let output = cmd()
            .args(["-p", dir.path().to_str().unwrap(), "testXXXXXX"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = stdout.trim();
        assert!(
            path.starts_with(dir.path().to_str().unwrap()),
            "path should be under specified dir: {}",
            path
        );
    }

    #[test]
    fn test_suffix_flag() {
        let dir = tempfile::tempdir().unwrap();
        let output = cmd()
            .args([
                "-p",
                dir.path().to_str().unwrap(),
                "--suffix=.txt",
                "testXXXXXX",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = stdout.trim();
        assert!(
            path.ends_with(".txt"),
            "path should end with .txt: {}",
            path
        );
        assert!(std::path::Path::new(path).exists());
    }

    #[test]
    fn test_t_flag() {
        let dir = tempfile::tempdir().unwrap();
        let output = cmd()
            .args(["-t", "-p", dir.path().to_str().unwrap(), "myXXXXXX"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = stdout.trim();
        assert!(
            path.starts_with(dir.path().to_str().unwrap()),
            "-t should use tmpdir: {}",
            path
        );
    }

    #[test]
    fn test_missing_xs_error() {
        let dir = tempfile::tempdir().unwrap();
        let template = format!("{}/notemplate", dir.path().display());
        let output = cmd().arg(&template).output().unwrap();
        assert_ne!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("too few X's"),
            "should report too few X's: {}",
            stderr
        );
    }

    #[test]
    fn test_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let template = format!("{}/dryXXXXXX", dir.path().display());
        let output = cmd().args(["-u", &template]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = stdout.trim();
        // dry-run should NOT create the file
        assert!(
            !std::path::Path::new(path).exists(),
            "dry-run should not create file"
        );
    }

    #[test]
    fn test_quiet_suppresses_error() {
        let output = cmd()
            .args(["-q", "/nonexistent_dir_12345/tmpXXXXXX"])
            .output()
            .unwrap();
        assert_ne!(output.status.code(), Some(0));
        // In quiet mode, stderr should be empty
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.is_empty(),
            "quiet mode should suppress errors: {}",
            stderr
        );
    }

    #[test]
    fn test_unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let template = format!("{}/uniqXXXXXX", dir.path().display());
        let out1 = cmd().arg(&template).output().unwrap();
        let out2 = cmd().arg(&template).output().unwrap();
        let path1 = String::from_utf8_lossy(&out1.stdout).trim().to_string();
        let path2 = String::from_utf8_lossy(&out2.stdout).trim().to_string();
        assert_ne!(
            path1, path2,
            "two invocations should produce different names"
        );
    }

    #[test]
    fn test_matches_gnu_exit_codes() {
        // Compare exit codes with GNU mktemp for error case
        let gnu = Command::new("mktemp").arg("/nonexistent/badXX").output();
        if let Ok(gnu_out) = gnu {
            let ours = cmd().arg("/nonexistent/badXX").output().unwrap();
            assert_eq!(
                ours.status.code(),
                gnu_out.status.code(),
                "Exit code mismatch with GNU mktemp"
            );
        }
    }

    #[test]
    fn test_file_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let output = cmd()
            .args(["-p", dir.path().to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = stdout.trim();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "file should have mode 0600, got {:o}", mode);
        }
    }

    #[test]
    fn test_directory_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let output = cmd()
            .args(["-d", "-p", dir.path().to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let path = stdout.trim();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o700,
                "directory should have mode 0700, got {:o}",
                mode
            );
        }
    }
}
