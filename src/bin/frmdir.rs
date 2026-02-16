// frmdir — remove empty directories
//
// Usage: rmdir [OPTION]... DIRECTORY...

use std::path::PathBuf;
use std::process;

const TOOL_NAME: &str = "rmdir";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut parents = false;
    let mut ignore_nonempty = false;
    let mut verbose = false;
    let mut dirs: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    for arg in std::env::args().skip(1) {
        if saw_dashdash {
            dirs.push(arg);
            continue;
        }
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]... DIRECTORY...", TOOL_NAME);
                println!("Remove the DIRECTORY(ies), if they are empty.");
                println!();
                println!("      --ignore-fail-on-non-empty");
                println!("                 ignore each failure that is solely because a directory");
                println!("                 is non-empty");
                println!("  -p, --parents  remove DIRECTORY and its ancestors");
                println!("  -v, --verbose  output a diagnostic for every directory processed");
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "-p" | "--parents" => parents = true,
            "-v" | "--verbose" => verbose = true,
            "--ignore-fail-on-non-empty" => ignore_nonempty = true,
            "--" => saw_dashdash = true,
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                for ch in s[1..].chars() {
                    match ch {
                        'p' => parents = true,
                        'v' => verbose = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
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

    let mut exit_code = 0;
    for dir in &dirs {
        if parents {
            if let Err(code) = remove_parents(dir, ignore_nonempty, verbose) {
                exit_code = code;
            }
        } else if let Err(code) = remove_one(dir, ignore_nonempty, verbose) {
            exit_code = code;
        }
    }

    if exit_code != 0 {
        process::exit(exit_code);
    }
}

fn remove_one(dir: &str, ignore_nonempty: bool, verbose: bool) -> Result<(), i32> {
    match std::fs::remove_dir(dir) {
        Ok(()) => {
            if verbose {
                eprintln!("{}: removing directory, '{}'", TOOL_NAME, dir);
            }
            Ok(())
        }
        Err(e) => {
            if ignore_nonempty && is_nonempty_error(&e) {
                return Ok(());
            }
            eprintln!(
                "{}: failed to remove '{}': {}",
                TOOL_NAME,
                dir,
                coreutils_rs::common::io_error_msg(&e)
            );
            Err(1)
        }
    }
}

fn remove_parents(dir: &str, ignore_nonempty: bool, verbose: bool) -> Result<(), i32> {
    let mut path = PathBuf::from(dir);
    loop {
        let path_str = path.to_string_lossy().to_string();
        // Don't try to remove empty path or root
        if path_str.is_empty() || path_str == "/" || path_str == "." {
            break;
        }
        remove_one(&path_str, ignore_nonempty, verbose)?;
        if !path.pop() {
            break;
        }
    }
    Ok(())
}

fn is_nonempty_error(e: &std::io::Error) -> bool {
    // Check by ErrorKind for cross-platform support
    if e.kind() == std::io::ErrorKind::DirectoryNotEmpty {
        return true;
    }
    // Also check raw OS error codes as fallback
    #[cfg(unix)]
    {
        if matches!(e.raw_os_error(), Some(libc::ENOTEMPTY) | Some(libc::EEXIST)) {
            return true;
        }
    }
    #[cfg(windows)]
    {
        // ERROR_DIR_NOT_EMPTY = 145
        if e.raw_os_error() == Some(145) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("frmdir");
        Command::new(path)
    }

    #[test]
    fn test_rmdir_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("empty");
        fs::create_dir(&target).unwrap();

        let output = cmd().arg(target.to_str().unwrap()).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(!target.exists());
    }

    #[test]
    fn test_rmdir_nonempty_fails() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("notempty");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("file.txt"), "data").unwrap();

        let output = cmd().arg(target.to_str().unwrap()).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(target.exists()); // Should still exist
    }

    #[test]
    fn test_rmdir_ignore_nonempty() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("notempty2");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("file.txt"), "data").unwrap();

        let output = cmd()
            .args(["--ignore-fail-on-non-empty", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(target.exists()); // Still exists, but no error
    }

    #[test]
    fn test_rmdir_parents() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("a").join("b").join("c");
        fs::create_dir_all(&base).unwrap();

        let _output = cmd().args(["-p", base.to_str().unwrap()]).output().unwrap();
        // c, b, and a should all be removed
        // (exit code may be non-zero because it tries to remove the tempdir parent too)
        assert!(!dir.path().join("a").exists(), "a/ should be removed");
    }

    #[test]
    fn test_rmdir_verbose() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("verbosedir");
        fs::create_dir(&target).unwrap();

        let output = cmd()
            .args(["-v", target.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("removing directory"),
            "verbose should report removal"
        );
    }

    #[test]
    fn test_rmdir_nonexistent() {
        let output = cmd().arg("/nonexistent_rmdir_test_12345").output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_rmdir_matches_gnu() {
        let gnu = Command::new("rmdir")
            .arg("/nonexistent_rmdir_test_67890")
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("/nonexistent_rmdir_test_67890").output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
