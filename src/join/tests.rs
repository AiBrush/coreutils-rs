use super::*;

fn join_str(input1: &str, input2: &str, config: &JoinConfig) -> String {
    let mut out = Vec::new();
    join(
        input1.as_bytes(),
        input2.as_bytes(),
        config,
        "join",
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

fn default_config() -> JoinConfig {
    JoinConfig::default()
}

// === Unit Tests ===

#[test]
fn test_empty_both() {
    assert_eq!(join_str("", "", &default_config()), "");
}

#[test]
fn test_empty_file1() {
    assert_eq!(join_str("", "a b\n", &default_config()), "");
}

#[test]
fn test_empty_file2() {
    assert_eq!(join_str("a b\n", "", &default_config()), "");
}

#[test]
fn test_basic_join() {
    let result = join_str("a 1\nb 2\n", "a x\nb y\n", &default_config());
    assert_eq!(result, "a 1 x\nb 2 y\n");
}

#[test]
fn test_partial_match() {
    let result = join_str("a 1\nb 2\nc 3\n", "b x\nc y\nd z\n", &default_config());
    assert_eq!(result, "b 2 x\nc 3 y\n");
}

#[test]
fn test_no_match() {
    let result = join_str("a 1\n", "b 2\n", &default_config());
    assert_eq!(result, "");
}

#[test]
fn test_all_match() {
    let result = join_str("a 1\nb 2\n", "a x\nb y\n", &default_config());
    assert_eq!(result, "a 1 x\nb 2 y\n");
}

#[test]
fn test_unpaired_a1() {
    let mut config = default_config();
    config.print_unpaired1 = true;
    let result = join_str("a 1\nb 2\nc 3\n", "b x\n", &config);
    assert_eq!(result, "a 1\nb 2 x\nc 3\n");
}

#[test]
fn test_unpaired_a2() {
    let mut config = default_config();
    config.print_unpaired2 = true;
    let result = join_str("b 2\n", "a x\nb y\nc z\n", &config);
    assert_eq!(result, "a x\nb 2 y\nc z\n");
}

#[test]
fn test_unpaired_a1_a2() {
    let mut config = default_config();
    config.print_unpaired1 = true;
    config.print_unpaired2 = true;
    let result = join_str("a 1\nb 2\n", "b x\nc y\n", &config);
    assert_eq!(result, "a 1\nb 2 x\nc y\n");
}

#[test]
fn test_only_unpaired_v1() {
    let mut config = default_config();
    config.only_unpaired1 = true;
    let result = join_str("a 1\nb 2\nc 3\n", "b x\n", &config);
    assert_eq!(result, "a 1\nc 3\n");
}

#[test]
fn test_only_unpaired_v2() {
    let mut config = default_config();
    config.only_unpaired2 = true;
    let result = join_str("b 2\n", "a x\nb y\nc z\n", &config);
    assert_eq!(result, "a x\nc z\n");
}

#[test]
fn test_only_unpaired_v1_v2() {
    let mut config = default_config();
    config.only_unpaired1 = true;
    config.only_unpaired2 = true;
    let result = join_str("a 1\nb 2\n", "b x\nc y\n", &config);
    assert_eq!(result, "a 1\nc y\n");
}

#[test]
fn test_join_field_2() {
    let mut config = default_config();
    config.field1 = 1; // 0-indexed field 2
    let result = join_str("1 a\n2 b\n", "a x\nb y\n", &default_config());
    // Default: join on field 1. "1" vs "a", "2" vs "b" → no match
    assert_eq!(result, "");

    // Now join on field 2 of file 1, field 1 of file 2
    let result2 = join_str("1 a\n2 b\n", "a x\nb y\n", &config);
    assert_eq!(result2, "a 1 x\nb 2 y\n");
}

#[test]
fn test_join_field_both() {
    let mut config = default_config();
    config.field1 = 1; // 0-indexed
    config.field2 = 1;
    let result = join_str("x a\ny b\n", "z a\nw b\n", &config);
    assert_eq!(result, "a x z\nb y w\n");
}

#[test]
fn test_custom_separator() {
    let mut config = default_config();
    config.separator = Some(b',');
    let result = join_str("a,1\nb,2\n", "a,x\nb,y\n", &config);
    assert_eq!(result, "a,1,x\nb,2,y\n");
}

#[test]
fn test_custom_separator_empty_fields() {
    let mut config = default_config();
    config.separator = Some(b',');
    let result = join_str("a,,1\nb,,2\n", "a,x\nb,y\n", &config);
    assert_eq!(result, "a,,1,x\nb,,2,y\n");
}

#[test]
fn test_output_format() {
    let mut config = default_config();
    config.output_format = Some(vec![
        OutputSpec::FileField(0, 0),
        OutputSpec::FileField(1, 1),
    ]);
    let result = join_str("a 1\nb 2\n", "a x\nb y\n", &config);
    assert_eq!(result, "a x\nb y\n");
}

#[test]
fn test_output_format_join_field() {
    let mut config = default_config();
    config.output_format = Some(vec![
        OutputSpec::JoinField,
        OutputSpec::FileField(0, 1),
        OutputSpec::FileField(1, 1),
    ]);
    let result = join_str("a 1\nb 2\n", "a x\nb y\n", &config);
    assert_eq!(result, "a 1 x\nb 2 y\n");
}

#[test]
fn test_empty_filler() {
    let mut config = default_config();
    config.print_unpaired1 = true;
    config.empty_filler = Some(b"EMPTY".to_vec());
    config.output_format = Some(vec![
        OutputSpec::JoinField,
        OutputSpec::FileField(0, 1),
        OutputSpec::FileField(1, 1),
    ]);
    let result = join_str("a 1\nb 2\n", "b x\n", &config);
    assert_eq!(result, "a 1 EMPTY\nb 2 x\n");
}

#[test]
fn test_case_insensitive() {
    let mut config = default_config();
    config.case_insensitive = true;
    let result = join_str("A 1\nB 2\n", "a x\nb y\n", &config);
    assert_eq!(result, "A 1 x\nB 2 y\n");
}

#[test]
fn test_many_to_many() {
    let result = join_str("a 1\na 2\n", "a x\na y\n", &default_config());
    assert_eq!(result, "a 1 x\na 1 y\na 2 x\na 2 y\n");
}

#[test]
fn test_one_to_many() {
    let result = join_str("a 1\n", "a x\na y\na z\n", &default_config());
    assert_eq!(result, "a 1 x\na 1 y\na 1 z\n");
}

#[test]
fn test_many_to_one() {
    let result = join_str("a 1\na 2\na 3\n", "a x\n", &default_config());
    assert_eq!(result, "a 1 x\na 2 x\na 3 x\n");
}

#[test]
fn test_header() {
    let mut config = default_config();
    config.header = true;
    let result = join_str("KEY V1\na 1\nb 2\n", "KEY V2\nb x\nc y\n", &config);
    assert_eq!(result, "KEY V1 V2\nb 2 x\n");
}

#[test]
fn test_zero_terminated() {
    let mut config = default_config();
    config.zero_terminated = true;
    let result = join_str("a 1\0b 2\0", "a x\0b y\0", &config);
    assert_eq!(result, "a 1 x\0b 2 y\0");
}

#[test]
fn test_multiple_fields() {
    let result = join_str("a 1 2 3\nb 4 5 6\n", "a x y\nb p q\n", &default_config());
    assert_eq!(result, "a 1 2 3 x y\nb 4 5 6 p q\n");
}

#[test]
fn test_no_trailing_newline() {
    let result = join_str("a 1\nb 2", "a x\nb y", &default_config());
    assert_eq!(result, "a 1 x\nb 2 y\n");
}

#[test]
fn test_single_field_lines() {
    let result = join_str("a\nb\n", "a\nb\n", &default_config());
    assert_eq!(result, "a\nb\n");
}

#[test]
fn test_auto_format() {
    let mut config = default_config();
    config.auto_format = true;
    config.print_unpaired1 = true;
    config.empty_filler = Some(b"-".to_vec());
    let result = join_str("a 1\nb 2\n", "b x\n", &config);
    // Auto: fields from first lines. File1 has 2 fields, file2 has 2 fields.
    // a: unpaired from file1, pad file2 field with -
    assert_eq!(result, "a 1 -\nb 2 x\n");
}

#[test]
fn test_order_check_default() {
    let config = default_config();
    let mut out = Vec::new();
    let had_error = join(b"b 1\na 2\n", b"a x\nb y\n", &config, "join", &mut out).unwrap();
    assert!(had_error);
}

#[test]
fn test_order_check_none() {
    let mut config = default_config();
    config.order_check = OrderCheck::None;
    let mut out = Vec::new();
    let had_error = join(b"b 1\na 2\n", b"a x\nb y\n", &config, "join", &mut out).unwrap();
    assert!(!had_error);
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
        path.push("fjoin");
        path
    }

    fn run_fjoin(args: &[&str]) -> (Vec<u8>, Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path());
        cmd.args(args);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let output = cmd.output().expect("failed to spawn fjoin");
        (
            output.stdout,
            output.stderr,
            output.status.code().unwrap_or(1),
        )
    }

    fn run_fjoin_stdin(input: &[u8], args: &[&str]) -> (Vec<u8>, Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path());
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn fjoin");
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
        std::fs::write(&f1, "a 1\nb 2\nc 3\n").unwrap();
        std::fs::write(&f2, "a x\nb y\nc z\n").unwrap();
        let (out, _, code) = run_fjoin(&[f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a 1 x\nb 2 y\nc 3 z\n");
    }

    #[test]
    fn test_partial_match() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a 1\nb 2\nc 3\n").unwrap();
        std::fs::write(&f2, "b x\nc y\nd z\n").unwrap();
        let (out, _, code) = run_fjoin(&[f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "b 2 x\nc 3 y\n");
    }

    #[test]
    fn test_unpaired_a1() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a 1\nb 2\n").unwrap();
        std::fs::write(&f2, "b x\n").unwrap();
        let (out, _, code) = run_fjoin(&["-a", "1", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a 1\nb 2 x\n");
    }

    #[test]
    fn test_only_unpaired_v1() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a 1\nb 2\nc 3\n").unwrap();
        std::fs::write(&f2, "b x\n").unwrap();
        let (out, _, code) = run_fjoin(&["-v", "1", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a 1\nc 3\n");
    }

    #[test]
    fn test_custom_separator() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a,1\nb,2\n").unwrap();
        std::fs::write(&f2, "a,x\nb,y\n").unwrap();
        let (out, _, code) = run_fjoin(&["-t", ",", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a,1,x\nb,2,y\n");
    }

    #[test]
    fn test_output_format() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a 1\nb 2\n").unwrap();
        std::fs::write(&f2, "a x\nb y\n").unwrap();
        let (out, _, code) = run_fjoin(&[
            "-o",
            "0,1.2,2.2",
            f1.to_str().unwrap(),
            f2.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a 1 x\nb 2 y\n");
    }

    #[test]
    fn test_empty_filler() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "a 1\nb 2\n").unwrap();
        std::fs::write(&f2, "b x\n").unwrap();
        let (out, _, code) = run_fjoin(&[
            "-a",
            "1",
            "-e",
            "---",
            "-o",
            "0,1.2,2.2",
            f1.to_str().unwrap(),
            f2.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a 1 ---\nb 2 x\n");
    }

    #[test]
    fn test_join_field_flags() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "1 a\n2 b\n").unwrap();
        std::fs::write(&f2, "a x\nb y\n").unwrap();
        let (out, _, code) = run_fjoin(&["-1", "2", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a 1 x\nb 2 y\n");
    }

    #[test]
    fn test_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "A 1\nB 2\n").unwrap();
        std::fs::write(&f2, "a x\nb y\n").unwrap();
        let (out, _, code) = run_fjoin(&["-i", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "A 1 x\nB 2 y\n");
    }

    #[test]
    fn test_header() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, "KEY V1\na 1\nb 2\n").unwrap();
        std::fs::write(&f2, "KEY V2\nb x\nc y\n").unwrap();
        let (out, _, code) = run_fjoin(&["--header", f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "KEY V1 V2\nb 2 x\n");
    }

    #[test]
    fn test_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f2, "a x\nb y\n").unwrap();
        let (out, _, code) = run_fjoin_stdin(b"a 1\nb 2\n", &["-", f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "a 1 x\nb 2 y\n");
    }

    #[test]
    fn test_help() {
        let (_, _, code) = run_fjoin(&["--help"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_version() {
        let (out, _, code) = run_fjoin(&["--version"]);
        assert_eq!(code, 0);
        assert!(String::from_utf8_lossy(&out).contains("join"));
    }

    #[test]
    fn test_missing_operand() {
        let (_, err, code) = run_fjoin(&[]);
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&err).contains("missing operand"));
    }

    #[test]
    fn test_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f2, "a x\n").unwrap();
        let (_, _, code) = run_fjoin(&["/tmp/nonexistent_fjoin_test", f2.to_str().unwrap()]);
        assert_eq!(code, 1);
    }

    #[test]
    fn test_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        let mut data1 = String::new();
        let mut data2 = String::new();
        for i in 0u64..10_000 {
            data1.push_str(&format!("{:07} alpha_{}\n", i, i));
            data2.push_str(&format!("{:07} beta_{}\n", i, i));
        }
        std::fs::write(&f1, &data1).unwrap();
        std::fs::write(&f2, &data2).unwrap();
        let (out, _, code) = run_fjoin(&[f1.to_str().unwrap(), f2.to_str().unwrap()]);
        assert_eq!(code, 0);
        let binding = String::from_utf8_lossy(&out);
        let lines: Vec<&str> = binding.lines().collect();
        assert_eq!(lines.len(), 10_000);
    }

    // GNU comparison tests
    #[cfg(target_os = "linux")]
    mod gnu_compat {
        use super::*;

        fn run_gnu_join(args: &[&str]) -> Option<(Vec<u8>, i32)> {
            let output = Command::new("join").args(args).output().ok()?;
            Some((output.stdout, output.status.code().unwrap_or(1)))
        }

        #[test]
        fn test_gnu_basic() {
            let dir = tempfile::tempdir().unwrap();
            let f1 = dir.path().join("a.txt");
            let f2 = dir.path().join("b.txt");
            std::fs::write(&f1, "a 1\nb 2\nc 3\n").unwrap();
            std::fs::write(&f2, "a x\nb y\nc z\n").unwrap();
            let args = [f1.to_str().unwrap(), f2.to_str().unwrap()];

            let (our_out, _, our_code) = run_fjoin(&args);
            if let Some((gnu_out, gnu_code)) = run_gnu_join(&args) {
                assert_eq!(our_out, gnu_out, "Output differs from GNU join");
                assert_eq!(our_code, gnu_code, "Exit code differs");
            }
        }

        #[test]
        fn test_gnu_partial_match() {
            let dir = tempfile::tempdir().unwrap();
            let f1 = dir.path().join("a.txt");
            let f2 = dir.path().join("b.txt");
            std::fs::write(&f1, "a 1\nb 2\nc 3\n").unwrap();
            std::fs::write(&f2, "b x\nc y\nd z\n").unwrap();
            let args = [f1.to_str().unwrap(), f2.to_str().unwrap()];

            let (our_out, _, _) = run_fjoin(&args);
            if let Some((gnu_out, _)) = run_gnu_join(&args) {
                assert_eq!(our_out, gnu_out, "Output differs from GNU join (partial)");
            }
        }

        #[test]
        fn test_gnu_unpaired() {
            let dir = tempfile::tempdir().unwrap();
            let f1 = dir.path().join("a.txt");
            let f2 = dir.path().join("b.txt");
            std::fs::write(&f1, "a 1\nb 2\nc 3\n").unwrap();
            std::fs::write(&f2, "b x\n").unwrap();
            let args = ["-a", "1", f1.to_str().unwrap(), f2.to_str().unwrap()];

            let (our_out, _, _) = run_fjoin(&args);
            if let Some((gnu_out, _)) = run_gnu_join(&args) {
                assert_eq!(our_out, gnu_out, "Output differs from GNU join -a 1");
            }
        }

        #[test]
        fn test_gnu_custom_separator() {
            let dir = tempfile::tempdir().unwrap();
            let f1 = dir.path().join("a.txt");
            let f2 = dir.path().join("b.txt");
            std::fs::write(&f1, "a:1\nb:2\n").unwrap();
            std::fs::write(&f2, "a:x\nb:y\n").unwrap();
            let args = ["-t", ":", f1.to_str().unwrap(), f2.to_str().unwrap()];

            let (our_out, _, _) = run_fjoin(&args);
            if let Some((gnu_out, _)) = run_gnu_join(&args) {
                assert_eq!(our_out, gnu_out, "Output differs from GNU join -t :");
            }
        }

        #[test]
        fn test_gnu_many_to_many() {
            let dir = tempfile::tempdir().unwrap();
            let f1 = dir.path().join("a.txt");
            let f2 = dir.path().join("b.txt");
            std::fs::write(&f1, "a 1\na 2\n").unwrap();
            std::fs::write(&f2, "a x\na y\n").unwrap();
            let args = [f1.to_str().unwrap(), f2.to_str().unwrap()];

            let (our_out, _, _) = run_fjoin(&args);
            if let Some((gnu_out, _)) = run_gnu_join(&args) {
                assert_eq!(
                    our_out, gnu_out,
                    "Output differs from GNU join (many-to-many)"
                );
            }
        }
    }
}
