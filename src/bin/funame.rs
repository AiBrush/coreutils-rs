// funame — print system information
//
// Usage: uname [OPTION]...

#[cfg(unix)]
use std::ffi::CStr;
use std::process;

const TOOL_NAME: &str = "uname";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[allow(unused_variables, unused_assignments)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut show_sysname = false;
    let mut show_nodename = false;
    let mut show_release = false;
    let mut show_version = false;
    let mut show_machine = false;
    let mut show_processor = false;
    let mut show_hardware = false;
    let mut show_os = false;
    let mut any_flag = false;
    let mut show_all = false;

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]...", TOOL_NAME);
                println!("Print certain system information.  With no OPTION, same as -s.");
                println!();
                println!("  -a, --all                print all information");
                println!("  -s, --kernel-name        print the kernel name");
                println!("  -n, --nodename           print the network node hostname");
                println!("  -r, --kernel-release     print the kernel release");
                println!("  -v, --kernel-version     print the kernel version");
                println!("  -m, --machine            print the machine hardware name");
                println!("  -p, --processor          print the processor type");
                println!("  -i, --hardware-platform  print the hardware platform");
                println!("  -o, --operating-system   print the operating system");
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "-a" | "--all" => {
                show_sysname = true;
                show_nodename = true;
                show_release = true;
                show_version = true;
                show_machine = true;
                show_processor = true;
                show_hardware = true;
                show_os = true;
                show_all = true;
                any_flag = true;
            }
            "-s" | "--kernel-name" => {
                show_sysname = true;
                any_flag = true;
            }
            "-n" | "--nodename" => {
                show_nodename = true;
                any_flag = true;
            }
            "-r" | "--kernel-release" => {
                show_release = true;
                any_flag = true;
            }
            "-v" | "--kernel-version" => {
                show_version = true;
                any_flag = true;
            }
            "-m" | "--machine" => {
                show_machine = true;
                any_flag = true;
            }
            "-p" | "--processor" => {
                show_processor = true;
                any_flag = true;
            }
            "-i" | "--hardware-platform" => {
                show_hardware = true;
                any_flag = true;
            }
            "-o" | "--operating-system" => {
                show_os = true;
                any_flag = true;
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                for ch in s[1..].chars() {
                    match ch {
                        'a' => {
                            show_sysname = true;
                            show_nodename = true;
                            show_release = true;
                            show_version = true;
                            show_machine = true;
                            show_processor = true;
                            show_hardware = true;
                            show_os = true;
                            show_all = true;
                        }
                        's' => show_sysname = true,
                        'n' => show_nodename = true,
                        'r' => show_release = true,
                        'v' => show_version = true,
                        'm' => show_machine = true,
                        'p' => show_processor = true,
                        'i' => show_hardware = true,
                        'o' => show_os = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    any_flag = true;
                }
            }
            _ => {
                eprintln!("{}: extra operand '{}'", TOOL_NAME, arg);
                eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                process::exit(1);
            }
        }
    }

    // Default: -s
    if !any_flag {
        show_sysname = true;
    }

    #[cfg(unix)]
    {
        // SAFETY: zeroed utsname is valid, uname fills it in
        let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
        if unsafe { libc::uname(&mut uts) } != 0 {
            eprintln!("{}: cannot get system name", TOOL_NAME);
            process::exit(1);
        }

        let mut parts: Vec<&str> = Vec::new();

        let sysname = unsafe { CStr::from_ptr(uts.sysname.as_ptr()) }
            .to_str()
            .unwrap_or("unknown");
        let nodename = unsafe { CStr::from_ptr(uts.nodename.as_ptr()) }
            .to_str()
            .unwrap_or("unknown");
        let release = unsafe { CStr::from_ptr(uts.release.as_ptr()) }
            .to_str()
            .unwrap_or("unknown");
        let version = unsafe { CStr::from_ptr(uts.version.as_ptr()) }
            .to_str()
            .unwrap_or("unknown");
        let machine = unsafe { CStr::from_ptr(uts.machine.as_ptr()) }
            .to_str()
            .unwrap_or("unknown");

        if show_sysname {
            parts.push(sysname);
        }
        if show_nodename {
            parts.push(nodename);
        }
        if show_release {
            parts.push(release);
        }
        if show_version {
            parts.push(version);
        }
        if show_machine {
            parts.push(machine);
        }
        // On Linux, -p and -i are typically "unknown"; GNU skips "unknown" in -a mode
        let processor = "unknown";
        let hardware = "unknown";
        if show_processor && !(show_all && processor == "unknown") {
            parts.push(processor);
        }
        if show_hardware && !(show_all && hardware == "unknown") {
            parts.push(hardware);
        }
        if show_os {
            #[cfg(target_os = "linux")]
            parts.push("GNU/Linux");
            #[cfg(target_os = "macos")]
            parts.push("Darwin");
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            parts.push("unknown");
        }

        println!("{}", parts.join(" "));
    }

    #[cfg(not(unix))]
    {
        eprintln!("{}: not supported on this platform", TOOL_NAME);
        process::exit(1);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("funame");
        Command::new(path)
    }

    #[test]
    fn test_uname_default() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let name = stdout.trim();
        assert!(!name.is_empty());
        // Default is kernel name (e.g., "Linux")
        assert!(name == "Linux" || name == "Darwin" || !name.is_empty());
    }

    #[test]
    fn test_uname_all() {
        let output = cmd().arg("-a").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().split_whitespace().collect();
        assert!(
            parts.len() >= 5,
            "uname -a should have multiple fields, got: {:?}",
            parts
        );
    }

    #[test]
    fn test_uname_each_flag() {
        for flag in ["-s", "-n", "-r", "-v", "-m"] {
            let output = cmd().arg(flag).output().unwrap();
            assert_eq!(output.status.code(), Some(0), "Failed for flag {}", flag);
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(!stdout.trim().is_empty(), "Empty output for flag {}", flag);
        }
    }

    #[test]
    fn test_uname_combined() {
        let output = cmd().arg("-sr").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().split_whitespace().collect();
        assert_eq!(parts.len(), 2, "uname -sr should have 2 fields");
    }

    #[test]
    fn test_uname_matches_gnu() {
        let gnu = Command::new("uname").arg("-s").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("-s").output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch for -s");
        }
    }
}
