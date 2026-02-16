use super::*;

#[test]
fn test_factor_small_primes() {
    assert_eq!(factorize(2), vec![2]);
    assert_eq!(factorize(3), vec![3]);
    assert_eq!(factorize(5), vec![5]);
    assert_eq!(factorize(7), vec![7]);
    assert_eq!(factorize(11), vec![11]);
    assert_eq!(factorize(13), vec![13]);
}

#[test]
fn test_factor_composite() {
    assert_eq!(format_factors(12), "12: 2 2 3");
    assert_eq!(factorize(12), vec![2, 2, 3]);
    assert_eq!(factorize(6), vec![2, 3]);
    assert_eq!(factorize(30), vec![2, 3, 5]);
    assert_eq!(factorize(100), vec![2, 2, 5, 5]);
    assert_eq!(factorize(360), vec![2, 2, 2, 3, 3, 5]);
}

#[test]
fn test_factor_large_prime() {
    // 999999999989 is prime
    assert_eq!(factorize(999999999989), vec![999999999989]);
}

#[test]
fn test_factor_one() {
    assert_eq!(format_factors(1), "1:");
    assert_eq!(factorize(1), Vec::<u128>::new());
}

#[test]
fn test_factor_zero() {
    assert_eq!(factorize(0), Vec::<u128>::new());
}

#[test]
fn test_factor_power_of_two() {
    assert_eq!(format_factors(1024), "1024: 2 2 2 2 2 2 2 2 2 2");
    assert_eq!(factorize(1024), vec![2; 10]);
    assert_eq!(factorize(65536), vec![2; 16]);
}

#[test]
fn test_factor_large_composite() {
    // 2^31 - 1 = 2147483647 is a Mersenne prime
    assert_eq!(factorize(2147483647), vec![2147483647]);

    // Product of two primes
    // 999961 * 999979 = 999940000819
    assert_eq!(factorize(999940000819), vec![999961, 999979]);
}

#[test]
fn test_factor_perfect_squares() {
    assert_eq!(factorize(49), vec![7, 7]);
    assert_eq!(factorize(121), vec![11, 11]);
    assert_eq!(factorize(169), vec![13, 13]);
}

#[test]
fn test_factor_powers_of_primes() {
    // 3^10 = 59049
    assert_eq!(factorize(59049), vec![3; 10]);
    // 7^5 = 16807
    assert_eq!(factorize(16807), vec![7; 5]);
}

#[test]
fn test_factor_format_output() {
    assert_eq!(format_factors(2), "2: 2");
    assert_eq!(format_factors(12), "12: 2 2 3");
    assert_eq!(format_factors(1), "1:");
    assert_eq!(format_factors(97), "97: 97");
}

#[test]
fn test_factor_very_large() {
    // 2^64 = 18446744073709551616
    let n: u128 = 1 << 64;
    let factors = factorize(n);
    assert_eq!(factors, vec![2; 64]);
}

#[test]
fn test_factor_large_semiprime() {
    // Product of two large primes: 1000003 * 1000033 = 1000036000099
    assert_eq!(factorize(1000036000099), vec![1000003, 1000033]);
}

#[test]
fn test_factor_sorted_output() {
    // Verify factors always come back sorted
    let factors = factorize(2 * 3 * 5 * 7 * 11 * 13 * 17 * 19 * 23);
    assert_eq!(factors, vec![2, 3, 5, 7, 11, 13, 17, 19, 23]);
}

// Integration tests using the binary
#[cfg(test)]
mod integration {
    use std::io::Write;
    use std::process::Command;

    fn bin_path() -> std::path::PathBuf {
        let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("target");
        if cfg!(debug_assertions) {
            path.push("debug");
        } else {
            path.push("release");
        }
        path.push("ffactor");
        path
    }

    fn run_ffactor(args: &[&str]) -> (String, String, i32) {
        let output = Command::new(bin_path())
            .args(args)
            .output()
            .expect("failed to spawn ffactor");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let code = output.status.code().unwrap_or(1);
        (stdout, stderr, code)
    }

    fn run_ffactor_stdin(input: &str) -> (String, String, i32) {
        let mut child = Command::new(bin_path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn ffactor");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().expect("failed to wait");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let code = output.status.code().unwrap_or(1);
        (stdout, stderr, code)
    }

    #[test]
    fn test_factor_multiple_args() {
        let (stdout, _, code) = run_ffactor(&["12", "15", "100"]);
        assert_eq!(code, 0);
        let lines: Vec<&str> = stdout.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "12: 2 2 3");
        assert_eq!(lines[1], "15: 3 5");
        assert_eq!(lines[2], "100: 2 2 5 5");
    }

    #[test]
    fn test_factor_stdin() {
        let (stdout, _, code) = run_ffactor_stdin("12\n15\n100\n");
        assert_eq!(code, 0);
        let lines: Vec<&str> = stdout.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "12: 2 2 3");
        assert_eq!(lines[1], "15: 3 5");
        assert_eq!(lines[2], "100: 2 2 5 5");
    }

    #[test]
    fn test_factor_invalid_input() {
        let (stdout, stderr, code) = run_ffactor(&["12", "abc", "15"]);
        // Should still output the valid ones and error on invalid
        assert!(stdout.contains("12: 2 2 3"));
        assert!(stdout.contains("15: 3 5"));
        assert!(stderr.contains("abc"));
        assert_eq!(code, 1);
    }

    #[test]
    fn test_factor_one_arg() {
        let (stdout, _, code) = run_ffactor(&["1"]);
        assert_eq!(code, 0);
        assert_eq!(stdout.trim(), "1:");
    }

    #[test]
    fn test_factor_help() {
        let (stdout, _, code) = run_ffactor(&["--help"]);
        assert_eq!(code, 0);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("factor"));
    }

    #[test]
    fn test_factor_version() {
        let (stdout, _, code) = run_ffactor(&["--version"]);
        assert_eq!(code, 0);
        assert!(stdout.contains("factor"));
        assert!(stdout.contains("fcoreutils"));
    }

    /// Compare output with GNU factor if available.
    #[test]
    fn test_factor_matches_gnu() {
        let gnu = Command::new("factor")
            .args(["12", "1", "1024", "97", "999999999989"])
            .output();
        let (ours, _, code) = run_ffactor(&["12", "1", "1024", "97", "999999999989"]);
        assert_eq!(code, 0);

        if let Ok(gnu_output) = gnu {
            if gnu_output.status.success() {
                let gnu_stdout = String::from_utf8_lossy(&gnu_output.stdout);
                assert_eq!(
                    ours.trim(),
                    gnu_stdout.trim(),
                    "Output differs from GNU factor"
                );
            }
        }
    }
}
