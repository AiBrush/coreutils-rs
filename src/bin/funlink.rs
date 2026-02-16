// funlink — call the unlink function to remove the specified file
//
// Usage: unlink FILE

use std::process;

const TOOL_NAME: &str = "unlink";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.len() == 1 {
        match args[0].as_str() {
            "--help" => {
                println!("Usage: {} FILE", TOOL_NAME);
                println!("  or:  {} OPTION", TOOL_NAME);
                println!("Call the unlink function to remove the specified FILE.");
                println!();
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            _ => {}
        }
    }

    if args.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    if args.len() > 1 {
        eprintln!("{}: extra operand '{}'", TOOL_NAME, args[1]);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    if let Err(e) = std::fs::remove_file(&args[0]) {
        eprintln!(
            "{}: cannot unlink '{}': {}",
            TOOL_NAME,
            args[0],
            coreutils_rs::common::io_error_msg(&e)
        );
        process::exit(1);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("funlink");
        Command::new(path)
    }

    #[test]
    fn test_unlink_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("testfile.txt");
        fs::write(&file, "hello").unwrap();
        assert!(file.exists());

        let output = cmd().arg(file.to_str().unwrap()).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(!file.exists());
    }

    #[test]
    fn test_unlink_nonexistent() {
        let output = cmd().arg("/nonexistent_file_67890").output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("cannot unlink"));
    }

    #[test]
    fn test_unlink_directory_fails() {
        let dir = tempfile::tempdir().unwrap();
        let output = cmd().arg(dir.path().to_str().unwrap()).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_unlink_wrong_arg_count_zero() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("missing operand"));
    }

    #[test]
    fn test_unlink_wrong_arg_count_two() {
        let output = cmd().args(["a", "b"]).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("extra operand"));
    }

    #[test]
    fn test_unlink_matches_gnu() {
        // Test nonexistent file — both should exit 1
        let gnu = Command::new("unlink")
            .arg("/nonexistent_unlink_test_file")
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("/nonexistent_unlink_test_file").output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
