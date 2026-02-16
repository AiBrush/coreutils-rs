use super::*;

// ---- Helper functions ----

fn run_cat(input: &[u8], config: &CatConfig) -> Vec<u8> {
    let mut out = Vec::new();
    let mut line_num = 1u64;
    cat_with_options(input, config, &mut line_num, &mut out).unwrap();
    out
}

fn plain_config() -> CatConfig {
    CatConfig::default()
}

fn numbered_config() -> CatConfig {
    CatConfig {
        number: true,
        ..Default::default()
    }
}

fn number_nonblank_config() -> CatConfig {
    CatConfig {
        number_nonblank: true,
        ..Default::default()
    }
}

fn show_ends_config() -> CatConfig {
    CatConfig {
        show_ends: true,
        ..Default::default()
    }
}

fn show_tabs_config() -> CatConfig {
    CatConfig {
        show_tabs: true,
        ..Default::default()
    }
}

fn show_nonprinting_config() -> CatConfig {
    CatConfig {
        show_nonprinting: true,
        ..Default::default()
    }
}

fn squeeze_blank_config() -> CatConfig {
    CatConfig {
        squeeze_blank: true,
        ..Default::default()
    }
}

fn bin_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push(name);
    path
}

// ---- Empty/minimal input ----

#[test]
fn test_empty_input() {
    assert_eq!(run_cat(b"", &plain_config()), b"");
}

#[test]
fn test_single_byte() {
    assert_eq!(run_cat(b"x", &plain_config()), b"x");
}

#[test]
fn test_single_line_with_newline() {
    assert_eq!(run_cat(b"hello\n", &plain_config()), b"hello\n");
}

// ---- cat -n (number all lines) ----

#[test]
fn test_number_lines() {
    let input = b"one\ntwo\nthree\n";
    let result = run_cat(input, &numbered_config());
    assert_eq!(
        String::from_utf8_lossy(&result),
        "     1\tone\n     2\ttwo\n     3\tthree\n"
    );
}

#[test]
fn test_number_with_empty() {
    let input = b"one\n\nthree\n";
    let result = run_cat(input, &numbered_config());
    assert_eq!(
        String::from_utf8_lossy(&result),
        "     1\tone\n     2\t\n     3\tthree\n"
    );
}

// ---- cat -b (number non-blank lines) ----

#[test]
fn test_number_nonblank() {
    let input = b"one\n\nthree\n";
    let result = run_cat(input, &number_nonblank_config());
    assert_eq!(
        String::from_utf8_lossy(&result),
        "     1\tone\n\n     2\tthree\n"
    );
}

// ---- cat -E (show ends) ----

#[test]
fn test_show_ends() {
    let input = b"one\ntwo\n";
    let result = run_cat(input, &show_ends_config());
    assert_eq!(String::from_utf8_lossy(&result), "one$\ntwo$\n");
}

#[test]
fn test_show_ends_no_trailing_newline() {
    let input = b"hello";
    let result = run_cat(input, &show_ends_config());
    assert_eq!(String::from_utf8_lossy(&result), "hello");
}

// ---- cat -T (show tabs) ----

#[test]
fn test_show_tabs() {
    let input = b"a\tb\n";
    let result = run_cat(input, &show_tabs_config());
    assert_eq!(String::from_utf8_lossy(&result), "a^Ib\n");
}

// ---- cat -v (show non-printing) ----

#[test]
fn test_show_nonprinting_control() {
    let input = &[1u8, 2, 3]; // ^A ^B ^C
    let result = run_cat(input, &show_nonprinting_config());
    assert_eq!(String::from_utf8_lossy(&result), "^A^B^C");
}

#[test]
fn test_show_nonprinting_del() {
    let input = &[127u8]; // DEL
    let result = run_cat(input, &show_nonprinting_config());
    assert_eq!(String::from_utf8_lossy(&result), "^?");
}

#[test]
fn test_show_nonprinting_high() {
    let input = &[128u8]; // M-^@
    let result = run_cat(input, &show_nonprinting_config());
    assert_eq!(String::from_utf8_lossy(&result), "M-^@");
}

#[test]
fn test_show_nonprinting_high_printable() {
    let input = &[160u8]; // M-space
    let result = run_cat(input, &show_nonprinting_config());
    assert_eq!(String::from_utf8_lossy(&result), "M- ");
}

#[test]
fn test_show_nonprinting_255() {
    let input = &[255u8]; // M-^?
    let result = run_cat(input, &show_nonprinting_config());
    assert_eq!(String::from_utf8_lossy(&result), "M-^?");
}

#[test]
fn test_show_nonprinting_tab_preserved() {
    // -v without -T: tab should be preserved
    let input = b"a\tb\n";
    let result = run_cat(input, &show_nonprinting_config());
    assert_eq!(String::from_utf8_lossy(&result), "a\tb\n");
}

#[test]
fn test_show_nonprinting_newline_preserved() {
    // -v: newline should always be preserved
    let input = b"a\nb\n";
    let result = run_cat(input, &show_nonprinting_config());
    assert_eq!(String::from_utf8_lossy(&result), "a\nb\n");
}

// ---- cat -s (squeeze blank) ----

#[test]
fn test_squeeze_blank() {
    let input = b"one\n\n\n\ntwo\n";
    let result = run_cat(input, &squeeze_blank_config());
    assert_eq!(String::from_utf8_lossy(&result), "one\n\ntwo\n");
}

#[test]
fn test_squeeze_no_consecutive() {
    let input = b"one\n\ntwo\n\nthree\n";
    let result = run_cat(input, &squeeze_blank_config());
    assert_eq!(String::from_utf8_lossy(&result), "one\n\ntwo\n\nthree\n");
}

// ---- Flag combinations ----

#[test]
fn test_number_and_show_ends() {
    let config = CatConfig {
        number: true,
        show_ends: true,
        ..Default::default()
    };
    let input = b"one\ntwo\n";
    let result = run_cat(input, &config);
    assert_eq!(
        String::from_utf8_lossy(&result),
        "     1\tone$\n     2\ttwo$\n"
    );
}

#[test]
fn test_show_all() {
    let config = CatConfig {
        show_nonprinting: true,
        show_ends: true,
        show_tabs: true,
        ..Default::default()
    };
    let input = b"a\t\x01\n";
    let result = run_cat(input, &config);
    assert_eq!(String::from_utf8_lossy(&result), "a^I^A$\n");
}

#[test]
fn test_number_nonblank_squeeze() {
    let config = CatConfig {
        number_nonblank: true,
        squeeze_blank: true,
        ..Default::default()
    };
    let input = b"one\n\n\n\ntwo\n";
    let result = run_cat(input, &config);
    assert_eq!(
        String::from_utf8_lossy(&result),
        "     1\tone\n\n     2\ttwo\n"
    );
}

// ---- Multi-file line numbering continuity ----

#[test]
fn test_line_number_continuity() {
    let config = numbered_config();
    let mut line_num = 1u64;
    let mut out = Vec::new();

    cat_with_options(b"one\ntwo\n", &config, &mut line_num, &mut out).unwrap();
    cat_with_options(b"three\nfour\n", &config, &mut line_num, &mut out).unwrap();

    assert_eq!(
        String::from_utf8_lossy(&out),
        "     1\tone\n     2\ttwo\n     3\tthree\n     4\tfour\n"
    );
}

// ---- Large input ----

#[test]
fn test_large_input_plain() {
    let input = vec![b'A'; 1_000_000];
    let result = run_cat(&input, &plain_config());
    assert_eq!(result.len(), 1_000_000);
}

#[test]
fn test_large_input_numbered() {
    let mut input = Vec::new();
    for i in 0..100_000 {
        input.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    let result = run_cat(&input, &numbered_config());
    assert!(result.len() > input.len()); // numbering adds bytes
}

// ---- Integration tests with binary ----

#[test]
fn test_binary_basic() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world\n").unwrap();

    let output = std::process::Command::new(bin_path("fcat"))
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello world\n");
}

#[test]
fn test_binary_number() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "one\ntwo\nthree\n").unwrap();

    let output = std::process::Command::new(bin_path("fcat"))
        .arg("-n")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "     1\tone\n     2\ttwo\n     3\tthree\n"
    );
}

#[test]
fn test_binary_show_ends() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "one\ntwo\n").unwrap();

    let output = std::process::Command::new(bin_path("fcat"))
        .arg("-E")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "one$\ntwo$\n");
}

#[test]
fn test_binary_show_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "a\tb\n").unwrap();

    let output = std::process::Command::new(bin_path("fcat"))
        .arg("-T")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "a^Ib\n");
}

#[test]
fn test_binary_squeeze() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "one\n\n\n\ntwo\n").unwrap();

    let output = std::process::Command::new(bin_path("fcat"))
        .arg("-s")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "one\n\ntwo\n");
}

#[test]
fn test_binary_multiple_files() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file1, "aaa\n").unwrap();
    std::fs::write(&file2, "bbb\n").unwrap();

    let output = std::process::Command::new(bin_path("fcat"))
        .arg(file1.to_str().unwrap())
        .arg(file2.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "aaa\nbbb\n");
}

#[test]
fn test_binary_nonexistent_file() {
    let output = std::process::Command::new(bin_path("fcat"))
        .arg("/nonexistent/file")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cat:"));
    #[cfg(unix)]
    assert!(stderr.contains("No such file or directory"));
}

#[test]
fn test_binary_directory() {
    let dir = tempfile::tempdir().unwrap();

    let output = std::process::Command::new(bin_path("fcat"))
        .arg(dir.path().to_str().unwrap())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Is a directory"));
}

#[test]
fn test_binary_version() {
    let output = std::process::Command::new(bin_path("fcat"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("cat (fcoreutils)"));
}

#[test]
fn test_binary_show_all() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "a\t\x01\n").unwrap();

    let output = std::process::Command::new(bin_path("fcat"))
        .arg("-A")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "a^I^A$\n");
}

// ---- GNU compatibility (Linux only — macOS/Windows have BSD utilities) ----

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_plain() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello\nworld\n").unwrap();

    let our_output = std::process::Command::new(bin_path("fcat"))
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("cat")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_number() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "one\n\nthree\n").unwrap();

    let our_output = std::process::Command::new(bin_path("fcat"))
        .arg("-n")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("cat")
        .arg("-n")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_number_nonblank() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "one\n\nthree\n").unwrap();

    let our_output = std::process::Command::new(bin_path("fcat"))
        .arg("-b")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("cat")
        .arg("-b")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_show_nonprinting() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.bin");
    // Write all 256 byte values
    let data: Vec<u8> = (0..=255).collect();
    std::fs::write(&file_path, &data).unwrap();

    let our_output = std::process::Command::new(bin_path("fcat"))
        .arg("-v")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("cat")
        .arg("-v")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}
