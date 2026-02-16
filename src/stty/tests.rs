use std::process::{Command, Stdio};

fn cmd() -> Command {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("fstty");
    Command::new(path)
}

#[test]
fn test_stty_runs() {
    // Running with --help should always succeed regardless of tty
    let output = cmd().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("stty"));
}

#[test]
fn test_stty_version() {
    let output = cmd().arg("--version").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("stty"));
    assert!(stdout.contains("fcoreutils"));
}

#[test]
fn test_stty_size_format() {
    // When stdin is a pipe (not a tty), stty size should fail
    let output = cmd()
        .arg("size")
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    // Should exit with non-zero when not a tty
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a tty") || stderr.contains("Inappropriate ioctl"),
        "Expected tty error, got: {}",
        stderr
    );
}

#[test]
fn test_stty_all_format() {
    // When stdin is a pipe, stty -a should fail with not-a-tty error
    let output = cmd()
        .arg("-a")
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a tty") || stderr.contains("Inappropriate ioctl"),
        "Expected tty error, got: {}",
        stderr
    );
}

#[test]
fn test_stty_speed() {
    // When stdin is a pipe, stty speed should fail
    let output = cmd()
        .arg("speed")
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a tty") || stderr.contains("Inappropriate ioctl"),
        "Expected tty error, got: {}",
        stderr
    );
}

#[test]
fn test_stty_matches_gnu_errors() {
    // Both GNU stty and our stty should fail when stdin is not a tty
    let gnu = Command::new("stty")
        .arg("size")
        .stdin(Stdio::piped())
        .output();
    if let Ok(gnu) = gnu {
        let ours = cmd()
            .arg("size")
            .stdin(Stdio::piped())
            .output()
            .unwrap();
        assert_eq!(
            ours.status.success(),
            gnu.status.success(),
            "Exit status mismatch with GNU stty"
        );
    }
}

// Unit tests for helper functions
use crate::stty::{
    apply_flag, apply_settings, baud_to_num, format_cc, num_to_baud,
    parse_control_char, set_cooked, set_raw, set_sane,
};

#[test]
fn test_baud_to_num_known() {
    assert_eq!(baud_to_num(libc::B9600), 9600);
    assert_eq!(baud_to_num(libc::B115200), 115200);
    assert_eq!(baud_to_num(libc::B0), 0);
}

#[test]
fn test_num_to_baud_known() {
    assert_eq!(num_to_baud(9600), Some(libc::B9600));
    assert_eq!(num_to_baud(115200), Some(libc::B115200));
    assert_eq!(num_to_baud(12345), None);
}

#[test]
fn test_baud_roundtrip() {
    for &rate in &[0, 50, 75, 110, 300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200] {
        let baud = num_to_baud(rate).unwrap();
        assert_eq!(baud_to_num(baud), rate);
    }
}

#[test]
fn test_parse_control_char_caret_c() {
    assert_eq!(parse_control_char("^C"), Some(0x03));
}

#[test]
fn test_parse_control_char_caret_question() {
    assert_eq!(parse_control_char("^?"), Some(0x7f));
}

#[test]
fn test_parse_control_char_undef() {
    assert_eq!(parse_control_char("^-"), Some(0));
    assert_eq!(parse_control_char("undef"), Some(0));
}

#[test]
fn test_parse_control_char_caret_at() {
    // ^@ = NUL = 0
    assert_eq!(parse_control_char("^@"), Some(0));
}

#[test]
fn test_parse_control_char_lowercase() {
    // ^c should also work
    assert_eq!(parse_control_char("^c"), Some(0x03));
}

#[test]
fn test_parse_control_char_literal() {
    assert_eq!(parse_control_char("x"), Some(b'x'));
}

#[test]
fn test_format_cc_ctrl_c() {
    assert_eq!(format_cc(0x03), "^C");
}

#[test]
fn test_format_cc_delete() {
    assert_eq!(format_cc(0x7f), "^?");
}

#[test]
fn test_format_cc_undef() {
    assert_eq!(format_cc(0), "<undef>");
}

#[test]
fn test_format_cc_printable() {
    assert_eq!(format_cc(b'x'), "x");
}

#[test]
fn test_set_sane_resets_flags() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    set_sane(&mut termios);
    // Check some expected flags
    assert_ne!(termios.c_iflag & libc::ICRNL, 0);
    assert_ne!(termios.c_oflag & libc::OPOST, 0);
    assert_ne!(termios.c_lflag & libc::ECHO, 0);
    assert_ne!(termios.c_lflag & libc::ICANON, 0);
    assert_eq!(termios.c_cc[libc::VINTR], 0x03);
    assert_eq!(termios.c_cc[libc::VERASE], 0x7f);
}

#[test]
fn test_set_raw_clears_flags() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    set_sane(&mut termios);
    set_raw(&mut termios);
    // Raw mode should disable canonical and echo
    assert_eq!(termios.c_lflag & libc::ICANON, 0);
    assert_eq!(termios.c_lflag & libc::ECHO, 0);
    assert_eq!(termios.c_lflag & libc::ISIG, 0);
    assert_eq!(termios.c_oflag & libc::OPOST, 0);
}

#[test]
fn test_set_cooked_restores_flags() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    set_raw(&mut termios);
    set_cooked(&mut termios);
    assert_ne!(termios.c_lflag & libc::ICANON, 0);
    assert_ne!(termios.c_lflag & libc::ECHO, 0);
    assert_ne!(termios.c_lflag & libc::ISIG, 0);
    assert_ne!(termios.c_oflag & libc::OPOST, 0);
}

#[test]
fn test_apply_flag_echo() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    assert!(apply_flag(&mut termios, "echo"));
    assert_ne!(termios.c_lflag & libc::ECHO, 0);
    assert!(apply_flag(&mut termios, "-echo"));
    assert_eq!(termios.c_lflag & libc::ECHO, 0);
}

#[test]
fn test_apply_flag_icanon() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    assert!(apply_flag(&mut termios, "icanon"));
    assert_ne!(termios.c_lflag & libc::ICANON, 0);
    assert!(apply_flag(&mut termios, "-icanon"));
    assert_eq!(termios.c_lflag & libc::ICANON, 0);
}

#[test]
fn test_apply_flag_unknown() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    assert!(!apply_flag(&mut termios, "nonexistent_flag"));
}

#[test]
fn test_apply_flag_csize() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    assert!(apply_flag(&mut termios, "cs7"));
    assert_eq!(termios.c_cflag & libc::CSIZE, libc::CS7);
    assert!(apply_flag(&mut termios, "cs8"));
    assert_eq!(termios.c_cflag & libc::CSIZE, libc::CS8);
}

#[test]
fn test_apply_settings_sane() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    let settings = vec!["sane".to_string()];
    let result = apply_settings(&mut termios, &settings);
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_ne!(termios.c_lflag & libc::ECHO, 0);
}

#[test]
fn test_apply_settings_raw_cooked() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    set_sane(&mut termios);

    let settings = vec!["raw".to_string()];
    let result = apply_settings(&mut termios, &settings);
    assert!(result.is_ok());
    assert_eq!(termios.c_lflag & libc::ICANON, 0);

    let settings = vec!["cooked".to_string()];
    let result = apply_settings(&mut termios, &settings);
    assert!(result.is_ok());
    assert_ne!(termios.c_lflag & libc::ICANON, 0);
}

#[test]
fn test_apply_settings_special_char() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    let settings = vec!["intr".to_string(), "^X".to_string()];
    let result = apply_settings(&mut termios, &settings);
    assert!(result.is_ok());
    assert_eq!(termios.c_cc[libc::VINTR], 0x18); // ^X = 0x18
}

#[test]
fn test_apply_settings_invalid() {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    let settings = vec!["totally_bogus_flag".to_string()];
    let result = apply_settings(&mut termios, &settings);
    assert!(result.is_err());
}
