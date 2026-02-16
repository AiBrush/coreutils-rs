use super::*;
use std::process::Command;

fn bin_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("fwho");
    path
}

fn cmd() -> Command {
    Command::new(bin_path())
}

// ---- Unit tests ----

#[test]
fn test_who_runs() {
    let output = cmd().output().unwrap();
    // who should succeed even with no logged-in users
    assert!(
        output.status.success(),
        "fwho should exit with code 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_who_heading() {
    let output = cmd().arg("-H").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Heading line should contain NAME and LINE
    assert!(stdout.contains("NAME"), "Heading should contain NAME");
    assert!(stdout.contains("LINE"), "Heading should contain LINE");
    assert!(stdout.contains("TIME"), "Heading should contain TIME");
}

#[test]
fn test_who_count() {
    let output = cmd().arg("-q").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Count mode should show "# users=N"
    assert!(
        stdout.contains("# users="),
        "Count mode should show '# users=N', got: {}",
        stdout
    );
}

#[test]
fn test_who_boot() {
    let output = cmd().arg("-b").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Boot output may or may not have "system boot" depending on utmpx data
    // On systems with boot record, verify it contains "system boot"
    if !stdout.trim().is_empty() {
        assert!(
            stdout.contains("system boot"),
            "Boot output should contain 'system boot', got: {}",
            stdout
        );
    }
}

#[test]
fn test_who_format_check() {
    // Verify that regular who output lines have reasonable formatting
    let output = cmd().output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Each line should have at least a name and a timestamp portion
        // Timestamps match YYYY-MM-DD HH:MM
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert!(
            parts.len() >= 3,
            "Output line should have at least 3 fields: '{}'",
            line
        );
    }
}

#[test]
fn test_who_matches_gnu_format() {
    let gnu = Command::new("who").output();
    if let Ok(gnu) = gnu {
        let ours = cmd().output().unwrap();
        assert_eq!(
            ours.status.code(),
            gnu.status.code(),
            "Exit code mismatch: ours={:?} gnu={:?}",
            ours.status.code(),
            gnu.status.code()
        );
        // Both should have the same number of output lines (same logged-in users)
        let gnu_lines = String::from_utf8_lossy(&gnu.stdout)
            .lines()
            .count();
        let our_lines = String::from_utf8_lossy(&ours.stdout)
            .lines()
            .count();
        assert_eq!(
            our_lines, gnu_lines,
            "Line count mismatch: ours={} gnu={}",
            our_lines, gnu_lines
        );
    }
}

// ---- Unit tests for internal functions ----

#[test]
fn test_format_time_zero() {
    let result = format_time(0);
    assert!(result.is_empty(), "format_time(0) should return empty string");
}

#[test]
fn test_format_time_nonzero() {
    // 2024-01-01 00:00:00 UTC = 1704067200
    let result = format_time(1_704_067_200);
    assert!(!result.is_empty());
    // Should match YYYY-MM-DD HH:MM pattern
    assert!(
        result.len() >= 16,
        "Time string should be at least 16 chars: '{}'",
        result
    );
    assert!(result.contains('-'), "Time should contain date separators");
    assert!(result.contains(':'), "Time should contain time separator");
}

#[test]
fn test_who_config_default_filter() {
    let config = WhoConfig::default();
    assert!(config.is_default_filter());
}

#[test]
fn test_who_config_apply_all() {
    let mut config = WhoConfig::default();
    config.apply_all();
    assert!(config.show_boot);
    assert!(config.show_dead);
    assert!(config.show_login);
    assert!(config.show_init_spawn);
    assert!(config.show_runlevel);
    assert!(config.show_clock_change);
    assert!(config.show_mesg);
    assert!(config.show_users);
    assert!(!config.is_default_filter());
}

#[test]
fn test_format_heading_basic() {
    let config = WhoConfig::default();
    let heading = format_heading(&config);
    assert!(heading.contains("NAME"));
    assert!(heading.contains("LINE"));
    assert!(heading.contains("TIME"));
}

#[test]
fn test_format_heading_with_mesg() {
    let config = WhoConfig {
        show_mesg: true,
        ..WhoConfig::default()
    };
    let heading = format_heading(&config);
    assert!(heading.contains("NAME"));
    assert!(heading.contains("LINE"));
    assert!(heading.contains("TIME"));
}

#[test]
fn test_format_entry_user_process() {
    let entry = UtmpxEntry {
        ut_type: 7, // USER_PROCESS
        ut_pid: 1234,
        ut_line: "pts/0".to_string(),
        ut_id: "ts/0".to_string(),
        ut_user: "testuser".to_string(),
        ut_host: "10.0.0.1".to_string(),
        ut_tv_sec: 1_704_067_200,
    };
    let config = WhoConfig::default();
    let line = format_entry(&entry, &config);
    assert!(line.contains("testuser"));
    assert!(line.contains("pts/0"));
    assert!(line.contains("10.0.0.1"));
}

#[test]
fn test_format_entry_boot_time() {
    let entry = UtmpxEntry {
        ut_type: 2, // BOOT_TIME
        ut_pid: 0,
        ut_line: "~".to_string(),
        ut_id: "~~".to_string(),
        ut_user: "reboot".to_string(),
        ut_host: String::new(),
        ut_tv_sec: 1_704_067_200,
    };
    let config = WhoConfig {
        show_boot: true,
        ..WhoConfig::default()
    };
    let line = format_entry(&entry, &config);
    assert!(
        line.contains("system boot"),
        "Boot entry should show 'system boot', got: {}",
        line
    );
}

#[test]
fn test_format_count_empty() {
    let entries: Vec<UtmpxEntry> = Vec::new();
    let result = format_count(&entries);
    assert!(result.contains("# users=0"));
}

#[test]
fn test_format_count_with_users() {
    let entries = vec![
        UtmpxEntry {
            ut_type: 7,
            ut_pid: 100,
            ut_line: "pts/0".to_string(),
            ut_id: "ts/0".to_string(),
            ut_user: "alice".to_string(),
            ut_host: String::new(),
            ut_tv_sec: 1_704_067_200,
        },
        UtmpxEntry {
            ut_type: 7,
            ut_pid: 200,
            ut_line: "pts/1".to_string(),
            ut_id: "ts/1".to_string(),
            ut_user: "bob".to_string(),
            ut_host: String::new(),
            ut_tv_sec: 1_704_067_200,
        },
    ];
    let result = format_count(&entries);
    assert!(result.contains("alice"));
    assert!(result.contains("bob"));
    assert!(result.contains("# users=2"));
}

#[test]
fn test_read_utmpx_returns_vec() {
    // read_utmpx should not panic and should return a vec
    let entries = read_utmpx();
    // We can't assert specific content, but we can check it's a valid vec
    let _ = entries.len();
}
