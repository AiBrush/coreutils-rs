// farch — print machine hardware name (equivalent to uname -m)

#[cfg(unix)]
use std::ffi::CStr;
use std::process;

const TOOL_NAME: &str = "arch";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(arg) = args.first() {
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]", TOOL_NAME);
                println!("Print machine architecture.");
                println!();
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            _ => {
                eprintln!("{}: extra operand '{}'", TOOL_NAME, arg);
                eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                process::exit(1);
            }
        }
    }

    #[cfg(unix)]
    {
        // SAFETY: zeroed utsname is valid, uname fills it in
        let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
        if unsafe { libc::uname(&mut uts) } != 0 {
            eprintln!("{}: cannot get system name", TOOL_NAME);
            process::exit(1);
        }
        // SAFETY: machine field is a null-terminated C string
        let machine = unsafe { CStr::from_ptr(uts.machine.as_ptr()) };
        println!("{}", machine.to_string_lossy());
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
        path.push("farch");
        Command::new(path)
    }

    #[test]
    fn test_arch_nonempty() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let arch = stdout.trim();
        assert!(!arch.is_empty(), "arch should output a non-empty string");
    }

    #[test]
    fn test_arch_matches_uname_m() {
        let uname_out = Command::new("uname").arg("-m").output();
        if let Ok(uname_out) = uname_out {
            let ours = cmd().output().unwrap();
            assert_eq!(ours.stdout, uname_out.stdout, "should match uname -m");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_arch_matches_gnu() {
        let gnu = Command::new("arch").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
