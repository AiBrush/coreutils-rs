#[cfg(not(unix))]
fn main() {
    eprintln!("chroot: only available on Unix");
    std::process::exit(1);
}

// fchroot -- run command or interactive shell with special root directory
//
// Usage: chroot [OPTION] NEWROOT [COMMAND [ARG]...]

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "chroot";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut userspec: Option<String> = None;
    let mut groups_list: Option<String> = None;
    let mut skip_chdir = false;
    let mut positional_start: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION] NEWROOT [COMMAND [ARG]...]", TOOL_NAME);
                println!("  or:  {} OPTION", TOOL_NAME);
                println!("Run COMMAND with root directory set to NEWROOT.");
                println!();
                println!("  --userspec=USER:GROUP  specify user and group (ID or name) to use");
                println!("  --groups=G_LIST        specify supplementary groups as g1,g2,..,gN");
                println!("  --skip-chdir           do not change working directory to '/'");
                println!("      --help             display this help and exit");
                println!("      --version          output version information and exit");
                println!();
                println!("If no command is given, run '\"$SHELL\" -i' (default: '/bin/sh -i').");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--skip-chdir" => skip_chdir = true,
            s if s.starts_with("--userspec=") => {
                userspec = Some(s["--userspec=".len()..].to_string());
            }
            s if s.starts_with("--groups=") => {
                groups_list = Some(s["--groups=".len()..].to_string());
            }
            "--" => {
                i += 1;
                if i < args.len() {
                    positional_start = Some(i);
                }
                break;
            }
            _ => {
                positional_start = Some(i);
                break;
            }
        }
        i += 1;
    }

    let start = positional_start.unwrap_or_else(|| {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(125);
    });

    if start >= args.len() {
        eprintln!("{}: missing operand", TOOL_NAME);
        process::exit(125);
    }

    let newroot = &args[start];

    // Determine command to run
    let (command, command_args): (String, Vec<String>) = if start + 1 < args.len() {
        let cmd = args[start + 1].clone();
        let cmd_args = args[start + 2..].to_vec();
        (cmd, cmd_args)
    } else {
        // Default: $SHELL -i or /bin/sh -i
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        (shell, vec!["-i".to_string()])
    };

    // Parse userspec
    let mut target_uid: Option<libc::uid_t> = None;
    let mut target_gid: Option<libc::gid_t> = None;

    if let Some(ref spec) = userspec {
        let parts: Vec<&str> = spec.splitn(2, ':').collect();
        let user_part = parts[0];
        let group_part = if parts.len() > 1 {
            Some(parts[1])
        } else {
            None
        };

        if !user_part.is_empty() {
            target_uid = Some(resolve_user(user_part).unwrap_or_else(|| {
                eprintln!("{}: invalid user: '{}'", TOOL_NAME, user_part);
                process::exit(125);
            }));
        }

        if let Some(group) = group_part
            && !group.is_empty()
        {
            target_gid = Some(resolve_group(group).unwrap_or_else(|| {
                eprintln!("{}: invalid group: '{}'", TOOL_NAME, group);
                process::exit(125);
            }));
        }
    }

    // Parse supplementary groups
    let mut sup_groups: Vec<libc::gid_t> = Vec::new();
    if let Some(ref gl) = groups_list {
        for g in gl.split(',') {
            let g = g.trim();
            if g.is_empty() {
                continue;
            }
            sup_groups.push(resolve_group(g).unwrap_or_else(|| {
                eprintln!("{}: invalid group: '{}'", TOOL_NAME, g);
                process::exit(125);
            }));
        }
    }

    // Perform chroot
    let c_newroot = CString::new(newroot.as_str()).unwrap_or_else(|_| {
        eprintln!(
            "{}: cannot change root directory to '{}': Invalid argument",
            TOOL_NAME, newroot
        );
        process::exit(125);
    });

    if unsafe { libc::chroot(c_newroot.as_ptr()) } != 0 {
        let err = std::io::Error::last_os_error();
        eprintln!(
            "{}: cannot change root directory to '{}': {}",
            TOOL_NAME,
            newroot,
            coreutils_rs::common::io_error_msg(&err)
        );
        process::exit(125);
    }

    // Change to / unless --skip-chdir
    if !skip_chdir {
        let c_slash = CString::new("/").unwrap();
        if unsafe { libc::chdir(c_slash.as_ptr()) } != 0 {
            let err = std::io::Error::last_os_error();
            eprintln!(
                "{}: cannot chdir to '/': {}",
                TOOL_NAME,
                coreutils_rs::common::io_error_msg(&err)
            );
            process::exit(125);
        }
    }

    // Set supplementary groups
    if !sup_groups.is_empty()
        && unsafe { libc::setgroups(sup_groups.len() as _, sup_groups.as_ptr()) } != 0
    {
        let err = std::io::Error::last_os_error();
        eprintln!(
            "{}: failed to set supplementary groups: {}",
            TOOL_NAME,
            coreutils_rs::common::io_error_msg(&err)
        );
        process::exit(125);
    }

    // Set GID
    if let Some(gid) = target_gid
        && unsafe { libc::setgid(gid) } != 0
    {
        let err = std::io::Error::last_os_error();
        eprintln!(
            "{}: failed to set group-id: {}",
            TOOL_NAME,
            coreutils_rs::common::io_error_msg(&err)
        );
        process::exit(125);
    }

    // Set UID
    if let Some(uid) = target_uid
        && unsafe { libc::setuid(uid) } != 0
    {
        let err = std::io::Error::last_os_error();
        eprintln!(
            "{}: failed to set user-id: {}",
            TOOL_NAME,
            coreutils_rs::common::io_error_msg(&err)
        );
        process::exit(125);
    }

    // Exec the command
    let cmd_args_refs: Vec<&str> = command_args.iter().map(|s| s.as_str()).collect();
    let err = std::process::Command::new(&command)
        .args(&cmd_args_refs)
        .exec();

    let code = if err.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    };
    eprintln!(
        "{}: failed to run command '{}': {}",
        TOOL_NAME,
        command,
        coreutils_rs::common::io_error_msg(&err)
    );
    process::exit(code);
}

#[cfg(unix)]
fn resolve_user(spec: &str) -> Option<libc::uid_t> {
    // Try numeric first
    if let Ok(uid) = spec.parse::<libc::uid_t>() {
        return Some(uid);
    }
    // Try by name
    let c_name = CString::new(spec).ok()?;
    let pw = unsafe { libc::getpwnam(c_name.as_ptr()) };
    if pw.is_null() {
        None
    } else {
        Some(unsafe { (*pw).pw_uid })
    }
}

#[cfg(unix)]
fn resolve_group(spec: &str) -> Option<libc::gid_t> {
    // Try numeric first
    if let Ok(gid) = spec.parse::<libc::gid_t>() {
        return Some(gid);
    }
    // Try by name
    let c_name = CString::new(spec).ok()?;
    let gr = unsafe { libc::getgrnam(c_name.as_ptr()) };
    if gr.is_null() {
        None
    } else {
        Some(unsafe { (*gr).gr_gid })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fchroot");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("NEWROOT"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("(fcoreutils)"));
    }

    #[test]
    fn test_missing_operand() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(125));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("missing operand"));
    }

    #[test]
    fn test_error_without_root() {
        // chroot requires root privileges, so this should fail as a normal user
        let output = cmd().arg("/tmp").output().unwrap();
        // Should fail with permission error (exit 125)
        let code = output.status.code().unwrap();
        // Either 125 (chroot fails) or 126/127 (command fails after chroot)
        assert_ne!(code, 0, "chroot should fail without root privileges");
    }

    #[test]
    fn test_nonexistent_directory() {
        let output = cmd().arg("/nonexistent_dir_xyz_999").output().unwrap();
        let code = output.status.code().unwrap();
        assert_ne!(code, 0);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot change root directory"),
            "Should report chroot failure"
        );
    }

    #[test]
    fn test_matches_gnu_error_no_root() {
        // Both should fail for non-root users
        let gnu = Command::new("chroot").arg("/tmp").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("/tmp").output().unwrap();
            // Both should fail
            assert_ne!(gnu.status.code(), Some(0));
            assert_ne!(ours.status.code(), Some(0));
        }
    }

    #[test]
    fn test_matches_gnu_error_missing_operand() {
        let gnu = Command::new("chroot").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().output().unwrap();
            // Both should have a non-zero exit code
            assert_ne!(gnu.status.code(), Some(0));
            assert_ne!(ours.status.code(), Some(0));
        }
    }

    #[test]
    fn test_matches_gnu_nonexistent() {
        let gnu = Command::new("chroot")
            .arg("/nonexistent_dir_xyz_999")
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("/nonexistent_dir_xyz_999").output().unwrap();
            assert_ne!(gnu.status.code(), Some(0));
            assert_ne!(ours.status.code(), Some(0));
        }
    }

    #[test]
    fn test_skip_chdir_accepted() {
        // Just verify the flag is accepted (will still fail without root)
        let output = cmd().args(["--skip-chdir", "/tmp"]).output().unwrap();
        assert_ne!(output.status.code(), Some(0));
    }

    #[test]
    fn test_userspec_accepted() {
        // Just verify the flag is parsed (will still fail without root)
        let output = cmd()
            .args(["--userspec=nobody:nogroup", "/tmp"])
            .output()
            .unwrap();
        assert_ne!(output.status.code(), Some(0));
    }

    #[test]
    fn test_groups_accepted() {
        // Just verify the flag is parsed (will still fail without root)
        let output = cmd().args(["--groups=0,1", "/tmp"]).output().unwrap();
        assert_ne!(output.status.code(), Some(0));
    }
}
