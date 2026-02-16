// ffalse — exit with status 1
//
// GNU false ignores ALL arguments and always exits 1.

fn main() {
    // false always exits 1, ignoring all arguments
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
    fn test_false_help_silent() {
        // GNU false ignores --help and still exits 1 silently
        let output = cmd().arg("--help").output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn test_false_version_silent() {
        // GNU false ignores --version and still exits 1 silently
        let output = cmd().arg("--version").output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
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
