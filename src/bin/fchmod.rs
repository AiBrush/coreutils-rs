#[cfg(not(unix))]
fn main() {
    eprintln!("chmod: only available on Unix");
    std::process::exit(1);
}

// fchmod -- change file mode bits
//
// Usage: chmod [OPTION]... MODE[,MODE]... FILE...
//   or:  chmod [OPTION]... OCTAL-MODE FILE...
//   or:  chmod [OPTION]... --reference=RFILE FILE...

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "chmod";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut config = coreutils_rs::chmod::ChmodConfig::default();
    let mut reference: Option<String> = None;
    let mut mode_str: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if saw_dashdash {
            files.push(arg.clone());
            i += 1;
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
            "-c" | "--changes" => config.changes = true,
            "-f" | "--silent" | "--quiet" => config.quiet = true,
            "-v" | "--verbose" => config.verbose = true,
            "--no-preserve-root" => config.preserve_root = false,
            "--preserve-root" => config.preserve_root = true,
            "-R" | "--recursive" => config.recursive = true,
            s if s.starts_with("--reference=") => {
                reference = Some(s["--reference=".len()..].to_string());
            }
            "--reference" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option '--reference' requires an argument", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                reference = Some(args[i].clone());
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                // Could be combined short flags like -Rvc, OR a symbolic mode like -rwx
                // Try to parse as flags first
                let chars: Vec<char> = s[1..].chars().collect();
                let all_flags = chars.iter().all(|c| matches!(c, 'c' | 'f' | 'v' | 'R'));
                if all_flags {
                    for ch in &chars {
                        match ch {
                            'c' => config.changes = true,
                            'f' => config.quiet = true,
                            'v' => config.verbose = true,
                            'R' => config.recursive = true,
                            _ => unreachable!(),
                        }
                    }
                } else {
                    // Treat as mode string (e.g. "-rwx" means remove rwx)
                    if mode_str.is_none() {
                        mode_str = Some(arg.clone());
                    } else {
                        files.push(arg.clone());
                    }
                }
            }
            _ => {
                // First non-option argument is the mode (unless --reference is used)
                if mode_str.is_none() && reference.is_none() {
                    mode_str = Some(arg.clone());
                } else {
                    files.push(arg.clone());
                }
            }
        }
        i += 1;
    }

    // If --reference is used, we don't need a mode string
    if reference.is_none() && mode_str.is_none() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    if files.is_empty() {
        if reference.is_some() {
            eprintln!("{}: missing operand", TOOL_NAME);
        } else {
            eprintln!(
                "{}: missing operand after '{}'",
                TOOL_NAME,
                mode_str.as_deref().unwrap_or("")
            );
        }
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    // Get mode from reference file if specified
    let effective_mode_str: String = if let Some(ref rfile) = reference {
        match std::fs::metadata(rfile) {
            Ok(meta) => {
                let m = meta.mode() & 0o7777;
                format!("{:o}", m)
            }
            Err(e) => {
                eprintln!(
                    "{}: failed to get attributes of '{}': {}",
                    TOOL_NAME,
                    rfile,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        }
    } else {
        mode_str.unwrap()
    };

    let mut exit_code = 0;

    for file in &files {
        let path = std::path::Path::new(file);

        if config.recursive {
            if config.preserve_root && path == std::path::Path::new("/") {
                eprintln!(
                    "{}: it is dangerous to operate recursively on '/'",
                    TOOL_NAME
                );
                eprintln!(
                    "{}: use --no-preserve-root to override this failsafe",
                    TOOL_NAME
                );
                exit_code = 1;
                continue;
            }

            if let Err(e) = coreutils_rs::chmod::chmod_recursive(path, &effective_mode_str, &config)
            {
                if !config.quiet {
                    eprintln!("{}: {}", TOOL_NAME, e);
                }
                exit_code = 1;
            }
        } else {
            // Get current mode
            let metadata = match std::fs::symlink_metadata(path) {
                Ok(m) => m,
                Err(e) => {
                    if !config.quiet {
                        eprintln!(
                            "{}: cannot access '{}': {}",
                            TOOL_NAME,
                            file,
                            coreutils_rs::common::io_error_msg(&e)
                        );
                    }
                    exit_code = 1;
                    continue;
                }
            };

            // Skip symlinks
            if metadata.file_type().is_symlink() {
                continue;
            }

            let current_mode = metadata.mode();
            let new_mode = match coreutils_rs::chmod::parse_mode(&effective_mode_str, current_mode)
            {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("{}: {}", TOOL_NAME, e);
                    process::exit(1);
                }
            };

            if let Err(e) = coreutils_rs::chmod::chmod_file(path, new_mode, &config) {
                if !config.quiet {
                    eprintln!(
                        "{}: changing permissions of '{}': {}",
                        TOOL_NAME,
                        file,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                }
                exit_code = 1;
            }
        }
    }

    if exit_code != 0 {
        process::exit(exit_code);
    }
}

#[cfg(unix)]
fn print_help() {
    println!("Usage: {} [OPTION]... MODE[,MODE]... FILE...", TOOL_NAME);
    println!("  or:  {} [OPTION]... OCTAL-MODE FILE...", TOOL_NAME);
    println!(
        "  or:  {} [OPTION]... --reference=RFILE FILE...",
        TOOL_NAME
    );
    println!();
    println!("Change the mode of each FILE to MODE.");
    println!("With --reference, change the mode of each FILE to that of RFILE.");
    println!();
    println!("  -c, --changes          like verbose but report only when a change is made");
    println!("  -f, --silent, --quiet   suppress most error messages");
    println!(
        "  -v, --verbose          output a diagnostic for every file processed"
    );
    println!("      --no-preserve-root  do not treat '/' specially (the default)");
    println!("      --preserve-root    fail to operate recursively on '/'");
    println!(
        "      --reference=RFILE  use RFILE's mode instead of MODE values"
    );
    println!("  -R, --recursive        change files and directories recursively");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
    println!();
    println!("Each MODE is of the form '[ugoa]*([-+=]([rwxXst]*|[ugo]))+|[-+=][0-7]+'.");
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fchmod");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("chmod"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("chmod"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_missing_operand() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("missing operand"));
    }

    #[test]
    fn test_missing_file() {
        let output = cmd().arg("755").output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("missing operand"));
    }

    #[test]
    fn test_octal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "test").unwrap();

        let output = cmd()
            .args(["755", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success(), "chmod 755 should succeed");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&file).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "mode should be 0755, got {:o}", mode);
        }
    }

    #[test]
    fn test_symbolic_mode() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sym.txt");
        std::fs::write(&file, "test").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        let output = cmd()
            .args(["u+x", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&file).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o744, "mode should be 0744, got {:o}", mode);
        }
    }

    #[test]
    fn test_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let file = sub.join("file.txt");
        std::fs::write(&file, "test").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        let output = cmd()
            .args(["-R", "755", dir.path().to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&file).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "mode should be 0755, got {:o}", mode);
        }
    }

    #[test]
    fn test_verbose_flag() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("verbose.txt");
        std::fs::write(&file, "test").unwrap();

        let output = cmd()
            .args(["-v", "755", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("mode of"),
            "verbose should report mode change: {}",
            stderr
        );
    }

    #[test]
    fn test_reference_file() {
        let dir = tempfile::tempdir().unwrap();
        let ref_file = dir.path().join("ref.txt");
        let target = dir.path().join("target.txt");
        std::fs::write(&ref_file, "ref").unwrap();
        std::fs::write(&target, "target").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ref_file, std::fs::Permissions::from_mode(0o751)).unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        let output = cmd()
            .args([
                &format!("--reference={}", ref_file.to_str().unwrap()),
                target.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&target).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(mode, 0o751, "mode should match reference, got {:o}", mode);
        }
    }

    #[test]
    fn test_nonexistent_file() {
        let output = cmd()
            .args(["755", "/nonexistent_file_12345"])
            .output()
            .unwrap();
        assert_ne!(output.status.code(), Some(0));
    }

    #[test]
    fn test_quiet_suppresses_errors() {
        let output = cmd()
            .args(["-f", "755", "/nonexistent_file_12345"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.is_empty(),
            "quiet mode should suppress errors: {}",
            stderr
        );
    }

    #[test]
    fn test_preserve_root() {
        let output = cmd()
            .args(["-R", "--preserve-root", "755", "/"])
            .output()
            .unwrap();
        assert_ne!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("dangerous"),
            "should warn about root: {}",
            stderr
        );
    }
}
