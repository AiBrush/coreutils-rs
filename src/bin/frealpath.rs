// frealpath — print the resolved path
//
// Usage: realpath [OPTION]... FILE...

use std::path::{Component, Path, PathBuf};
use std::process;

const TOOL_NAME: &str = "realpath";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Default: all components must exist, resolve symlinks
    Canonicalize,
    /// -e: all components must exist (explicit)
    CanonicalizeExisting,
    /// -m: no existence requirements
    CanonicalizeMissing,
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut mode = Mode::Canonicalize;
    let mut no_symlinks = false;
    let mut zero = false;
    let mut quiet = false;
    let mut relative_to: Option<String> = None;
    let mut relative_base: Option<String> = None;
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
            "-e" | "--canonicalize-existing" => mode = Mode::CanonicalizeExisting,
            "-m" | "--canonicalize-missing" => mode = Mode::CanonicalizeMissing,
            "-s" | "--strip" | "--no-symlinks" => no_symlinks = true,
            "-z" | "--zero" => zero = true,
            "-q" | "--quiet" => quiet = true,
            "--relative-to" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option '--relative-to' requires an argument", TOOL_NAME);
                    process::exit(1);
                }
                relative_to = Some(args[i].clone());
            }
            "--relative-base" => {
                i += 1;
                if i >= args.len() {
                    eprintln!(
                        "{}: option '--relative-base' requires an argument",
                        TOOL_NAME
                    );
                    process::exit(1);
                }
                relative_base = Some(args[i].clone());
            }
            s if s.starts_with("--relative-to=") => {
                relative_to = Some(s["--relative-to=".len()..].to_string());
            }
            s if s.starts_with("--relative-base=") => {
                relative_base = Some(s["--relative-base=".len()..].to_string());
            }
            "--" => saw_dashdash = true,
            s if s.starts_with('-') && !s.starts_with("--") && s.len() > 1 => {
                for ch in s[1..].chars() {
                    match ch {
                        'e' => mode = Mode::CanonicalizeExisting,
                        'm' => mode = Mode::CanonicalizeMissing,
                        's' => no_symlinks = true,
                        'z' => zero = true,
                        'q' => quiet = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            _ => files.push(arg.clone()),
        }
        i += 1;
    }

    if files.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    // Resolve relative-to and relative-base directories
    let resolved_relative_to = relative_to.as_ref().map(|d| {
        resolve_path(d, mode, no_symlinks).unwrap_or_else(|_| make_absolute(Path::new(d)))
    });
    let resolved_relative_base = relative_base.as_ref().map(|d| {
        resolve_path(d, mode, no_symlinks).unwrap_or_else(|_| make_absolute(Path::new(d)))
    });

    let terminator = if zero { "\0" } else { "\n" };
    let mut exit_code = 0;

    for file in &files {
        match resolve_path(file, mode, no_symlinks) {
            Ok(resolved) => {
                let output =
                    apply_relative(&resolved, &resolved_relative_to, &resolved_relative_base);
                print!("{}{}", output.to_string_lossy(), terminator);
            }
            Err(e) => {
                exit_code = 1;
                if !quiet {
                    eprintln!(
                        "{}: {}: {}",
                        TOOL_NAME,
                        file,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                }
            }
        }
    }

    process::exit(exit_code);
}

fn resolve_path(path: &str, mode: Mode, no_symlinks: bool) -> Result<PathBuf, std::io::Error> {
    if no_symlinks {
        // Just normalize the path logically without resolving symlinks
        let abs = make_absolute(Path::new(path));
        let normalized = normalize_path(&abs);
        match mode {
            Mode::CanonicalizeExisting | Mode::Canonicalize => {
                // All components must exist
                if !normalized.exists() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "No such file or directory",
                    ));
                }
                Ok(normalized)
            }
            Mode::CanonicalizeMissing => Ok(normalized),
        }
    } else {
        match mode {
            Mode::Canonicalize | Mode::CanonicalizeExisting => std::fs::canonicalize(path),
            Mode::CanonicalizeMissing => canonicalize_missing(Path::new(path)),
        }
    }
}

/// Make a path absolute
fn make_absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    }
}

/// Normalize a path by resolving . and .. without touching the filesystem
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            c => {
                result.push(c.as_os_str());
            }
        }
    }
    result
}

/// Canonicalize a path where not all components need to exist.
fn canonicalize_missing(path: &Path) -> Result<PathBuf, std::io::Error> {
    let abs = make_absolute(path);

    // Try to canonicalize the whole thing first
    if let Ok(canon) = std::fs::canonicalize(&abs) {
        return Ok(canon);
    }

    let components: Vec<Component<'_>> = abs.components().collect();
    let mut resolved = PathBuf::new();
    let mut remaining_start = 0;

    // Find the longest resolvable prefix
    for i in (0..components.len()).rev() {
        let mut prefix = PathBuf::new();
        for c in &components[..=i] {
            prefix.push(c.as_os_str());
        }
        if let Ok(canon) = std::fs::canonicalize(&prefix) {
            resolved = canon;
            remaining_start = i + 1;
            break;
        }
    }

    if resolved.as_os_str().is_empty() {
        if let Some(Component::RootDir) = components.first() {
            resolved.push("/");
            remaining_start = 1;
        } else {
            resolved = std::env::current_dir()?;
        }
    }

    for c in &components[remaining_start..] {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(s) => {
                resolved.push(s);
                if resolved.symlink_metadata().is_ok()
                    && let Ok(canon) = std::fs::canonicalize(&resolved)
                {
                    resolved = canon;
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                resolved.push(c.as_os_str());
            }
        }
    }

    Ok(resolved)
}

/// Compute the relative path from `from` to `to`
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from_components: Vec<Component<'_>> = from.components().collect();
    let to_components: Vec<Component<'_>> = to.components().collect();

    // Find common prefix length
    let common_len = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut result = PathBuf::new();

    // Add ".." for each remaining component in `from`
    for _ in common_len..from_components.len() {
        result.push("..");
    }

    // Append remaining components of `to`
    for c in &to_components[common_len..] {
        result.push(c.as_os_str());
    }

    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

/// Apply --relative-to and --relative-base logic
fn apply_relative(
    path: &Path,
    relative_to: &Option<PathBuf>,
    relative_base: &Option<PathBuf>,
) -> PathBuf {
    // If --relative-base is given (without --relative-to), output relative if under base, else absolute
    if let Some(base) = relative_base
        && relative_to.is_none()
    {
        // If path starts with base, output relative to base
        if path.starts_with(base) {
            return relative_path(base, path);
        }
        // Otherwise return absolute
        return path.to_path_buf();
    }

    // If --relative-to is given
    if let Some(rel_to) = relative_to {
        // If --relative-base is also given, only make relative if both are under base
        if let Some(base) = relative_base {
            if path.starts_with(base) && rel_to.starts_with(base) {
                return relative_path(rel_to, path);
            }
            return path.to_path_buf();
        }
        return relative_path(rel_to, path);
    }

    path.to_path_buf()
}

fn print_help() {
    println!("Usage: {} [OPTION]... FILE...", TOOL_NAME);
    println!("Print the resolved absolute file name;");
    println!("all but the last component must exist");
    println!();
    println!("  -e, --canonicalize-existing   all components of the path must exist");
    println!("  -m, --canonicalize-missing    no path components need exist or be a directory");
    println!("  -s, --strip, --no-symlinks    don't expand symlinks");
    println!("  -z, --zero                    end each output line with NUL, not newline");
    println!("  -q, --quiet                   suppress most error messages");
    println!("      --relative-to=DIR         print the resolved path relative to DIR");
    println!("      --relative-base=DIR       print absolute paths unless paths below DIR");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("frealpath");
        Command::new(path)
    }

    #[test]
    fn test_realpath_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file.txt");
        fs::write(&file, "hello").unwrap();

        let output = cmd().arg(file.to_str().unwrap()).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let canon = fs::canonicalize(&file).unwrap();
        assert_eq!(stdout.trim(), canon.to_str().unwrap());
    }

    #[test]
    fn test_realpath_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        let link = dir.path().join("sym.txt");
        fs::write(&target, "data").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let output = cmd().arg(link.to_str().unwrap()).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let canon = fs::canonicalize(&target).unwrap();
        assert_eq!(stdout.trim(), canon.to_str().unwrap());
    }

    #[test]
    fn test_realpath_no_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real2.txt");
        let link = dir.path().join("sym2.txt");
        fs::write(&target, "data").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let output = cmd().args(["-s", link.to_str().unwrap()]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        // With -s, should NOT resolve the symlink — output should contain the symlink path
        // Use canonicalize on parent dir to handle macOS /var -> /private/var
        let canon_dir = fs::canonicalize(dir.path()).unwrap();
        let abs_link = canon_dir.join("sym2.txt");
        // On macOS, -s won't resolve /var -> /private/var, so compare path components
        let stdout_trimmed = stdout.trim();
        assert!(
            stdout_trimmed == abs_link.to_str().unwrap() || stdout_trimmed.ends_with("/sym2.txt"),
            "Expected path to sym2.txt, got: {}",
            stdout_trimmed
        );
    }

    #[test]
    fn test_realpath_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nonexistent").join("deep").join("path.txt");

        let output = cmd()
            .args(["-m", missing.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Should contain the path components even though they don't exist
        assert!(stdout.contains("nonexistent"));
        assert!(stdout.contains("path.txt"));
    }

    #[test]
    fn test_realpath_existing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does_not_exist.txt");

        let output = cmd()
            .args(["-e", missing.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("No such file or directory"));
    }

    #[test]
    fn test_realpath_relative_to() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("sub");
        let file = dir.path().join("file.txt");
        fs::create_dir(&subdir).unwrap();
        fs::write(&file, "test").unwrap();

        let canon_dir = fs::canonicalize(&subdir).unwrap();
        let output = cmd()
            .args([
                &format!("--relative-to={}", canon_dir.to_str().unwrap()),
                file.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "../file.txt");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_realpath_matches_gnu() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gnu_test.txt");
        fs::write(&file, "hello").unwrap();

        let gnu = Command::new("realpath")
            .arg(file.to_str().unwrap())
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg(file.to_str().unwrap()).output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
            let gnu_out = String::from_utf8_lossy(&gnu.stdout);
            let our_out = String::from_utf8_lossy(&ours.stdout);
            assert_eq!(our_out.trim(), gnu_out.trim(), "Output mismatch");
        }

        // Compare -m on nonexistent path
        let missing = dir.path().join("missing_gnu");
        let gnu_m = Command::new("realpath")
            .args(["-m", missing.to_str().unwrap()])
            .output();
        if let Ok(gnu_m) = gnu_m {
            let ours_m = cmd()
                .args(["-m", missing.to_str().unwrap()])
                .output()
                .unwrap();
            assert_eq!(
                ours_m.status.code(),
                gnu_m.status.code(),
                "Exit code mismatch for -m"
            );
            let gnu_out = String::from_utf8_lossy(&gnu_m.stdout);
            let our_out = String::from_utf8_lossy(&ours_m.stdout);
            assert_eq!(our_out.trim(), gnu_out.trim(), "Output mismatch for -m");
        }
    }
}
