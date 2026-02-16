#[cfg(not(unix))]
fn main() {
    eprintln!("nproc: only available on Unix");
    std::process::exit(1);
}

// fnproc — print the number of processing units available
//
// By default, prints the number of processing units available to the current
// process (respects cgroups, OMP_NUM_THREADS, etc.). With --all, prints the
// number of installed processors.

#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "nproc";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut all = false;
    let mut ignore: usize = 0;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]...", TOOL_NAME);
                println!("Print the number of processing units available to the current process,");
                println!("which may be less than the number of online processors.");
                println!();
                println!("      --all        print the number of installed processors");
                println!("      --ignore=N   if possible, exclude N processing units");
                println!("      --help       display this help and exit");
                println!("      --version    output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--all" => all = true,
            s if s.starts_with("--ignore=") => {
                let val = &s["--ignore=".len()..];
                ignore = val.parse().unwrap_or_else(|_| {
                    eprintln!("{}: invalid number: '{}'", TOOL_NAME, val);
                    process::exit(1);
                });
            }
            "--ignore" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option '--ignore' requires an argument", TOOL_NAME);
                    process::exit(1);
                }
                ignore = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("{}: invalid number: '{}'", TOOL_NAME, args[i]);
                    process::exit(1);
                });
            }
            _ => {
                eprintln!("{}: unrecognized option '{}'", TOOL_NAME, arg);
                eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                process::exit(1);
            }
        }
        i += 1;
    }

    let n = if all {
        get_nprocs_conf()
    } else {
        get_nprocs_available()
    };

    // Floor at 1 — never report 0 processors
    let result = n.saturating_sub(ignore).max(1);
    println!("{}", result);
}

#[cfg(unix)]
fn get_nprocs_available() -> usize {
    // First check OMP_NUM_THREADS (GNU nproc respects this)
    if let Ok(val) = std::env::var("OMP_NUM_THREADS") {
        // OMP_NUM_THREADS can be comma-separated list; use first value
        if let Some(n) = val
            .split(',')
            .next()
            .and_then(|first| first.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
        {
            return n;
        }
    }

    // Use std::thread::available_parallelism which respects cgroups on Linux
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

#[cfg(unix)]
fn get_nprocs_conf() -> usize {
    // _SC_NPROCESSORS_CONF: number of configured (installed) processors
    #[cfg(unix)]
    {
        let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
        if n > 0 {
            return n as usize;
        }
    }
    // Fallback
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fnproc");
        Command::new(path)
    }

    #[test]
    fn test_nproc_positive_number() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let n: usize = stdout.trim().parse().unwrap();
        assert!(n > 0, "nproc should return at least 1");
    }

    #[test]
    fn test_nproc_all() {
        let default_out = cmd().output().unwrap();
        let all_out = cmd().arg("--all").output().unwrap();
        assert_eq!(all_out.status.code(), Some(0));

        let default_n: usize = String::from_utf8_lossy(&default_out.stdout)
            .trim()
            .parse()
            .unwrap();
        let all_n: usize = String::from_utf8_lossy(&all_out.stdout)
            .trim()
            .parse()
            .unwrap();
        assert!(
            all_n >= default_n,
            "--all ({}) should be >= default ({})",
            all_n,
            default_n
        );
    }

    #[test]
    fn test_nproc_ignore() {
        let output = cmd().arg("--ignore=1").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let n: usize = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap();
        assert!(n >= 1, "nproc with --ignore should be at least 1");
    }

    #[test]
    fn test_nproc_ignore_more_than_avail() {
        let output = cmd().arg("--ignore=99999").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let n: usize = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap();
        assert_eq!(n, 1, "nproc --ignore=99999 should floor at 1");
    }

    #[test]
    fn test_nproc_matches_gnu() {
        let gnu = Command::new("nproc").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
