// fsync — synchronize cached writes to persistent storage
//
// Usage: sync [OPTION] [FILE]...
// Flush all or specified filesystems/files.

use std::process;

const TOOL_NAME: &str = "sync";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut data_only = false;
    let mut file_system = false;
    let mut files: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION] [FILE]...", TOOL_NAME);
                println!("Force changed blocks to disk, update the super block.");
                println!();
                println!("  -d, --data             sync only file data, no unneeded metadata");
                println!("  -f, --file-system      sync the file systems that contain the files");
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                println!();
                println!("With no FILE, sync all file systems.");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--data" | "-d" => data_only = true,
            "--file-system" | "-f" => file_system = true,
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                // Parse combined short flags like -df
                for ch in s[1..].chars() {
                    match ch {
                        'd' => data_only = true,
                        'f' => file_system = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            "--" => {
                // Remaining args are files
                for remaining in &mut args {
                    files.push(remaining);
                }
                break;
            }
            _ => files.push(arg),
        }
    }

    if files.is_empty() {
        if data_only || file_system {
            eprintln!(
                "{}: --data and --file-system require at least one FILE argument",
                TOOL_NAME
            );
            process::exit(1);
        }
        // sync all filesystems
        #[cfg(unix)]
        unsafe {
            libc::sync();
        }
    } else {
        let mut exit_code = 0;
        for file in &files {
            match sync_file(file, data_only, file_system) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!(
                        "{}: error syncing '{}': {}",
                        TOOL_NAME,
                        file,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                    exit_code = 1;
                }
            }
        }
        if exit_code != 0 {
            process::exit(exit_code);
        }
    }
}

fn sync_file(path: &str, data_only: bool, file_system: bool) -> std::io::Result<()> {
    use std::fs::File;
    #[cfg(unix)]
    use std::os::unix::io::AsRawFd;

    let file = File::open(path)?;

    #[cfg(unix)]
    {
        let fd = file.as_raw_fd();
        let ret = if file_system {
            // syncfs — sync the filesystem containing this file
            // syncfs is Linux-specific; fall back to fsync on other Unix
            #[cfg(target_os = "linux")]
            {
                unsafe { libc::syncfs(fd) }
            }
            #[cfg(not(target_os = "linux"))]
            {
                unsafe { libc::fsync(fd) }
            }
        } else if data_only {
            // fdatasync — sync data only, skip metadata
            // fdatasync is Linux-specific; fall back to fsync on other Unix
            #[cfg(target_os = "linux")]
            {
                unsafe { libc::fdatasync(fd) }
            }
            #[cfg(not(target_os = "linux"))]
            {
                unsafe { libc::fsync(fd) }
            }
        } else {
            // fsync — sync data + metadata
            unsafe { libc::fsync(fd) }
        };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (data_only, file_system);
        file.sync_all()?;
    }

    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fsync");
        Command::new(path)
    }

    #[test]
    fn test_sync_no_args() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
    }

    #[test]
    fn test_sync_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("testfile.txt");
        fs::write(&file, "hello").unwrap();

        // -f (filesystem sync)
        let output = cmd().args(["-f", file.to_str().unwrap()]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
    }

    #[test]
    fn test_sync_data() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("testfile.txt");
        fs::write(&file, "hello").unwrap();

        let output = cmd().args(["-d", file.to_str().unwrap()]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
    }

    #[test]
    fn test_sync_nonexistent_file() {
        let output = cmd()
            .args(["-f", "/nonexistent_sync_test_file"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_sync_exit_codes() {
        // No args = success
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));

        // Nonexistent file = failure
        let output = cmd()
            .args(["-d", "/nonexistent_sync_test"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_sync_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("sync"));
    }
}
