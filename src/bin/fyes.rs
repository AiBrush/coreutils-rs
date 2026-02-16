// fyes — output a string repeatedly until killed
//
// Usage: yes [STRING]...
// Repeatedly output a line with all specified STRING(s), or 'y'.

use std::io::Write;
use std::process;

const TOOL_NAME: &str = "yes";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Buffer size for bulk writes (64 KiB).
const BUF_SIZE: usize = 64 * 1024;

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.len() == 1 {
        match args[0].as_str() {
            "--help" => {
                println!("Usage: {} [STRING]...", TOOL_NAME);
                println!("  or:  {} OPTION", TOOL_NAME);
                println!("Repeatedly output a line with all specified STRING(s), or 'y'.");
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

    let line = if args.is_empty() {
        "y\n".to_string()
    } else {
        let mut s = args.join(" ");
        s.push('\n');
        s
    };

    let line_bytes = line.as_bytes();

    // Build a large buffer filled with repeated copies of the line.
    let mut buf = Vec::with_capacity(BUF_SIZE + line_bytes.len());
    while buf.len() < BUF_SIZE {
        buf.extend_from_slice(line_bytes);
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    loop {
        if out.write_all(&buf).is_err() {
            // Broken pipe or other write error — exit silently.
            break;
        }
    }

    // Attempt flush; ignore errors (broken pipe).
    let _ = out.flush();
    process::exit(0);
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::process::{Command, Stdio};

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fyes");
        Command::new(path)
    }

    #[test]
    fn test_yes_default_y() {
        let mut child = cmd().stdout(Stdio::piped()).spawn().unwrap();

        let mut stdout = child.stdout.take().unwrap();
        let mut buf = Vec::new();
        // Read enough to get at least 5 lines
        let mut tmp = [0u8; 4096];
        while buf.len() < 10 {
            let n = stdout.read(&mut tmp).unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        drop(stdout);
        let _ = child.kill();
        let _ = child.wait();

        let text = String::from_utf8_lossy(&buf);
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.len() >= 5,
            "Expected at least 5 lines, got {}",
            lines.len()
        );
        for line in &lines[..5] {
            assert_eq!(*line, "y");
        }
    }

    #[test]
    fn test_yes_custom_string() {
        let mut child = cmd().arg("hello").stdout(Stdio::piped()).spawn().unwrap();

        let mut stdout = child.stdout.take().unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        while buf.len() < 20 {
            let n = stdout.read(&mut tmp).unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        drop(stdout);
        let _ = child.kill();
        let _ = child.wait();

        let text = String::from_utf8_lossy(&buf);
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.len() >= 3,
            "Expected at least 3 lines, got {}",
            lines.len()
        );
        for line in &lines[..3] {
            assert_eq!(*line, "hello");
        }
    }

    #[test]
    fn test_yes_multiple_args() {
        let mut child = cmd()
            .args(["a", "b"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();

        let mut stdout = child.stdout.take().unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        while buf.len() < 20 {
            let n = stdout.read(&mut tmp).unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        drop(stdout);
        let _ = child.kill();
        let _ = child.wait();

        let text = String::from_utf8_lossy(&buf);
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.len() >= 2,
            "Expected at least 2 lines, got {}",
            lines.len()
        );
        for line in &lines[..2] {
            assert_eq!(*line, "a b");
        }
    }

    #[test]
    fn test_yes_pipe_closes() {
        // yes piped to head should terminate
        let child = cmd().stdout(Stdio::piped()).spawn().unwrap();

        let head = Command::new("head")
            .arg("-n")
            .arg("1")
            .stdin(child.stdout.unwrap())
            .stdout(Stdio::piped())
            .output()
            .unwrap();

        assert_eq!(head.status.code(), Some(0));
        let text = String::from_utf8_lossy(&head.stdout);
        assert_eq!(text.trim(), "y");
    }

    #[test]
    #[cfg(unix)]
    fn test_yes_matches_gnu() {
        // Compare first 1000 lines with GNU yes
        let gnu = Command::new("sh")
            .args(["-c", "yes | head -n 1000"])
            .output();
        if let Ok(gnu) = gnu {
            let ours = Command::new("sh")
                .args([
                    "-c",
                    &format!("{} | head -n 1000", cmd().get_program().to_str().unwrap()),
                ])
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&ours.stdout),
                String::from_utf8_lossy(&gnu.stdout),
                "Output mismatch with GNU yes"
            );
        }
    }
}
