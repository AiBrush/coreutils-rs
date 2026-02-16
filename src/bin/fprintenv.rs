// fprintenv — print all or part of environment
//
// Usage: printenv [OPTION]... [VARIABLE]...

use std::process;

const TOOL_NAME: &str = "printenv";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut null_terminated = false;
    let mut names: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    for arg in std::env::args().skip(1) {
        if saw_dashdash {
            names.push(arg);
            continue;
        }
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]... [VARIABLE]...", TOOL_NAME);
                println!("Print the values of the specified environment VARIABLE(s).");
                println!("If no VARIABLE is specified, print name and value pairs for them all.");
                println!();
                println!("  -0, --null    end each output line with NUL, not newline");
                println!("      --help    display this help and exit");
                println!("      --version output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--null" | "-0" => null_terminated = true,
            "--" => saw_dashdash = true,
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                for ch in s[1..].chars() {
                    match ch {
                        '0' => null_terminated = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(2);
                        }
                    }
                }
            }
            _ => names.push(arg),
        }
    }

    let terminator = if null_terminated { '\0' } else { '\n' };

    if names.is_empty() {
        // Print all environment variables
        for (key, value) in std::env::vars() {
            print!("{}={}{}", key, value, terminator);
        }
    } else {
        let mut exit_code = 0;
        for name in &names {
            match std::env::var(name) {
                Ok(val) => print!("{}{}", val, terminator),
                Err(_) => exit_code = 1,
            }
        }
        process::exit(exit_code);
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fprintenv");
        Command::new(path)
    }

    #[test]
    fn test_printenv_all() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Should contain at least PATH
        assert!(stdout.contains("PATH="), "Should contain PATH");
    }

    #[test]
    fn test_printenv_specific() {
        let output = cmd().arg("PATH").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.is_empty());
        // Should NOT contain "PATH=" prefix (just the value)
        assert!(!stdout.starts_with("PATH="), "Should print value only");
    }

    #[test]
    fn test_printenv_missing() {
        let output = cmd()
            .arg("DEFINITELY_NOT_A_REAL_ENV_VAR_12345")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn test_printenv_null_terminator() {
        let output = cmd().args(["-0", "PATH"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = &output.stdout;
        assert!(stdout.ends_with(&[0u8]), "Should end with NUL byte");
        assert!(!stdout.ends_with(b"\n"), "Should not end with newline");
    }

    #[test]
    #[cfg(unix)]
    fn test_printenv_matches_gnu() {
        let gnu = Command::new("printenv").arg("PATH").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("PATH").output().unwrap();
            assert_eq!(ours.stdout, gnu.stdout, "STDOUT mismatch");
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
