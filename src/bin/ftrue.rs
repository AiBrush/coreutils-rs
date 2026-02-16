// ftrue — exit with status 0
//
// GNU true ignores ALL arguments and always exits 0.

fn main() {
    // true always exits 0, ignoring all arguments
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop(); // remove test binary name
        path.pop(); // remove deps/
        path.push("ftrue");
        Command::new(path)
    }

    #[test]
    fn test_true_exit_code() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn test_true_ignores_args() {
        let output = cmd().args(["foo", "bar", "--baz"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
    }

    #[test]
    fn test_true_help_silent() {
        // GNU true ignores --help and still exits 0 silently
        let output = cmd().arg("--help").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn test_true_version_silent() {
        // GNU true ignores --version and still exits 0 silently
        let output = cmd().arg("--version").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn test_true_matches_gnu() {
        let gnu = Command::new("true").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
