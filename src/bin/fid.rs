#[cfg(not(unix))]
fn main() {
    eprintln!("id: only available on Unix");
    std::process::exit(1);
}

// fid -- print real and effective user and group IDs
//
// Usage: id [OPTION]... [USER]

#[cfg(unix)]
use std::ffi::{CStr, CString};
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "id";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut flag_user = false;
    let mut flag_group = false;
    let mut flag_groups = false;
    let mut flag_name = false;
    let mut flag_real = false;
    let mut flag_zero = false;
    let mut username: Option<String> = None;
    let mut saw_dashdash = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if saw_dashdash {
            if username.is_some() {
                eprintln!("{}: extra operand '{}'", TOOL_NAME, arg);
                process::exit(1);
            }
            username = Some(arg.clone());
            i += 1;
            continue;
        }
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]... [USER]", TOOL_NAME);
                println!("Print user and group information for the specified USER,");
                println!("or (when USER omitted) for the current user.");
                println!();
                println!("  -u, --user    print only the effective user ID");
                println!("  -g, --group   print only the effective group ID");
                println!("  -G, --groups  print all group IDs");
                println!("  -n, --name    print a name instead of a number, for -ugG");
                println!("  -r, --real    print the real ID instead of the effective ID, for -ugG");
                println!("  -z, --zero    delimit entries with NUL characters, not whitespace");
                println!("  -a            ignore, for compatibility with other versions");
                println!("      --help    display this help and exit");
                println!("      --version output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--user" => flag_user = true,
            "--group" => flag_group = true,
            "--groups" => flag_groups = true,
            "--name" => flag_name = true,
            "--real" => flag_real = true,
            "--zero" => flag_zero = true,
            "--" => saw_dashdash = true,
            s if s.starts_with('-') && !s.starts_with("--") && s.len() > 1 => {
                for ch in s[1..].chars() {
                    match ch {
                        'u' => flag_user = true,
                        'g' => flag_group = true,
                        'G' => flag_groups = true,
                        'n' => flag_name = true,
                        'r' => flag_real = true,
                        'z' => flag_zero = true,
                        'a' => {} // ignored for compat
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            _ => {
                if username.is_some() {
                    eprintln!("{}: extra operand '{}'", TOOL_NAME, arg);
                    process::exit(1);
                }
                username = Some(arg.clone());
            }
        }
        i += 1;
    }

    // -n and -r only valid with -u, -g, or -G
    if (flag_name || flag_real) && !flag_user && !flag_group && !flag_groups {
        eprintln!(
            "{}: cannot print only names or real IDs in default format",
            TOOL_NAME
        );
        process::exit(1);
    }

    // Only one of -u, -g, -G may be specified
    let mode_count = flag_user as u8 + flag_group as u8 + flag_groups as u8;
    if mode_count > 1 {
        eprintln!(
            "{}: cannot print \"only\" of more than one choice",
            TOOL_NAME
        );
        process::exit(1);
    }

    let delim = if flag_zero { '\0' } else { '\n' };

    if let Some(ref name) = username {
        // Look up user by name
        let c_name = CString::new(name.as_str()).unwrap_or_else(|_| {
            eprintln!("{}: '{}': no such user", TOOL_NAME, name);
            process::exit(1);
        });
        let pw = unsafe { libc::getpwnam(c_name.as_ptr()) };
        if pw.is_null() {
            eprintln!("{}: '{}': no such user", TOOL_NAME, name);
            process::exit(1);
        }
        let uid = unsafe { (*pw).pw_uid };
        let gid = unsafe { (*pw).pw_gid };
        let groups = get_user_groups(&c_name, gid);

        if flag_user {
            print_id(uid, flag_name, delim);
        } else if flag_group {
            print_gid(gid, flag_name, delim);
        } else if flag_groups {
            print_groups(&groups, flag_name, flag_zero, delim);
        } else {
            print_default(uid, gid, &groups, delim);
        }
    } else {
        // Current user
        let (uid, gid) = if flag_real {
            (unsafe { libc::getuid() }, unsafe { libc::getgid() })
        } else {
            (unsafe { libc::geteuid() }, unsafe { libc::getegid() })
        };

        if flag_user {
            print_id(uid, flag_name, delim);
        } else if flag_group {
            print_gid(gid, flag_name, delim);
        } else if flag_groups {
            let groups = get_current_groups();
            print_groups(&groups, flag_name, flag_zero, delim);
        } else {
            let euid = unsafe { libc::geteuid() };
            let egid = unsafe { libc::getegid() };
            let groups = get_current_groups();
            print_default(euid, egid, &groups, delim);
        }
    }
}

#[cfg(unix)]
fn print_id(uid: libc::uid_t, name: bool, delim: char) {
    if name {
        print!("{}{}", uid_to_name(uid), delim);
    } else {
        print!("{}{}", uid, delim);
    }
}

#[cfg(unix)]
fn print_gid(gid: libc::gid_t, name: bool, delim: char) {
    if name {
        print!("{}{}", gid_to_name(gid), delim);
    } else {
        print!("{}{}", gid, delim);
    }
}

#[cfg(unix)]
fn print_groups(groups: &[libc::gid_t], name: bool, zero: bool, delim: char) {
    let sep = if zero { '\0' } else { ' ' };
    for (i, &gid) in groups.iter().enumerate() {
        if i > 0 {
            print!("{}", sep);
        }
        if name {
            print!("{}", gid_to_name(gid));
        } else {
            print!("{}", gid);
        }
    }
    print!("{}", delim);
}

#[cfg(unix)]
fn print_default(uid: libc::uid_t, gid: libc::gid_t, groups: &[libc::gid_t], delim: char) {
    print!("uid={}({})", uid, uid_to_name(uid));
    print!(" gid={}({})", gid, gid_to_name(gid));
    // Print groups
    print!(" groups=");
    for (i, &g) in groups.iter().enumerate() {
        if i > 0 {
            print!(",");
        }
        print!("{}({})", g, gid_to_name(g));
    }
    print!("{}", delim);
}

#[cfg(unix)]
fn uid_to_name(uid: libc::uid_t) -> String {
    let pw = unsafe { libc::getpwuid(uid) };
    if pw.is_null() {
        return uid.to_string();
    }
    let name = unsafe { CStr::from_ptr((*pw).pw_name) };
    name.to_string_lossy().into_owned()
}

#[cfg(unix)]
fn gid_to_name(gid: libc::gid_t) -> String {
    let gr = unsafe { libc::getgrgid(gid) };
    if gr.is_null() {
        return gid.to_string();
    }
    let name = unsafe { CStr::from_ptr((*gr).gr_name) };
    name.to_string_lossy().into_owned()
}

#[cfg(unix)]
fn get_user_groups(c_name: &CString, pw_gid: libc::gid_t) -> Vec<libc::gid_t> {
    let mut ngroups: libc::c_int = 64;

    // macOS getgrouplist uses c_int, Linux uses gid_t
    #[cfg(target_vendor = "apple")]
    {
        let mut gids: Vec<libc::c_int> = vec![0; ngroups as usize];
        let ret = unsafe {
            libc::getgrouplist(
                c_name.as_ptr(),
                pw_gid as libc::c_int,
                gids.as_mut_ptr(),
                &mut ngroups,
            )
        };
        if ret == -1 {
            gids.resize(ngroups as usize, 0);
            unsafe {
                libc::getgrouplist(
                    c_name.as_ptr(),
                    pw_gid as libc::c_int,
                    gids.as_mut_ptr(),
                    &mut ngroups,
                );
            }
        }
        gids.truncate(ngroups as usize);
        return gids.into_iter().map(|g| g as libc::gid_t).collect();
    }

    #[cfg(not(target_vendor = "apple"))]
    {
        let mut gids: Vec<libc::gid_t> = vec![0; ngroups as usize];
        let ret =
            unsafe { libc::getgrouplist(c_name.as_ptr(), pw_gid, gids.as_mut_ptr(), &mut ngroups) };
        if ret == -1 {
            gids.resize(ngroups as usize, 0);
            unsafe {
                libc::getgrouplist(c_name.as_ptr(), pw_gid, gids.as_mut_ptr(), &mut ngroups);
            }
        }
        gids.truncate(ngroups as usize);
        gids
    }
}

#[cfg(unix)]
fn get_current_groups() -> Vec<libc::gid_t> {
    let ngroups = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    if ngroups < 0 {
        return vec![unsafe { libc::getegid() }];
    }
    let mut gids = vec![0u32; ngroups as usize];
    let n = unsafe { libc::getgroups(ngroups, gids.as_mut_ptr()) };
    if n < 0 {
        return vec![unsafe { libc::getegid() }];
    }
    gids.truncate(n as usize);

    let egid = unsafe { libc::getegid() };
    // Ensure egid is at position 0, matching GNU behavior
    if let Some(pos) = gids.iter().position(|&g| g == egid) {
        if pos != 0 {
            gids.remove(pos);
            gids.insert(0, egid);
        }
    } else {
        gids.insert(0, egid);
    }
    gids
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fid");
        Command::new(path)
    }

    #[test]
    fn test_default_output() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("uid="), "Should contain uid=");
        assert!(stdout.contains("gid="), "Should contain gid=");
        assert!(stdout.contains("groups="), "Should contain groups=");
    }

    #[test]
    fn test_flag_u() {
        let output = cmd().arg("-u").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let _uid: u32 = stdout.trim().parse().expect("Should print a UID number");
    }

    #[test]
    fn test_flag_g() {
        let output = cmd().arg("-g").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let _gid: u32 = stdout.trim().parse().expect("Should print a GID number");
    }

    #[test]
    fn test_flag_big_g() {
        let output = cmd().arg("-G").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Should be space-separated numbers
        for part in stdout.trim().split(' ') {
            let _gid: u32 = part.parse().expect("Each group should be a number");
        }
    }

    #[test]
    fn test_flag_un() {
        let output = cmd().args(["-u", "-n"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let name = stdout.trim();
        assert!(!name.is_empty(), "Should print a username");
        // Should NOT be purely numeric (it's a name)
        assert!(
            name.parse::<u32>().is_err(),
            "Should print a name, not a number"
        );
    }

    #[test]
    fn test_flag_gn() {
        let output = cmd().args(["-g", "-n"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.trim().is_empty(), "Should print a group name");
    }

    #[test]
    fn test_flag_big_gn() {
        let output = cmd().args(["-G", "-n"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.trim().is_empty(), "Should print group names");
    }

    #[test]
    fn test_flag_z() {
        let output = cmd().args(["-G", "-z"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = &output.stdout;
        assert!(
            stdout.contains(&0u8),
            "Should contain NUL bytes as separators"
        );
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("(fcoreutils)"));
    }

    #[test]
    fn test_matches_gnu_default() {
        let gnu = Command::new("id").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }

    #[test]
    fn test_matches_gnu_u() {
        let gnu = Command::new("id").arg("-u").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("-u").output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch for -u");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }

    #[test]
    fn test_matches_gnu_g() {
        let gnu = Command::new("id").arg("-g").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("-g").output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch for -g");
        }
    }

    #[test]
    fn test_matches_gnu_big_g() {
        let gnu = Command::new("id").arg("-G").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("-G").output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch for -G");
        }
    }

    #[test]
    fn test_matches_gnu_un() {
        let gnu = Command::new("id").args(["-u", "-n"]).output();
        if let Ok(gnu) = gnu {
            let ours = cmd().args(["-u", "-n"]).output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch for -un");
        }
    }

    #[test]
    fn test_nonexistent_user() {
        let output = cmd().arg("nonexistent_user_xyz_99999").output().unwrap();
        assert_ne!(output.status.code(), Some(0));
    }
}
