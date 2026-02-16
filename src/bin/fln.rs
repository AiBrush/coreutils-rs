#[cfg(not(unix))]
fn main() {
    eprintln!("ln: only available on Unix");
    std::process::exit(1);
}

// fln -- make links between files
//
// Usage: ln [OPTION]... [-T] TARGET LINK_NAME
//        ln [OPTION]... TARGET
//        ln [OPTION]... TARGET... DIRECTORY
//        ln [OPTION]... -t DIRECTORY TARGET...

#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "ln";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
const DEFAULT_BACKUP_SUFFIX: &str = "~";

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg(unix)]
enum BackupMode {
    None,
    Simple,
}

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut symbolic = false;
    let mut force = false;
    let mut no_deref = false;
    let mut verbose = false;
    let mut relative = false;
    let mut backup = BackupMode::None;
    let mut suffix = DEFAULT_BACKUP_SUFFIX.to_string();
    let mut target_dir: Option<String> = None;
    let mut no_target_dir = false;
    let mut _logical = false;
    let mut _physical = false;
    let mut _interactive = false;
    let mut operands: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if saw_dashdash {
            operands.push(arg.clone());
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
            "-s" | "--symbolic" => symbolic = true,
            "-f" | "--force" => force = true,
            "-n" | "--no-dereference" => no_deref = true,
            "-v" | "--verbose" => verbose = true,
            "-r" | "--relative" => relative = true,
            "-b" | "--backup" => backup = BackupMode::Simple,
            "-i" | "--interactive" => _interactive = true,
            "-L" | "--logical" => {
                _logical = true;
                _physical = false;
            }
            "-P" | "--physical" => {
                _physical = true;
                _logical = false;
            }
            "-T" | "--no-target-directory" => no_target_dir = true,
            "-t" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 't'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                target_dir = Some(args[i].clone());
            }
            "--" => saw_dashdash = true,
            _ if arg.starts_with("--target-directory=") => {
                target_dir = Some(arg["--target-directory=".len()..].to_string());
            }
            _ if arg.starts_with("--suffix=") => {
                suffix = arg["--suffix=".len()..].to_string();
                backup = BackupMode::Simple;
            }
            _ if arg.starts_with("-S") && arg.len() > 2 => {
                suffix = arg[2..].to_string();
                backup = BackupMode::Simple;
            }
            _ if arg == "-S" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'S'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                suffix = args[i].clone();
                backup = BackupMode::Simple;
            }
            _ if arg.starts_with("-t") && arg.len() > 2 => {
                target_dir = Some(arg[2..].to_string());
            }
            _ if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") => {
                // Combined short flags
                let chars: Vec<char> = arg[1..].chars().collect();
                let mut j = 0;
                while j < chars.len() {
                    match chars[j] {
                        's' => symbolic = true,
                        'f' => force = true,
                        'n' => no_deref = true,
                        'v' => verbose = true,
                        'r' => relative = true,
                        'b' => backup = BackupMode::Simple,
                        'i' => _interactive = true,
                        'L' => {
                            _logical = true;
                            _physical = false;
                        }
                        'P' => {
                            _physical = true;
                            _logical = false;
                        }
                        'T' => no_target_dir = true,
                        'S' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 'S'", TOOL_NAME);
                                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                                    process::exit(1);
                                }
                                suffix = args[i].clone();
                            } else {
                                suffix = rest;
                            }
                            backup = BackupMode::Simple;
                            break;
                        }
                        't' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 't'", TOOL_NAME);
                                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                                    process::exit(1);
                                }
                                target_dir = Some(args[i].clone());
                            } else {
                                target_dir = Some(rest);
                            }
                            break;
                        }
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, chars[j]);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    j += 1;
                }
            }
            _ => operands.push(arg.clone()),
        }
        i += 1;
    }

    if operands.is_empty() {
        eprintln!("{}: missing file operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let mut exit_code = 0;

    if let Some(ref dir) = target_dir {
        // -t DIRECTORY TARGET...
        // All operands are targets; link them into DIRECTORY
        if !Path::new(dir).is_dir() {
            eprintln!("{}: target '{}' is not a directory", TOOL_NAME, dir);
            process::exit(1);
        }
        for target in &operands {
            let link_name = link_name_in_dir(target, dir);
            if let Err(code) = make_link(
                target, &link_name, symbolic, force, no_deref, verbose, relative, backup, &suffix,
            ) {
                exit_code = code;
            }
        }
    } else if no_target_dir {
        // -T: treat LINK_NAME as a normal file, not a directory
        if operands.len() != 2 {
            if operands.len() < 2 {
                eprintln!(
                    "{}: missing destination file operand after '{}'",
                    TOOL_NAME, operands[0]
                );
            } else {
                eprintln!("{}: extra operand '{}'", TOOL_NAME, operands[2]);
            }
            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
            process::exit(1);
        }
        if let Err(code) = make_link(
            &operands[0],
            &operands[1],
            symbolic,
            force,
            no_deref,
            verbose,
            relative,
            backup,
            &suffix,
        ) {
            exit_code = code;
        }
    } else if operands.len() == 1 {
        // Single operand: create link in current directory
        let target = &operands[0];
        let basename = Path::new(target)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| target.clone());
        if let Err(code) = make_link(
            target, &basename, symbolic, force, no_deref, verbose, relative, backup, &suffix,
        ) {
            exit_code = code;
        }
    } else if operands.len() == 2 {
        let target = &operands[0];
        let dest = &operands[1];
        // If dest is a directory (and we're not using -n on a symlink-to-dir), link into it
        let dest_is_dir = if no_deref {
            // With -n, don't dereference: check the path itself
            Path::new(dest).symlink_metadata().is_ok_and(|m| m.is_dir())
        } else {
            Path::new(dest).is_dir()
        };
        if dest_is_dir {
            let link_name = link_name_in_dir(target, dest);
            if let Err(code) = make_link(
                target, &link_name, symbolic, force, no_deref, verbose, relative, backup, &suffix,
            ) {
                exit_code = code;
            }
        } else if let Err(code) = make_link(
            target, dest, symbolic, force, no_deref, verbose, relative, backup, &suffix,
        ) {
            exit_code = code;
        }
    } else {
        // Multiple operands: last must be a directory
        let dir = &operands[operands.len() - 1];
        if !Path::new(dir).is_dir() {
            eprintln!("{}: target '{}' is not a directory", TOOL_NAME, dir);
            process::exit(1);
        }
        for target in &operands[..operands.len() - 1] {
            let link_name = link_name_in_dir(target, dir);
            if let Err(code) = make_link(
                target, &link_name, symbolic, force, no_deref, verbose, relative, backup, &suffix,
            ) {
                exit_code = code;
            }
        }
    }

    if exit_code != 0 {
        process::exit(exit_code);
    }
}

/// Compute the link name when linking TARGET into DIRECTORY.
#[cfg(unix)]
fn link_name_in_dir(target: &str, dir: &str) -> String {
    let basename = Path::new(target)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| target.to_string());
    let p = Path::new(dir).join(&basename);
    p.to_string_lossy().to_string()
}

/// Create a link from target to link_name.
#[allow(clippy::too_many_arguments)]
#[cfg(unix)]
fn make_link(
    target: &str,
    link_name: &str,
    symbolic: bool,
    force: bool,
    _no_deref: bool,
    verbose: bool,
    relative: bool,
    backup: BackupMode,
    suffix: &str,
) -> Result<(), i32> {
    let link_path = Path::new(link_name);

    // Check if link_name already exists
    let link_exists = link_path.symlink_metadata().is_ok();

    if link_exists {
        // Make backup if requested
        if backup == BackupMode::Simple {
            let backup_name = format!("{}{}", link_name, suffix);
            if let Err(e) = std::fs::rename(link_name, &backup_name) {
                eprintln!(
                    "{}: cannot backup '{}': {}",
                    TOOL_NAME,
                    link_name,
                    coreutils_rs::common::io_error_msg(&e)
                );
                return Err(1);
            }
        } else if force {
            if let Err(e) = remove_dest(link_name) {
                eprintln!(
                    "{}: cannot remove '{}': {}",
                    TOOL_NAME,
                    link_name,
                    coreutils_rs::common::io_error_msg(&e)
                );
                return Err(1);
            }
        } else {
            eprintln!(
                "{}: failed to create {} link '{}': File exists",
                TOOL_NAME,
                if symbolic { "symbolic" } else { "hard" },
                link_name
            );
            return Err(1);
        }
    }

    let actual_target = if symbolic && relative {
        // Compute relative path from link location to target
        compute_relative_target(target, link_name)
    } else {
        target.to_string()
    };

    let result = if symbolic {
        std::os::unix::fs::symlink(&actual_target, link_name)
    } else {
        std::fs::hard_link(target, link_name)
    };

    match result {
        Ok(()) => {
            if verbose {
                if symbolic {
                    println!("'{}' -> '{}'", link_name, actual_target);
                } else {
                    println!("'{}' => '{}'", link_name, target);
                }
            }
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "{}: failed to create {} link '{}' -> '{}': {}",
                TOOL_NAME,
                if symbolic { "symbolic" } else { "hard" },
                link_name,
                actual_target,
                coreutils_rs::common::io_error_msg(&e)
            );
            Err(1)
        }
    }
}

/// Remove a destination file or symlink.
#[cfg(unix)]
fn remove_dest(path: &str) -> Result<(), std::io::Error> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Compute a relative path from the link location directory to the target.
#[cfg(unix)]
fn compute_relative_target(target: &str, link_name: &str) -> String {
    let target_abs = make_absolute(target);
    let link_abs = make_absolute(link_name);

    let link_dir = match Path::new(&link_abs).parent() {
        Some(p) => p.to_path_buf(),
        None => return target.to_string(),
    };

    make_relative(&target_abs, &link_dir)
}

#[cfg(unix)]
fn make_absolute(path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(p),
            Err(_) => p.to_path_buf(),
        }
    }
}

/// Compute a relative path from `from_dir` to `to_path`.
#[cfg(unix)]
fn make_relative(to_path: &Path, from_dir: &Path) -> String {
    // Normalize both paths by collecting components
    let to_components: Vec<_> = to_path.components().collect();
    let from_components: Vec<_> = from_dir.components().collect();

    // Find the common prefix length
    let mut common = 0;
    let max_common = to_components.len().min(from_components.len());
    while common < max_common && to_components[common] == from_components[common] {
        common += 1;
    }

    // Build the relative path: go up from from_dir, then down to to_path
    let mut result = PathBuf::new();
    for _ in common..from_components.len() {
        result.push("..");
    }
    for comp in &to_components[common..] {
        result.push(comp.as_os_str());
    }

    if result.as_os_str().is_empty() {
        ".".to_string()
    } else {
        result.to_string_lossy().to_string()
    }
}

#[cfg(unix)]
fn print_help() {
    println!("Usage: {} [OPTION]... [-T] TARGET LINK_NAME", TOOL_NAME);
    println!("  or:  {} [OPTION]... TARGET", TOOL_NAME);
    println!("  or:  {} [OPTION]... TARGET... DIRECTORY", TOOL_NAME);
    println!("  or:  {} [OPTION]... -t DIRECTORY TARGET...", TOOL_NAME);
    println!("In the 1st form, create a link to TARGET with the name LINK_NAME.");
    println!("In the 2nd form, create a link to TARGET in the current directory.");
    println!("In the 3rd and 4th forms, create links to each TARGET in DIRECTORY.");
    println!("Create hard links by default, symbolic links with --symbolic.");
    println!();
    println!("  -b                         like --backup but does not accept an argument");
    println!("  -f, --force                remove existing destination files");
    println!("  -i, --interactive          prompt whether to remove destinations");
    println!("  -L, --logical              dereference TARGETs that are symbolic links");
    println!("  -n, --no-dereference       treat LINK_NAME as a normal file if");
    println!("                               it is a symbolic link to a directory");
    println!("  -P, --physical             make hard links directly to symbolic links");
    println!("  -r, --relative             create symbolic links relative to link location");
    println!("  -s, --symbolic             make symbolic links instead of hard links");
    println!("  -S, --suffix=SUFFIX        override the usual backup suffix");
    println!("  -t, --target-directory=DIRECTORY  specify the DIRECTORY in which to create");
    println!("                               the links");
    println!("  -T, --no-target-directory   treat LINK_NAME as a normal file always");
    println!("  -v, --verbose              print name of each linked file");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
    println!();
    println!("The backup suffix is '~', unless set with --suffix or SIMPLE_BACKUP_SUFFIX.");
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fln");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("ln"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("ln"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_hard_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, "hello").unwrap();

        let output = cmd()
            .args([target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(link.exists());

        // Verify same inode
        let target_meta = fs::metadata(&target).unwrap();
        let link_meta = fs::metadata(&link).unwrap();
        assert_eq!(target_meta.ino(), link_meta.ino());
        assert_eq!(target_meta.nlink(), 2);
    }

    #[test]
    fn test_symbolic_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("symlink.txt");
        fs::write(&target, "hello").unwrap();

        let output = cmd()
            .args(["-s", target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());

        let link_target = fs::read_link(&link).unwrap();
        assert_eq!(link_target, target);
    }

    #[test]
    fn test_force_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, "hello").unwrap();
        fs::write(&link, "existing").unwrap();

        // Without -f, should fail
        let output = cmd()
            .args(["-s", target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));

        // With -f, should succeed
        let output = cmd()
            .args(["-sf", target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn test_verbose() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("verbose_link.txt");
        fs::write(&target, "hello").unwrap();

        let output = cmd()
            .args(["-sv", target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("->"), "verbose output should contain '->'");
    }

    #[test]
    fn test_relative_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let target = dir.path().join("target.txt");
        let link = sub.join("rel_link.txt");
        fs::write(&target, "hello").unwrap();

        let output = cmd()
            .args(["-sr", target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let link_target = fs::read_link(&link).unwrap();
        // Should be relative, like ../target.txt
        assert!(
            link_target.to_str().unwrap().starts_with(".."),
            "relative link should start with '..': got {:?}",
            link_target
        );
        // The link should still resolve correctly
        assert_eq!(fs::read_to_string(&link).unwrap(), "hello");
    }

    #[test]
    fn test_backup() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("backup_link.txt");
        fs::write(&target, "new_content").unwrap();
        fs::write(&link, "old_content").unwrap();

        let output = cmd()
            .args(["-sb", target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        // Original should be backed up
        let backup = dir.path().join("backup_link.txt~");
        assert!(backup.exists(), "backup file should exist");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "old_content");

        // Link should point to target
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn test_backup_custom_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("custom_link.txt");
        fs::write(&target, "new").unwrap();
        fs::write(&link, "old").unwrap();

        let output = cmd()
            .args([
                "-s",
                "--suffix=.bak",
                target.to_str().unwrap(),
                link.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());

        let backup = dir.path().join("custom_link.txt.bak");
        assert!(
            backup.exists(),
            "backup file with custom suffix should exist"
        );
        assert_eq!(fs::read_to_string(&backup).unwrap(), "old");
    }

    #[test]
    fn test_target_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let dest_dir = dir.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();
        fs::write(&target, "hello").unwrap();

        let output = cmd()
            .args([
                "-s",
                "-t",
                dest_dir.to_str().unwrap(),
                target.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());

        let expected_link = dest_dir.join("target.txt");
        assert!(
            expected_link
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink(),
            "link should be created in target directory"
        );
    }

    #[test]
    fn test_no_target_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("notdir_link");
        fs::write(&target, "hello").unwrap();

        let output = cmd()
            .args(["-sT", target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn test_no_target_directory_extra_operand() {
        let output = cmd().args(["-T", "a", "b", "c"]).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("extra operand"));
    }

    #[test]
    fn test_multiple_targets_to_directory() {
        let dir = tempfile::tempdir().unwrap();
        let t1 = dir.path().join("t1.txt");
        let t2 = dir.path().join("t2.txt");
        let dest = dir.path().join("dest");
        fs::write(&t1, "a").unwrap();
        fs::write(&t2, "b").unwrap();
        fs::create_dir(&dest).unwrap();

        let output = cmd()
            .args([
                "-s",
                t1.to_str().unwrap(),
                t2.to_str().unwrap(),
                dest.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            dest.join("t1.txt")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            dest.join("t2.txt")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn test_missing_operand() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("missing file operand"));
    }

    #[test]
    fn test_hard_link_nonexistent_target() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("bad_link.txt");

        let output = cmd()
            .args(["/nonexistent_ln_test_file", link.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("failed to create"));
    }

    #[test]
    fn test_symlink_to_nonexistent_target() {
        // Symlinks can point to nonexistent targets (dangling link)
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("dangling.txt");

        let output = cmd()
            .args(["-s", "/nonexistent_target_12345", link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn test_link_exists_no_force() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("existing.txt");
        fs::write(&target, "a").unwrap();
        fs::write(&link, "b").unwrap();

        let output = cmd()
            .args([target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("File exists"));
    }

    #[test]
    fn test_link_into_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let dest = dir.path().join("dest");
        fs::write(&target, "hello").unwrap();
        fs::create_dir(&dest).unwrap();

        let output = cmd()
            .args(["-s", target.to_str().unwrap(), dest.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let expected = dest.join("target.txt");
        assert!(
            expected
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn test_matches_gnu_hard_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gnu_target.txt");
        fs::write(&target, "test").unwrap();

        let gnu_link = dir.path().join("gnu_link.txt");
        let our_link = dir.path().join("our_link.txt");

        let gnu = Command::new("ln")
            .args([target.to_str().unwrap(), gnu_link.to_str().unwrap()])
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd()
                .args([target.to_str().unwrap(), our_link.to_str().unwrap()])
                .output()
                .unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");

            if gnu.status.success() {
                // Both links should exist and point to same inode
                let gnu_ino = fs::metadata(&gnu_link).unwrap().ino();
                let our_ino = fs::metadata(&our_link).unwrap().ino();
                let target_ino = fs::metadata(&target).unwrap().ino();
                assert_eq!(gnu_ino, target_ino);
                assert_eq!(our_ino, target_ino);
            }
        }
    }

    #[test]
    fn test_matches_gnu_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gnu_sym_target.txt");
        fs::write(&target, "test").unwrap();

        let gnu_link = dir.path().join("gnu_sym.txt");
        let our_link = dir.path().join("our_sym.txt");

        let gnu = Command::new("ln")
            .args(["-s", target.to_str().unwrap(), gnu_link.to_str().unwrap()])
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd()
                .args(["-s", target.to_str().unwrap(), our_link.to_str().unwrap()])
                .output()
                .unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");

            if gnu.status.success() {
                let gnu_target = fs::read_link(&gnu_link).unwrap();
                let our_target = fs::read_link(&our_link).unwrap();
                assert_eq!(gnu_target, our_target, "Symlink targets should match");
            }
        }
    }

    #[test]
    fn test_matches_gnu_force_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gnu_force_target.txt");
        let gnu_link = dir.path().join("gnu_force.txt");
        let our_link = dir.path().join("our_force.txt");
        fs::write(&target, "new").unwrap();
        fs::write(&gnu_link, "old").unwrap();
        fs::write(&our_link, "old").unwrap();

        let gnu = Command::new("ln")
            .args(["-sf", target.to_str().unwrap(), gnu_link.to_str().unwrap()])
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd()
                .args(["-sf", target.to_str().unwrap(), our_link.to_str().unwrap()])
                .output()
                .unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }

    // Unit tests for relative path computation
    #[test]
    fn test_make_relative_sibling() {
        use std::path::Path;
        let result = super::make_relative(Path::new("/a/b/target.txt"), Path::new("/a/b"));
        assert_eq!(result, "target.txt");
    }

    #[test]
    fn test_make_relative_parent() {
        use std::path::Path;
        let result = super::make_relative(Path::new("/a/target.txt"), Path::new("/a/b"));
        assert_eq!(result, "../target.txt");
    }

    #[test]
    fn test_make_relative_deep() {
        use std::path::Path;
        let result = super::make_relative(Path::new("/a/b/c/target.txt"), Path::new("/a/x/y"));
        assert_eq!(result, "../../b/c/target.txt");
    }

    #[test]
    fn test_single_operand() {
        // ln -s /some/target creates a link in the current directory with the basename
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        let work_dir = dir.path().join("work");
        fs::create_dir(&src_dir).unwrap();
        fs::create_dir(&work_dir).unwrap();

        let target = src_dir.join("single_target.txt");
        fs::write(&target, "data").unwrap();

        // Run in work_dir so the link is created there (no conflict)
        let output = cmd()
            .args(["-s", target.to_str().unwrap()])
            .current_dir(&work_dir)
            .output()
            .unwrap();
        assert!(output.status.success());

        let expected_link = work_dir.join("single_target.txt");
        assert!(
            expected_link
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink(),
            "link should be created in current directory"
        );
        assert_eq!(fs::read_to_string(&expected_link).unwrap(), "data");
    }

    #[test]
    fn test_combined_flags() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("combined.txt");
        let link = dir.path().join("combined_link.txt");
        fs::write(&target, "data").unwrap();
        fs::write(&link, "old").unwrap();

        // -sfv = symbolic, force, verbose
        let output = cmd()
            .args(["-sfv", target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("->"));
    }

    #[test]
    fn test_no_deref_flag() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nd_target.txt");
        let link = dir.path().join("nd_link.txt");
        fs::write(&target, "data").unwrap();

        // -n should work without error
        let output = cmd()
            .args(["-sn", target.to_str().unwrap(), link.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
    }
}
