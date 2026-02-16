// fgroups — print the groups a user is in
//
// Usage: groups [USERNAME]...

use std::ffi::CStr;
use std::process;

const TOOL_NAME: &str = "groups";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut users: Vec<String> = Vec::new();

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]... [USERNAME]...", TOOL_NAME);
                println!("Print group memberships for each USERNAME or, if no USERNAME is specified,");
                println!("for the current process.");
                println!();
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            _ => users.push(arg),
        }
    }

    if users.is_empty() {
        // Print groups for current user
        match get_current_groups() {
            Ok(groups) => println!("{}", groups.join(" ")),
            Err(e) => {
                eprintln!("{}: {}", TOOL_NAME, e);
                process::exit(1);
            }
        }
    } else {
        let mut exit_code = 0;
        for user in &users {
            match get_user_groups(user) {
                Ok(groups) => println!("{} : {}", user, groups.join(" ")),
                Err(e) => {
                    eprintln!("{}: '{}': {}", TOOL_NAME, user, e);
                    exit_code = 1;
                }
            }
        }
        if exit_code != 0 {
            process::exit(exit_code);
        }
    }
}

fn get_current_groups() -> Result<Vec<String>, String> {
    let ngroups = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    if ngroups < 0 {
        return Err("cannot get groups".to_string());
    }
    let mut gids = vec![0u32; ngroups as usize];
    let n = unsafe { libc::getgroups(ngroups, gids.as_mut_ptr()) };
    if n < 0 {
        return Err("cannot get groups".to_string());
    }
    gids.truncate(n as usize);

    // Also include the effective gid
    let egid = unsafe { libc::getegid() };
    if !gids.contains(&egid) {
        gids.insert(0, egid);
    }

    Ok(gids.iter().map(|&gid| gid_to_name(gid)).collect())
}

fn get_user_groups(user: &str) -> Result<Vec<String>, String> {
    let c_user = std::ffi::CString::new(user).map_err(|_| "invalid username".to_string())?;
    let pw = unsafe { libc::getpwnam(c_user.as_ptr()) };
    if pw.is_null() {
        return Err("no such user".to_string());
    }
    let pw_gid = unsafe { (*pw).pw_gid };

    // Get supplementary groups
    let mut ngroups: libc::c_int = 32;
    let mut gids: Vec<libc::gid_t> = vec![0; ngroups as usize];

    // SAFETY: pw is valid, gids has capacity ngroups
    let ret = unsafe {
        libc::getgrouplist(
            c_user.as_ptr(),
            pw_gid as libc::gid_t,
            gids.as_mut_ptr(),
            &mut ngroups,
        )
    };

    if ret == -1 {
        // Buffer too small, resize
        gids.resize(ngroups as usize, 0);
        unsafe {
            libc::getgrouplist(
                c_user.as_ptr(),
                pw_gid as libc::gid_t,
                gids.as_mut_ptr(),
                &mut ngroups,
            );
        }
    }

    gids.truncate(ngroups as usize);
    Ok(gids.iter().map(|&gid| gid_to_name(gid)).collect())
}

fn gid_to_name(gid: libc::gid_t) -> String {
    let gr = unsafe { libc::getgrgid(gid) };
    if gr.is_null() {
        return gid.to_string();
    }
    // SAFETY: getgrgid returned a valid pointer
    let name = unsafe { CStr::from_ptr((*gr).gr_name) };
    name.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fgroups");
        Command::new(path)
    }

    #[test]
    fn test_groups_current_user() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.trim().is_empty(), "Should list at least one group");
    }

    #[test]
    fn test_groups_specific_user() {
        let output = cmd().arg("root").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("root"), "root should be in a group containing 'root'");
    }

    #[test]
    fn test_groups_nonexistent_user() {
        let output = cmd().arg("nonexistent_user_12345").output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_groups_matches_gnu() {
        let gnu = Command::new("groups").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
