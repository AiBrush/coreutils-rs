// fpathchk — check whether file names are valid or portable
//
// Usage: pathchk [OPTION]... NAME...

use std::process;

const TOOL_NAME: &str = "pathchk";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// POSIX minimum limits
const POSIX_NAME_MAX: usize = 14;
const POSIX_PATH_MAX: usize = 256;

/// POSIX portable filename character set
const POSIX_PORTABLE_CHARS: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._-";

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut posix_check = false; // -p
    let mut extra_check = false; // -P
    let mut names: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    for arg in &args {
        if saw_dashdash {
            names.push(arg.clone());
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
            "--portability" => {
                posix_check = true;
                extra_check = true;
            }
            "--" => saw_dashdash = true,
            s if s.starts_with('-') && !s.starts_with("--") && s.len() > 1 => {
                for ch in s[1..].chars() {
                    match ch {
                        'p' => posix_check = true,
                        'P' => extra_check = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            _ => names.push(arg.clone()),
        }
    }

    if names.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let mut exit_code = 0;

    for name in &names {
        if let Err(msg) = check_path(name, posix_check, extra_check) {
            eprintln!("{}: {}", TOOL_NAME, msg);
            exit_code = 1;
        }
    }

    process::exit(exit_code);
}

fn check_path(path: &str, posix_check: bool, extra_check: bool) -> Result<(), String> {
    // -P checks: empty name and leading hyphen
    if extra_check {
        if path.is_empty() {
            return Err("empty file name".to_string());
        }
        // Check each component for leading hyphen and empty components
        let components: Vec<&str> = if path.contains('/') {
            path.split('/').collect()
        } else {
            vec![path]
        };
        for component in &components {
            if component.is_empty() && path != "/" && !path.starts_with('/') {
                return Err(format!("empty file name component in '{}'", path));
            }
            if component.starts_with('-') {
                return Err(format!(
                    "leading '-' in a component of file name '{}'",
                    path
                ));
            }
        }
    }

    if posix_check {
        // -p: POSIX portability checks
        // Check path length
        if path.len() > POSIX_PATH_MAX {
            return Err(format!(
                "limit {} exceeded by length {} of file name '{}'",
                POSIX_PATH_MAX,
                path.len(),
                path
            ));
        }

        // Check each component
        let components: Vec<&str> = path.split('/').collect();
        for component in &components {
            if component.is_empty() {
                continue; // Skip empty components from leading/trailing/double slashes
            }
            // Check for non-portable characters first (GNU order)
            for ch in component.chars() {
                if !POSIX_PORTABLE_CHARS.contains(ch) {
                    return Err(format!(
                        "nonportable character '{}' in file name '{}'",
                        ch, path
                    ));
                }
            }
            // Check component length
            if component.len() > POSIX_NAME_MAX {
                return Err(format!(
                    "limit {} exceeded by length {} of file name component '{}'",
                    POSIX_NAME_MAX,
                    component.len(),
                    component
                ));
            }
        }
    } else {
        // Without -p: check against system limits
        check_system_limits(path)?;
    }

    Ok(())
}

fn check_system_limits(path: &str) -> Result<(), String> {
    // Get system limits (pathconf is Unix-only, use defaults elsewhere)
    #[cfg(unix)]
    let (path_max, name_max) = unsafe {
        (
            libc::pathconf(c"/".as_ptr(), libc::_PC_PATH_MAX),
            libc::pathconf(c"/".as_ptr(), libc::_PC_NAME_MAX),
        )
    };
    #[cfg(not(unix))]
    let (path_max, name_max): (i64, i64) = (4096, 255);

    if path_max > 0 && path.len() > path_max as usize {
        return Err(format!(
            "limit {} exceeded by length {} of file name '{}'",
            path_max,
            path.len(),
            path
        ));
    }

    let components: Vec<&str> = path.split('/').collect();
    for component in &components {
        if component.is_empty() {
            continue;
        }
        if name_max > 0 && component.len() > name_max as usize {
            return Err(format!(
                "limit {} exceeded by length {} of file name component '{}'",
                name_max,
                component.len(),
                component
            ));
        }
    }

    // Check if the path (up to existing ancestors) is accessible
    // We just check that the longest existing prefix is a directory
    let p = std::path::Path::new(path);
    let mut check = p.to_path_buf();
    while !check.as_os_str().is_empty() && check != std::path::Path::new("/") {
        if check.exists() {
            if !check.is_dir() && check != p {
                return Err(format!("'{}' is not a directory", check.display()));
            }
            break;
        }
        if !check.pop() {
            break;
        }
    }

    Ok(())
}

fn print_help() {
    println!("Usage: {} [OPTION]... NAME...", TOOL_NAME);
    println!("Diagnose invalid or unportable file names.");
    println!();
    println!("  -p                  check for most POSIX systems");
    println!("  -P                  check for empty names and leading \"-\"");
    println!("      --portability   check for all POSIX systems (equivalent to -p -P)");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fpathchk");
        Command::new(path)
    }

    #[test]
    fn test_pathchk_valid_path() {
        let output = cmd().arg("/tmp/valid_file.txt").output().unwrap();
        assert!(output.status.success());
    }

    #[test]
    fn test_pathchk_portable_invalid_chars() {
        // -p should reject non-POSIX characters like spaces
        let output = cmd().args(["-p", "/tmp/bad name"]).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("nonportable character"));

        // Also check a colon
        let output2 = cmd().args(["-p", "/tmp/f:n"]).output().unwrap();
        assert_eq!(output2.status.code(), Some(1));
        let stderr2 = String::from_utf8_lossy(&output2.stderr);
        assert!(stderr2.contains("nonportable character"));

        // Valid POSIX name should pass
        let output3 = cmd().args(["-p", "/tmp/valid_name.txt"]).output().unwrap();
        assert!(output3.status.success());
    }

    #[test]
    fn test_pathchk_empty_name() {
        let output = cmd().args(["-P", ""]).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("empty file name"));
    }

    #[test]
    fn test_pathchk_leading_dash() {
        let output = cmd().args(["-P", "--", "-filename"]).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("leading '-'"));

        // Also check component with leading dash
        let output2 = cmd().args(["-P", "--", "/tmp/-badname"]).output().unwrap();
        assert_eq!(output2.status.code(), Some(1));
    }

    #[test]
    fn test_pathchk_matches_gnu() {
        // Valid path
        let gnu = Command::new("pathchk").arg("/tmp").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("/tmp").output().unwrap();
            assert_eq!(
                ours.status.code(),
                gnu.status.code(),
                "Exit code mismatch for valid path"
            );
        }

        // -p with invalid chars
        let gnu_p = Command::new("pathchk")
            .args(["-p", "/tmp/file name"])
            .output();
        if let Ok(gnu_p) = gnu_p {
            let ours_p = cmd().args(["-p", "/tmp/file name"]).output().unwrap();
            assert_eq!(
                ours_p.status.code(),
                gnu_p.status.code(),
                "Exit code mismatch for -p with spaces"
            );
        }

        // -P with leading dash
        let gnu_pd = Command::new("pathchk").args(["-P", "--", "-test"]).output();
        if let Ok(gnu_pd) = gnu_pd {
            let ours_pd = cmd().args(["-P", "--", "-test"]).output().unwrap();
            assert_eq!(
                ours_pd.status.code(),
                gnu_pd.status.code(),
                "Exit code mismatch for -P with leading dash"
            );
        }
    }
}
