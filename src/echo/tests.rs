use super::*;
use std::process::Command;

// ---- Helper functions ----

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn bin_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("fecho");
    path
}

fn cmd() -> Command {
    Command::new(bin_path())
}

// ---- Unit tests for echo_output ----

#[test]
fn test_echo_simple() {
    let args = strs(&["hello", "world"]);
    let config = EchoConfig::default();
    let output = echo_output(&args, &config);
    assert_eq!(output, b"hello world\n");
}

#[test]
fn test_echo_no_newline() {
    let args = strs(&["hello"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: false,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, b"hello");
}

#[test]
fn test_echo_escapes() {
    let args = strs(&["a\\tb\\nc\\\\d"]);
    let config = EchoConfig {
        trailing_newline: true,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, b"a\tb\nc\\d\n");
}

#[test]
fn test_echo_escape_c() {
    let args = strs(&["hello\\cworld"]);
    let config = EchoConfig {
        trailing_newline: true,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    // \c stops all output — no trailing newline, no "world"
    assert_eq!(output, b"hello");
}

#[test]
fn test_echo_octal() {
    let args = strs(&["\\0101"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, b"A");
}

#[test]
fn test_echo_hex() {
    let args = strs(&["\\x41"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, b"A");
}

#[test]
fn test_echo_no_args() {
    let args: Vec<String> = Vec::new();
    let config = EchoConfig::default();
    let output = echo_output(&args, &config);
    assert_eq!(output, b"\n");
}

// ---- parse_echo_args tests ----

#[test]
fn test_parse_n_flag() {
    let args = strs(&["-n", "hello"]);
    let (config, rest) = parse_echo_args(&args);
    assert!(!config.trailing_newline);
    assert!(!config.interpret_escapes);
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0], "hello");
}

#[test]
fn test_parse_e_flag() {
    let args = strs(&["-e", "hello"]);
    let (config, rest) = parse_echo_args(&args);
    assert!(config.trailing_newline);
    assert!(config.interpret_escapes);
    assert_eq!(rest.len(), 1);
}

#[test]
fn test_parse_combined_flags() {
    let args = strs(&["-neE", "hello"]);
    let (config, rest) = parse_echo_args(&args);
    // -n, -e, then -E: newline off, escapes off (E overrides e)
    assert!(!config.trailing_newline);
    assert!(!config.interpret_escapes);
    assert_eq!(rest.len(), 1);
}

#[test]
fn test_parse_invalid_flag_treated_as_text() {
    let args = strs(&["-z", "hello"]);
    let (config, rest) = parse_echo_args(&args);
    // -z is not valid, so it's text
    assert!(config.trailing_newline);
    assert!(!config.interpret_escapes);
    assert_eq!(rest.len(), 2);
    assert_eq!(rest[0], "-z");
}

#[test]
fn test_parse_double_dash_is_text() {
    let args = strs(&["--", "hello"]);
    let (config, rest) = parse_echo_args(&args);
    // "--" is not a valid flag combo, treated as text
    assert!(config.trailing_newline);
    assert_eq!(rest.len(), 2);
    assert_eq!(rest[0], "--");
}

#[test]
fn test_parse_bare_dash_is_text() {
    let args = strs(&["-"]);
    let (config, rest) = parse_echo_args(&args);
    // "-" alone: length < 2, so treated as text
    assert!(config.trailing_newline);
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0], "-");
}

// ---- Escape sequence edge cases ----

#[test]
fn test_escape_bell() {
    let args = strs(&["\\a"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, &[0x07]);
}

#[test]
fn test_escape_backspace() {
    let args = strs(&["\\b"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, &[0x08]);
}

#[test]
fn test_escape_esc() {
    let args = strs(&["\\e"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, &[0x1B]);
}

#[test]
fn test_escape_form_feed() {
    let args = strs(&["\\f"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, &[0x0C]);
}

#[test]
fn test_escape_vertical_tab() {
    let args = strs(&["\\v"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, &[0x0B]);
}

#[test]
fn test_escape_carriage_return() {
    let args = strs(&["\\r"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, &[b'\r']);
}

#[test]
fn test_escape_octal_nul() {
    let args = strs(&["\\0"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, &[0x00]);
}

#[test]
fn test_escape_trailing_backslash() {
    let args = strs(&["abc\\"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, b"abc\\");
}

#[test]
fn test_escape_unknown_sequence() {
    let args = strs(&["\\z"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    assert_eq!(output, b"\\z");
}

#[test]
fn test_escape_hex_no_digits() {
    let args = strs(&["\\xZZ"]);
    let config = EchoConfig {
        trailing_newline: false,
        interpret_escapes: true,
    };
    let output = echo_output(&args, &config);
    // No valid hex digits after \x, so \x is output literally
    assert_eq!(output, b"\\xZZ");
}

#[test]
fn test_escapes_disabled() {
    // With -E (default), backslash sequences are NOT interpreted
    let args = strs(&["hello\\nworld"]);
    let config = EchoConfig::default();
    let output = echo_output(&args, &config);
    assert_eq!(output, b"hello\\nworld\n");
}

// ---- Command-based integration tests ----

#[test]
fn test_cmd_echo_simple() {
    let output = cmd().args(["hello", "world"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"hello world\n");
}

#[test]
fn test_cmd_echo_no_newline() {
    let output = cmd().args(["-n", "hello"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"hello");
}

#[test]
fn test_cmd_echo_escape_tab_newline() {
    let output = cmd().args(["-e", "a\\tb\\n"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"a\tb\n\n");
}

#[test]
fn test_cmd_echo_escape_c() {
    let output = cmd().args(["-e", "hello\\cworld"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"hello");
}

#[test]
fn test_cmd_echo_octal() {
    let output = cmd().args(["-ne", "\\0101"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"A");
}

#[test]
fn test_cmd_echo_hex() {
    let output = cmd().args(["-ne", "\\x41"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"A");
}

#[test]
fn test_cmd_echo_no_args() {
    let output = cmd().output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"\n");
}

#[test]
fn test_cmd_invalid_flag_is_text() {
    let output = cmd().args(["-z", "hello"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"-z hello\n");
}

#[test]
fn test_echo_matches_gnu() {
    // Compare basic output with GNU echo
    let gnu = Command::new("echo").args(["hello", "world"]).output();
    if let Ok(gnu) = gnu {
        let ours = cmd().args(["hello", "world"]).output().unwrap();
        assert_eq!(ours.stdout, gnu.stdout, "Output mismatch with GNU echo");
        assert_eq!(
            ours.status.code(),
            gnu.status.code(),
            "Exit code mismatch"
        );
    }
}
