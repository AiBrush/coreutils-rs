#[cfg(not(unix))]
fn main() {
    eprintln!("link: only available on Unix");
    std::process::exit(1);
}

// flink — create a hard link (call the link function)
//
// Usage: link FILE1 FILE2
// Create a hard link named FILE2 to FILE1.

#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "link";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.len() == 1 {
        match args[0].as_str() {
            "--help" => {
                println!("Usage: {} FILE1 FILE2", TOOL_NAME);
                println!("  or:  {} OPTION", TOOL_NAME);
                println!("Call the link function to create a link named FILE2 to existing FILE1.");
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

    if args.len() != 2 {
        eprintln!(
            "{}: missing operand{}",
            TOOL_NAME,
            if args.is_empty() {
                ""
            } else if args.len() == 1 {
                " after the first argument"
            } else {
                ""
            }
        );
        if args.len() > 2 {
            eprintln!("{}: extra operand '{}'", TOOL_NAME, args[2]);
        }
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    if let Err(e) = std::fs::hard_link(&args[0], &args[1]) {
        eprintln!(
            "{}: cannot create link '{}' to '{}': {}",
            TOOL_NAME,
            args[1],
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
        path.push("flink");
        Command::new(path)
    }

    #[test]
    fn test_link_creates_hardlink() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.txt");
        let dst = dir.path().join("dest.txt");
        fs::write(&src, "hello").unwrap();

        let output = cmd()
            .arg(src.to_str().unwrap())
            .arg(dst.to_str().unwrap())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(dst.exists());

        // Verify same inode (hard link)
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let src_meta = fs::metadata(&src).unwrap();
            let dst_meta = fs::metadata(&dst).unwrap();
            assert_eq!(src_meta.ino(), dst_meta.ino());
            assert_eq!(src_meta.nlink(), 2);
        }
    }

    #[test]
    fn test_link_wrong_arg_count_zero() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_link_wrong_arg_count_one() {
        let output = cmd().arg("file1").output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_link_wrong_arg_count_three() {
        let output = cmd().args(["a", "b", "c"]).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_link_nonexistent_source() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dest.txt");
        let output = cmd()
            .arg("/nonexistent_file_12345")
            .arg(dst.to_str().unwrap())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("cannot create link"));
    }

    #[test]
    fn test_link_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.txt");
        let dst = dir.path().join("dest.txt");
        fs::write(&src, "hello").unwrap();
        fs::write(&dst, "existing").unwrap();

        let output = cmd()
            .arg(src.to_str().unwrap())
            .arg(dst.to_str().unwrap())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_link_matches_gnu() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("gnu_src.txt");
        fs::write(&src, "test").unwrap();

        // Test with nonexistent source — both should fail
        let gnu = Command::new("link")
            .arg("/nonexistent_link_test")
            .arg(dir.path().join("out").to_str().unwrap())
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd()
                .arg("/nonexistent_link_test")
                .arg(dir.path().join("out2").to_str().unwrap())
                .output()
                .unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
