use super::*;

fn comm_str(input1: &str, input2: &str, config: &CommConfig) -> String {
    let mut out = Vec::new();
    comm(
        input1.as_bytes(),
        input2.as_bytes(),
        config,
        "comm",
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

fn default_config() -> CommConfig {
    CommConfig::default()
}

// === Unit Tests ===

#[test]
fn test_empty_both() {
    assert_eq!(comm_str("", "", &default_config()), "");
}

#[test]
fn test_empty_file1() {
    assert_eq!(comm_str("", "a\nb\n", &default_config()), "\ta\n\tb\n");
}

#[test]
fn test_empty_file2() {
    assert_eq!(comm_str("a\nb\n", "", &default_config()), "a\nb\n");
}

#[test]
fn test_all_common() {
    assert_eq!(
        comm_str("a\nb\nc\n", "a\nb\nc\n", &default_config()),
        "\t\ta\n\t\tb\n\t\tc\n"
    );
}

#[test]
fn test_all_unique_file1() {
    assert_eq!(
        comm_str("a\nb\nc\n", "d\ne\nf\n", &default_config()),
        "a\nb\nc\n\td\n\te\n\tf\n"
    );
}

#[test]
fn test_all_unique_file2() {
    assert_eq!(
        comm_str("d\ne\nf\n", "a\nb\nc\n", &default_config()),
        "\ta\n\tb\n\tc\nd\ne\nf\n"
    );
}

#[test]
fn test_mixed() {
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &default_config());
    assert_eq!(result, "a\n\t\tb\n\tc\n\t\td\n");
}

#[test]
fn test_suppress_col1() {
    let mut config = default_config();
    config.suppress_col1 = true;
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &config);
    assert_eq!(result, "\tb\nc\n\td\n");
}

#[test]
fn test_suppress_col2() {
    let mut config = default_config();
    config.suppress_col2 = true;
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &config);
    assert_eq!(result, "a\n\tb\n\td\n");
}

#[test]
fn test_suppress_col3() {
    let mut config = default_config();
    config.suppress_col3 = true;
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &config);
    assert_eq!(result, "a\n\tc\n");
}

#[test]
fn test_suppress_col12() {
    let mut config = default_config();
    config.suppress_col1 = true;
    config.suppress_col2 = true;
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &config);
    assert_eq!(result, "b\nd\n");
}

#[test]
fn test_suppress_col13() {
    let mut config = default_config();
    config.suppress_col1 = true;
    config.suppress_col3 = true;
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &config);
    assert_eq!(result, "c\n");
}

#[test]
fn test_suppress_col23() {
    let mut config = default_config();
    config.suppress_col2 = true;
    config.suppress_col3 = true;
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &config);
    assert_eq!(result, "a\n");
}

#[test]
fn test_suppress_all() {
    let mut config = default_config();
    config.suppress_col1 = true;
    config.suppress_col2 = true;
    config.suppress_col3 = true;
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &config);
    assert_eq!(result, "");
}

#[test]
fn test_case_insensitive() {
    let mut config = default_config();
    config.case_insensitive = true;
    let result = comm_str("A\nB\n", "a\nb\n", &config);
    assert_eq!(result, "\t\tA\n\t\tB\n");
}

#[test]
fn test_case_insensitive_mixed() {
    let mut config = default_config();
    config.case_insensitive = true;
    let result = comm_str("A\nc\n", "b\nC\n", &config);
    assert_eq!(result, "A\n\tb\n\t\tc\n");
}

#[test]
fn test_custom_delimiter() {
    let mut config = default_config();
    config.output_delimiter = Some(b",".to_vec());
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &config);
    assert_eq!(result, "a\n,,b\n,c\n,,d\n");
}

#[test]
fn test_custom_delimiter_multi_byte() {
    let mut config = default_config();
    config.output_delimiter = Some(b"::".to_vec());
    let result = comm_str("a\nb\n", "a\nc\n", &config);
    assert_eq!(result, "::::a\nb\n::c\n");
}

#[test]
fn test_total() {
    let mut config = default_config();
    config.total = true;
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &config);
    assert_eq!(result, "a\n\t\tb\n\tc\n\t\td\n1\t1\t2\ttotal\n");
}

#[test]
fn test_total_custom_delimiter() {
    let mut config = default_config();
    config.total = true;
    config.output_delimiter = Some(b",".to_vec());
    let result = comm_str("a\nb\n", "b\nc\n", &config);
    assert_eq!(result, "a\n,,b\n,c\n1,1,1,total\n");
}

#[test]
fn test_total_with_suppression() {
    let mut config = default_config();
    config.total = true;
    config.suppress_col1 = true;
    let result = comm_str("a\nb\nd\n", "b\nc\nd\n", &config);
    // Total still shows all counts even when columns are suppressed
    assert_eq!(result, "\tb\nc\n\td\n1\t1\t2\ttotal\n");
}

#[test]
fn test_zero_terminated() {
    let mut config = default_config();
    config.zero_terminated = true;
    let result = comm_str("a\0b\0", "b\0c\0", &config);
    assert_eq!(result, "a\0\t\tb\0\tc\0");
}

#[test]
fn test_no_trailing_newline() {
    let result = comm_str("a\nb", "a\nb", &default_config());
    assert_eq!(result, "\t\ta\n\t\tb\n");
}

#[test]
fn test_single_line_each() {
    assert_eq!(
        comm_str("hello\n", "hello\n", &default_config()),
        "\t\thello\n"
    );
}

#[test]
fn test_single_line_different() {
    assert_eq!(
        comm_str("aaa\n", "bbb\n", &default_config()),
        "aaa\n\tbbb\n"
    );
}

#[test]
fn test_order_check_default() {
    let config = default_config();
    let mut out = Vec::new();
    let result = comm(b"b\na\n", b"a\nb\n", &config, "comm", &mut out).unwrap();
    assert!(result.had_order_error);
}

#[test]
fn test_order_check_strict() {
    let mut config = default_config();
    config.order_check = OrderCheck::Strict;
    let mut out = Vec::new();
    let result = comm(b"b\na\n", b"a\nb\n", &config, "comm", &mut out).unwrap();
    assert!(result.had_order_error);
}

#[test]
fn test_order_check_none() {
    let mut config = default_config();
    config.order_check = OrderCheck::None;
    let mut out = Vec::new();
    let result = comm(b"b\na\n", b"a\nb\n", &config, "comm", &mut out).unwrap();
    assert!(!result.had_order_error);
}

#[test]
fn test_large_file() {
    let mut data1 = String::new();
    let mut data2 = String::new();
    for i in 0..1000 {
        data1.push_str(&format!("{:05}\n", i * 2));
        data2.push_str(&format!("{:05}\n", i * 2 + 1));
    }
    let result = comm_str(&data1, &data2, &default_config());
    // All lines should be in col1 or col2 (none common since even vs odd)
    assert!(!result.contains("\t\t"));
}

#[test]
fn test_large_file_common() {
    let mut data = String::new();
    for i in 0..1000 {
        data.push_str(&format!("{:05}\n", i));
    }
    let result = comm_str(&data, &data, &default_config());
    // All lines should be in col3
    for line in result.lines() {
        assert!(
            line.starts_with("\t\t"),
            "Expected tab-tab prefix: {:?}",
            line
        );
    }
}

// === Integration Tests ===

#[cfg(test)]
mod integration {
    use std::process::Command;

    fn bin_path() -> std::path::PathBuf {
        let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("target");
        if cfg!(debug_assertions) {
            path.push("debug");
        } else {
            path.push("release");
        }
        path.push("fcomm");
        path
    }

    fn run_fcomm(args: &[&str]) -> (Vec<u8>, Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path());
        cmd.args(args);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let output = cmd.output().expect("failed to spawn fcomm");
        (
            output.stdout,
            output.stderr,
            output.status.code().unwrap_or(1),
        )
    }

    fn run_fcomm_stdin(input: &[u8], args: &[&str]) -> (Vec<u8>, Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path());
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn fcomm");
        use std::io::Write;
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().expect("failed to wait");
        (
            output.stdout,
            output.stderr,
            output.status.code().unwrap_or(1),
        )
    }

    #[test]
    fn test_basic() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a\nb\nc\n").unwrap();
        std::fs::write(&f2, "b\nc\nd\n").unwrap();
        let (out, _, code) = run_fcomm(&[f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a\n\t\tb\n\t\tc\n\td\n");
    }

    #[test]
    fn test_suppress_flags() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a\nb\nc\n").unwrap();
        std::fs::write(&f2, "b\nc\nd\n").unwrap();

        let (out, _, code) = run_fcomm(&["-12", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "b\nc\n");
    }

    #[test]
    fn test_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f2, "b\nc\n").unwrap();
        let (out, _, code) = run_fcomm_stdin(b"a\nb\nc\n", &["-", f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a\n\t\tb\n\t\tc\n");
    }

    #[test]
    fn test_help() {
        let (_, _, code) = run_fcomm(&["--help"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_version() {
        let (out, _, code) = run_fcomm(&["--version"]);
        assert_eq!(code, 0);
        assert!(String::from_utf8_lossy(&out).contains("comm"));
    }

    #[test]
    fn test_missing_operand() {
        let (_, err, code) = run_fcomm(&[]);
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&err).contains("missing operand"));
    }

    #[test]
    fn test_missing_second_operand() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        std::fs::write(&f1, "a\n").unwrap();
        let (_, err, code) = run_fcomm(&[f1.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&err).contains("missing operand"));
    }

    #[test]
    fn test_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f2, "a\n").unwrap();
        let (_, _, code) = run_fcomm(&["/tmp/nonexistent_fcomm_test", f2.to_str().unwrap()]);
        assert_eq!(code, 1);
    }

    #[test]
    fn test_output_delimiter() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a\nb\n").unwrap();
        std::fs::write(&f2, "b\nc\n").unwrap();
        let (out, _, code) = run_fcomm(&[
            "--output-delimiter=,",
            f1.to_str().unwrap(),
            f2.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a\n,,b\n,c\n");
    }

    #[test]
    fn test_total_flag() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a\nb\n").unwrap();
        std::fs::write(&f2, "b\nc\n").unwrap();
        let (out, _, code) = run_fcomm(&["--total", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("total"));
        assert!(s.ends_with("1\t1\t1\ttotal\n"));
    }

    #[test]
    fn test_check_order_unsorted() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "b\na\n").unwrap();
        std::fs::write(&f2, "a\nb\n").unwrap();
        let (_, err, code) =
            run_fcomm(&["--check-order", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&err).contains("not in sorted order"));
    }

    #[test]
    fn test_nocheck_order_unsorted() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "b\na\n").unwrap();
        std::fs::write(&f2, "a\nb\n").unwrap();
        let (_, err, code) = run_fcomm(&[
            "--nocheck-order",
            f1.to_str().unwrap(),
            f2.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert!(String::from_utf8_lossy(&err).is_empty());
    }

    #[test]
    fn test_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        // 100K lines — triggers mmap path
        let mut data1 = String::new();
        let mut data2 = String::new();
        for i in 0u64..100_000 {
            data1.push_str(&format!("{:07}\n", i));
            data2.push_str(&format!("{:07}\n", i + 50_000));
        }
        std::fs::write(&f1, &data1).unwrap();
        std::fs::write(&f2, &data2).unwrap();
        let (out, _, code) = run_fcomm(&["-12", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        let binding = String::from_utf8_lossy(&out);
        let lines: Vec<&str> = binding.lines().collect();
        // Lines 50000-99999 are common
        assert_eq!(lines.len(), 50_000);
    }

    // GNU comparison tests
    #[cfg(target_os = "linux")]
    mod gnu_compat {
        use super::*;

        fn run_gnu_comm(args: &[&str]) -> Option<(Vec<u8>, i32)> {
            let output = Command::new("comm").args(args).output().ok()?;
            Some((output.stdout, output.status.code().unwrap_or(1)))
        }

        #[test]
        fn test_gnu_basic() {
            let dir = tempfile::tempdir().unwrap();
            let f1 = dir.path().join("a.txt");
            let f2 = dir.path().join("b.txt");
            std::fs::write(&f1, "a\nb\nc\n").unwrap();
            std::fs::write(&f2, "b\nc\nd\n").unwrap();
            let args = [f1.to_str().unwrap(), f2.to_str().unwrap()];

            let (our_out, _, our_code) = run_fcomm(&args);
            if let Some((gnu_out, gnu_code)) = run_gnu_comm(&args) {
                assert_eq!(our_out, gnu_out, "Output differs from GNU comm");
                assert_eq!(our_code, gnu_code, "Exit code differs");
            }
        }

        #[test]
        fn test_gnu_suppress_12() {
            let dir = tempfile::tempdir().unwrap();
            let f1 = dir.path().join("a.txt");
            let f2 = dir.path().join("b.txt");
            std::fs::write(&f1, "a\nb\nc\n").unwrap();
            std::fs::write(&f2, "b\nc\nd\n").unwrap();
            let args = ["-12", f1.to_str().unwrap(), f2.to_str().unwrap()];

            let (our_out, _, _) = run_fcomm(&args);
            if let Some((gnu_out, _)) = run_gnu_comm(&args) {
                assert_eq!(our_out, gnu_out, "Output differs from GNU comm -12");
            }
        }

        #[test]
        fn test_gnu_all_common() {
            let dir = tempfile::tempdir().unwrap();
            let f1 = dir.path().join("a.txt");
            let f2 = dir.path().join("b.txt");
            std::fs::write(&f1, "apple\nbanana\ncherry\n").unwrap();
            std::fs::write(&f2, "apple\nbanana\ncherry\n").unwrap();
            let args = [f1.to_str().unwrap(), f2.to_str().unwrap()];

            let (our_out, _, _) = run_fcomm(&args);
            if let Some((gnu_out, _)) = run_gnu_comm(&args) {
                assert_eq!(
                    our_out, gnu_out,
                    "Output differs from GNU comm (all common)"
                );
            }
        }

        #[test]
        fn test_gnu_empty_files() {
            let dir = tempfile::tempdir().unwrap();
            let f1 = dir.path().join("a.txt");
            let f2 = dir.path().join("b.txt");
            std::fs::write(&f1, "").unwrap();
            std::fs::write(&f2, "a\nb\n").unwrap();
            let args = [f1.to_str().unwrap(), f2.to_str().unwrap()];

            let (our_out, _, _) = run_fcomm(&args);
            if let Some((gnu_out, _)) = run_gnu_comm(&args) {
                assert_eq!(
                    our_out, gnu_out,
                    "Output differs from GNU comm (empty file)"
                );
            }
        }
    }
}
