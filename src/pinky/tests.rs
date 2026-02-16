use super::*;
use std::process::Command;

fn bin_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("fpinky");
    path
}

fn cmd() -> Command {
    Command::new(bin_path())
}

// ---- Integration tests ----

#[test]
fn test_pinky_runs() {
    let output = cmd().output().unwrap();
    // pinky should succeed even with no logged-in users
    assert!(
        output.status.success(),
        "fpinky should exit with code 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_pinky_short() {
    // Default short format should include a heading
    let output = cmd().output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // If there are any logged-in users, heading should be present
    if !stdout.trim().is_empty() {
        assert!(
            stdout.contains("Login"),
            "Short format heading should contain 'Login', got: {}",
            stdout
        );
    }
}

#[test]
fn test_pinky_long() {
    // Long format with -l; needs a username
    // Get current username for testing
    let whoami = Command::new("whoami").output();
    if let Ok(whoami) = whoami {
        if whoami.status.success() {
            let username = String::from_utf8_lossy(&whoami.stdout).trim().to_string();
            if !username.is_empty() {
                let output = cmd().args(["-l", &username]).output().unwrap();
                assert!(output.status.success());
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(
                    stdout.contains("Login name:"),
                    "Long format should contain 'Login name:', got: {}",
                    stdout
                );
            }
        }
    }
}

#[test]
fn test_pinky_specific_user() {
    // Look up current user
    let whoami = Command::new("whoami").output();
    if let Ok(whoami) = whoami {
        if whoami.status.success() {
            let username = String::from_utf8_lossy(&whoami.stdout).trim().to_string();
            if !username.is_empty() {
                let output = cmd().arg(&username).output().unwrap();
                assert!(
                    output.status.success(),
                    "pinky should succeed for specific user"
                );
            }
        }
    }
}

#[test]
fn test_pinky_matches_gnu_format() {
    let gnu = Command::new("pinky").output();
    if let Ok(gnu) = gnu {
        let ours = cmd().output().unwrap();
        assert_eq!(
            ours.status.code(),
            gnu.status.code(),
            "Exit code mismatch: ours={:?} gnu={:?}",
            ours.status.code(),
            gnu.status.code()
        );
        // Both should have the same number of output lines
        let gnu_lines = String::from_utf8_lossy(&gnu.stdout).lines().count();
        let our_lines = String::from_utf8_lossy(&ours.stdout).lines().count();
        assert_eq!(
            our_lines, gnu_lines,
            "Line count mismatch: ours={} gnu={}",
            our_lines, gnu_lines
        );
    }
}

// ---- Unit tests for internal functions ----

#[test]
fn test_pinky_config_default() {
    let config = PinkyConfig::default();
    assert!(config.short_format);
    assert!(!config.long_format);
    assert!(!config.omit_heading);
    assert!(!config.omit_fullname);
    assert!(config.users.is_empty());
}

#[test]
fn test_format_short_heading_default() {
    let config = PinkyConfig::default();
    let heading = format_short_heading(&config);
    assert!(heading.contains("Login"));
    assert!(heading.contains("Name"));
    assert!(heading.contains("TTY"));
    assert!(heading.contains("Idle"));
    assert!(heading.contains("When"));
    assert!(heading.contains("Where"));
}

#[test]
fn test_format_short_heading_omit_fullname() {
    let config = PinkyConfig {
        omit_fullname: true,
        ..PinkyConfig::default()
    };
    let heading = format_short_heading(&config);
    assert!(heading.contains("Login"));
    assert!(!heading.contains("Name"));
    assert!(heading.contains("TTY"));
}

#[test]
fn test_format_short_entry_basic() {
    let entry = crate::who::UtmpxEntry {
        ut_type: 7, // USER_PROCESS
        ut_pid: 1234,
        ut_line: "pts/0".to_string(),
        ut_id: "ts/0".to_string(),
        ut_user: "testuser".to_string(),
        ut_host: "10.0.0.1".to_string(),
        ut_tv_sec: 1_704_067_200,
    };
    let config = PinkyConfig::default();
    let line = format_short_entry(&entry, &config);
    assert!(line.contains("testuser"));
    assert!(line.contains("pts/0"));
    assert!(line.contains("10.0.0.1"));
}

#[test]
fn test_format_long_entry_basic() {
    // Test with current user (guaranteed to exist in passwd)
    let whoami_output = Command::new("whoami").output();
    if let Ok(whoami) = whoami_output {
        if whoami.status.success() {
            let username = String::from_utf8_lossy(&whoami.stdout).trim().to_string();
            if !username.is_empty() {
                let config = PinkyConfig::default();
                let entry = format_long_entry(&username, &config);
                assert!(
                    entry.contains("Login name:"),
                    "Long entry should contain 'Login name:', got: {}",
                    entry
                );
                assert!(
                    entry.contains(&username),
                    "Long entry should contain the username"
                );
            }
        }
    }
}

#[test]
fn test_format_long_entry_omit_home_shell() {
    let config = PinkyConfig {
        omit_home_shell: true,
        omit_project: true,
        omit_plan: true,
        ..PinkyConfig::default()
    };
    let whoami_output = Command::new("whoami").output();
    if let Ok(whoami) = whoami_output {
        if whoami.status.success() {
            let username = String::from_utf8_lossy(&whoami.stdout).trim().to_string();
            if !username.is_empty() {
                let entry = format_long_entry(&username, &config);
                assert!(entry.contains("Login name:"));
                // With -b, should not contain "Directory:" label with actual dir path
                // (it's omitted entirely)
                assert!(
                    !entry.contains("Directory:"),
                    "With -b, should omit directory line"
                );
            }
        }
    }
}

#[test]
fn test_get_user_info_current_user() {
    let whoami_output = Command::new("whoami").output();
    if let Ok(whoami) = whoami_output {
        if whoami.status.success() {
            let username = String::from_utf8_lossy(&whoami.stdout).trim().to_string();
            if !username.is_empty() {
                let info = get_user_info(&username);
                assert!(info.is_some(), "Should find current user in passwd");
                let info = info.unwrap();
                assert_eq!(info.login, username);
                assert!(!info.home_dir.is_empty(), "Home dir should not be empty");
                assert!(!info.shell.is_empty(), "Shell should not be empty");
            }
        }
    }
}

#[test]
fn test_get_user_info_nonexistent() {
    let info = get_user_info("this_user_definitely_does_not_exist_12345");
    assert!(info.is_none(), "Should not find nonexistent user");
}
