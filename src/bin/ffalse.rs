// ffalse — exit with status 1
//
// GNU false accepts and ignores all arguments except --help and --version.
// --help and --version exit 0; all other invocations exit 1.

const TOOL_NAME: &str = "false";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 {
        match args[0].as_str() {
            "--help" => {
                println!("Usage: {} [ignored command line arguments]", TOOL_NAME);
                println!("  or:  {} OPTION", TOOL_NAME);
                println!("Exit with a status code indicating failure.");
                println!();
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                // GNU false --help exits 0
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                // GNU false --version exits 0
                return;
            }
            _ => {}
        }
    }
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("ffalse");
        Command::new(path)
    }

    #[test]
    fn test_false_exit_code() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn test_false_ignores_args() {
        let output = cmd().args(["foo", "bar", "--baz"]).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_false_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Exit with a status code indicating failure"));
    }

    #[test]
    fn test_false_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("false"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_false_matches_gnu() {
        let gnu = Command::new("false").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
