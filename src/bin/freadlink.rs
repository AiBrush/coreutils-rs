// freadlink — print resolved symbolic links or canonical file names
//
// Usage: readlink [OPTION]... FILE...

use std::path::{Path, PathBuf};
use std::process;

const TOOL_NAME: &str = "readlink";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, PartialEq, Eq)]
enum CanonMode {
    None,
    /// -f: all components must exist
    Canonicalize,
    /// -e: all components must exist (stricter)
    CanonicalizeExisting,
    /// -m: no existence requirements
    CanonicalizeMissing,
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut mode = CanonMode::None;
    let mut no_newline = false;
    let mut quiet = false;
    let mut verbose = false;
    let mut zero = false;
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
            "-f" | "--canonicalize" => mode = CanonMode::Canonicalize,
            "-e" | "--canonicalize-existing" => mode = CanonMode::CanonicalizeExisting,
            "-m" | "--canonicalize-missing" => mode = CanonMode::CanonicalizeMissing,
            "-n" | "--no-newline" => no_newline = true,
            "-q" | "--quiet" | "--silent" => quiet = true,
            "-v" | "--verbose" => verbose = true,
            "-z" | "--zero" => zero = true,
            "--" => saw_dashdash = true,
            s if s.starts_with('-') && !s.starts_with("--") && s.len() > 1 => {
                for ch in s[1..].chars() {
                    match ch {
                        'f' => mode = CanonMode::Canonicalize,
                        'e' => mode = CanonMode::CanonicalizeExisting,
                        'm' => mode = CanonMode::CanonicalizeMissing,
                        'n' => no_newline = true,
                        'q' => quiet = true,
                        'v' => verbose = true,
                        'z' => zero = true,
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

    let terminator = if zero { "\0" } else { "\n" };
    let mut exit_code = 0;
    let multiple = files.len() > 1;

    for (idx, file) in files.iter().enumerate() {
        match resolve(file, mode) {
            Ok(resolved) => {
                let s = resolved.to_string_lossy();
                if no_newline && !multiple && idx == files.len() - 1 {
                    print!("{}", s);
                } else {
                    print!("{}{}", s, terminator);
                }
            }
            Err(e) => {
                exit_code = 1;
                if verbose || !quiet {
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

fn resolve(path: &str, mode: CanonMode) -> Result<PathBuf, std::io::Error> {
    match mode {
        CanonMode::None => {
            // Just read the symlink target
            std::fs::read_link(path)
        }
        CanonMode::Canonicalize | CanonMode::CanonicalizeExisting => {
            // All components must exist
            std::fs::canonicalize(path)
        }
        CanonMode::CanonicalizeMissing => canonicalize_missing(Path::new(path)),
    }
}

/// Canonicalize a path where not all components need to exist.
/// Resolve what we can, then normalize the rest.
fn canonicalize_missing(path: &Path) -> Result<PathBuf, std::io::Error> {
    // Make the path absolute first
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    // Try to canonicalize the whole thing first
    if let Ok(canon) = std::fs::canonicalize(&abs) {
        return Ok(canon);
    }

    // Split into components, resolve as much as possible
    let mut resolved = PathBuf::new();
    let mut components: Vec<std::path::Component<'_>> = abs.components().collect();
    let mut remaining_start = 0;

    // Try to resolve from the full path down to find the longest resolvable prefix
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

    // If nothing resolved, start from root
    if resolved.as_os_str().is_empty() {
        // At minimum, the root component should be there
        if let Some(std::path::Component::RootDir) = components.first() {
            resolved.push("/");
            remaining_start = 1;
        } else {
            // Relative path that can't be resolved at all - use cwd
            resolved = std::env::current_dir()?;
        }
    }

    // Append remaining components, normalizing . and ..
    for c in components.drain(remaining_start..) {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::Normal(s) => {
                resolved.push(s);
                // If this component now exists, try to fully resolve it
                if resolved.symlink_metadata().is_ok()
                    && let Ok(canon) = std::fs::canonicalize(&resolved)
                {
                    resolved = canon;
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                resolved.push(c.as_os_str());
            }
        }
    }

    Ok(resolved)
}

fn print_help() {
    println!("Usage: {} [OPTION]... FILE...", TOOL_NAME);
    println!("Print value of a symbolic link or canonical file name");
    println!();
    println!("  -f, --canonicalize            canonicalize by following every symlink in");
    println!("                                every component of the given name recursively;");
    println!("                                all but the last component must exist");
    println!("  -e, --canonicalize-existing   canonicalize by following every symlink in");
    println!("                                every component of the given name recursively,");
    println!("                                all components must exist");
    println!("  -m, --canonicalize-missing    canonicalize by following every symlink in");
    println!("                                every component of the given name recursively,");
    println!("                                without requirements on components existence");
    println!("  -n, --no-newline              do not output the trailing delimiter");
    println!("  -q, --quiet, --silent         suppress most error messages");
    println!("  -v, --verbose                 report error messages");
    println!("  -z, --zero                    end each output line with NUL, not newline");
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
        path.push("freadlink");
        Command::new(path)
    }

    #[test]
    fn test_readlink_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, "content").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let output = cmd().arg(link.to_str().unwrap()).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), target.to_str().unwrap());
    }

    #[test]
    fn test_readlink_canonicalize() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        let link = dir.path().join("sym.txt");
        fs::write(&target, "data").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let output = cmd()
            .args(["-f", link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Canonicalized path should resolve to the real target
        let canon = fs::canonicalize(&target).unwrap();
        assert_eq!(stdout.trim(), canon.to_str().unwrap());
    }

    #[test]
    fn test_readlink_not_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let regular = dir.path().join("regular.txt");
        fs::write(&regular, "hello").unwrap();

        let output = cmd().arg(regular.to_str().unwrap()).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_readlink_no_newline() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target2.txt");
        let link = dir.path().join("link2.txt");
        fs::write(&target, "content").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let output = cmd()
            .args(["-n", link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Should NOT end with newline
        assert!(!stdout.ends_with('\n'), "output should not end with newline");
        assert_eq!(stdout.as_ref(), target.to_str().unwrap());
    }

    #[test]
    fn test_readlink_matches_gnu() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gnu_target.txt");
        let link = dir.path().join("gnu_link.txt");
        fs::write(&target, "test").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let gnu = Command::new("readlink")
            .arg(link.to_str().unwrap())
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg(link.to_str().unwrap()).output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
            let gnu_out = String::from_utf8_lossy(&gnu.stdout);
            let our_out = String::from_utf8_lossy(&ours.stdout);
            assert_eq!(our_out.trim(), gnu_out.trim(), "Output mismatch");
        }

        // Also compare -f behavior
        let gnu_f = Command::new("readlink")
            .args(["-f", link.to_str().unwrap()])
            .output();
        if let Ok(gnu_f) = gnu_f {
            let ours_f = cmd()
                .args(["-f", link.to_str().unwrap()])
                .output()
                .unwrap();
            assert_eq!(
                ours_f.status.code(),
                gnu_f.status.code(),
                "Exit code mismatch for -f"
            );
            let gnu_out = String::from_utf8_lossy(&gnu_f.stdout);
            let our_out = String::from_utf8_lossy(&ours_f.stdout);
            assert_eq!(our_out.trim(), gnu_out.trim(), "Output mismatch for -f");
        }

        // Compare non-symlink behavior
        let regular = dir.path().join("regular_gnu.txt");
        fs::write(&regular, "test").unwrap();
        let gnu_reg = Command::new("readlink")
            .arg(regular.to_str().unwrap())
            .output();
        if let Ok(gnu_reg) = gnu_reg {
            let ours_reg = cmd().arg(regular.to_str().unwrap()).output().unwrap();
            assert_eq!(
                ours_reg.status.code(),
                gnu_reg.status.code(),
                "Exit code mismatch for regular file"
            );
        }
    }
}
