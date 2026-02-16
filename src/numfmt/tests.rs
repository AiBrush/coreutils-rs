use super::*;

// ──────────────────────────────────────────────────
// Helper: process a single line with the given config
// ──────────────────────────────────────────────────

fn default_config() -> NumfmtConfig {
    NumfmtConfig::default()
}

// ──────────────────────────────────────────────────
// SI → None conversion tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_si_to_none() {
    let mut config = default_config();
    config.from = ScaleUnit::Si;
    config.to = ScaleUnit::None;

    let result = process_line("1K", &config).unwrap();
    assert_eq!(result, "1000");

    let result = process_line("1M", &config).unwrap();
    assert_eq!(result, "1000000");

    let result = process_line("1G", &config).unwrap();
    assert_eq!(result, "1000000000");

    let result = process_line("2.5K", &config).unwrap();
    assert_eq!(result, "2500");

    let result = process_line("1T", &config).unwrap();
    assert_eq!(result, "1000000000000");
}

// ──────────────────────────────────────────────────
// None → SI conversion tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_none_to_si() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::Si;

    let result = process_line("1000", &config).unwrap();
    assert_eq!(result, "1.0K");

    let result = process_line("1000000", &config).unwrap();
    assert_eq!(result, "1.0M");

    let result = process_line("1500", &config).unwrap();
    assert_eq!(result, "1.5K");

    let result = process_line("500", &config).unwrap();
    assert_eq!(result, "500");

    let result = process_line("0", &config).unwrap();
    assert_eq!(result, "0");
}

// ──────────────────────────────────────────────────
// IEC conversion tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_iec() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::Iec;

    let result = process_line("1024", &config).unwrap();
    assert_eq!(result, "1.0K");

    let result = process_line("1048576", &config).unwrap();
    assert_eq!(result, "1.0M");

    let result = process_line("1536", &config).unwrap();
    assert_eq!(result, "1.5K");

    let result = process_line("500", &config).unwrap();
    assert_eq!(result, "500");
}

#[test]
fn test_numfmt_iec_i() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::IecI;

    let result = process_line("1024", &config).unwrap();
    assert_eq!(result, "1.0Ki");

    let result = process_line("1048576", &config).unwrap();
    assert_eq!(result, "1.0Mi");
}

#[test]
fn test_numfmt_from_iec_i() {
    let mut config = default_config();
    config.from = ScaleUnit::IecI;
    config.to = ScaleUnit::None;

    let result = process_line("1Ki", &config).unwrap();
    assert_eq!(result, "1024");

    let result = process_line("1Mi", &config).unwrap();
    assert_eq!(result, "1048576");
}

// ──────────────────────────────────────────────────
// Auto detection tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_auto() {
    let mut config = default_config();
    config.from = ScaleUnit::Auto;
    config.to = ScaleUnit::None;

    // SI suffix (no 'i')
    let result = process_line("1K", &config).unwrap();
    assert_eq!(result, "1000");

    // IEC suffix (with 'i')
    let result = process_line("1Ki", &config).unwrap();
    assert_eq!(result, "1024");
}

// ──────────────────────────────────────────────────
// Padding tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_padding() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.padding = Some(10);

    let result = process_line("42", &config).unwrap();
    assert_eq!(result, "        42");
    assert_eq!(result.len(), 10);
}

#[test]
fn test_numfmt_padding_left_align() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.padding = Some(-10);

    let result = process_line("42", &config).unwrap();
    assert_eq!(result, "42        ");
    assert_eq!(result.len(), 10);
}

// ──────────────────────────────────────────────────
// Format tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_format() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.format = Some("%.2f".to_string());

    let result = process_line("1000", &config).unwrap();
    assert_eq!(result, "1000.00");
}

#[test]
fn test_numfmt_format_width() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.format = Some("%10.2f".to_string());

    let result = process_line("42", &config).unwrap();
    assert_eq!(result, "     42.00");
}

// ──────────────────────────────────────────────────
// Field selection tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_field() {
    let mut config = default_config();
    config.from = ScaleUnit::Si;
    config.to = ScaleUnit::None;
    config.field = vec![2];
    config.delimiter = Some(',');

    let result = process_line("foo,1K,bar", &config).unwrap();
    assert_eq!(result, "foo,1000,bar");
}

#[test]
fn test_numfmt_field_whitespace() {
    let mut config = default_config();
    config.from = ScaleUnit::Si;
    config.to = ScaleUnit::None;
    config.field = vec![2];

    let result = process_line("foo 1K bar", &config).unwrap();
    assert_eq!(result, "foo 1000 bar");
}

#[test]
fn test_numfmt_multiple_fields() {
    let mut config = default_config();
    config.from = ScaleUnit::Si;
    config.to = ScaleUnit::None;
    config.field = vec![1, 3];
    config.delimiter = Some(',');

    let result = process_line("1K,foo,2M", &config).unwrap();
    assert_eq!(result, "1000,foo,2000000");
}

// ──────────────────────────────────────────────────
// Header tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_header() {
    let config = NumfmtConfig {
        from: ScaleUnit::Si,
        to: ScaleUnit::None,
        header: 1,
        ..default_config()
    };

    let input = "Size\n1K\n2M\n";
    let mut output = Vec::new();
    run_numfmt(input.as_bytes(), &mut output, &config).unwrap();
    let result = String::from_utf8(output).unwrap();
    assert_eq!(result, "Size\n1000\n2000000\n");
}

#[test]
fn test_numfmt_header_multiple() {
    let config = NumfmtConfig {
        from: ScaleUnit::Si,
        to: ScaleUnit::None,
        header: 2,
        ..default_config()
    };

    let input = "Header1\nHeader2\n1K\n";
    let mut output = Vec::new();
    run_numfmt(input.as_bytes(), &mut output, &config).unwrap();
    let result = String::from_utf8(output).unwrap();
    assert_eq!(result, "Header1\nHeader2\n1000\n");
}

// ──────────────────────────────────────────────────
// Rounding tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_round() {
    // Test --round=up with --to=si
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::Si;
    config.round = RoundMethod::Up;

    let result = process_line("1001", &config).unwrap();
    // 1001/1000 = 1.001, rounded up to 1 decimal => 1.1
    assert_eq!(result, "1.1K");
}

#[test]
fn test_numfmt_round_down() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::Si;
    config.round = RoundMethod::Down;

    let result = process_line("1999", &config).unwrap();
    // 1999/1000 = 1.999, rounded down to 1 decimal => 1.9
    assert_eq!(result, "1.9K");
}

#[test]
fn test_numfmt_round_nearest() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::Si;
    config.round = RoundMethod::Nearest;

    let result = process_line("1450", &config).unwrap();
    // 1450/1000 = 1.45, rounded nearest to 1 decimal => 1.5
    assert_eq!(result, "1.5K");
}

#[test]
fn test_numfmt_round_towards_zero() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.round = RoundMethod::TowardsZero;

    let result = process_line("1999", &config).unwrap();
    assert_eq!(result, "1999");
}

#[test]
fn test_numfmt_round_from_zero() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.round = RoundMethod::FromZero;

    let result = process_line("1999", &config).unwrap();
    assert_eq!(result, "1999");
}

// ──────────────────────────────────────────────────
// Grouping tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_grouping() {
    let mut config = default_config();
    config.from = ScaleUnit::Si;
    config.to = ScaleUnit::None;
    config.grouping = true;

    let result = process_line("1M", &config).unwrap();
    assert_eq!(result, "1,000,000");
}

// ──────────────────────────────────────────────────
// Suffix tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_suffix() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.suffix = Some("B".to_string());

    let result = process_line("1024", &config).unwrap();
    assert_eq!(result, "1024B");
}

// ──────────────────────────────────────────────────
// From-unit / to-unit scaling tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_from_unit() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.from_unit = 1024.0;

    let result = process_line("1", &config).unwrap();
    assert_eq!(result, "1024");
}

#[test]
fn test_numfmt_to_unit() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.to_unit = 1024.0;

    let result = process_line("1024", &config).unwrap();
    assert_eq!(result, "1");
}

// ──────────────────────────────────────────────────
// Invalid mode tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_invalid_abort() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.invalid = InvalidMode::Abort;

    let result = process_line("notanumber", &config);
    assert!(result.is_err());
}

#[test]
fn test_numfmt_invalid_ignore() {
    let mut config = default_config();
    config.from = ScaleUnit::None;
    config.to = ScaleUnit::None;
    config.invalid = InvalidMode::Ignore;

    let result = process_line("notanumber", &config).unwrap();
    assert_eq!(result, "notanumber");
}

// ──────────────────────────────────────────────────
// Zero terminated tests
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_zero_terminated() {
    let config = NumfmtConfig {
        from: ScaleUnit::Si,
        to: ScaleUnit::None,
        zero_terminated: true,
        ..default_config()
    };

    let input = "1K\02M\0";
    let mut output = Vec::new();
    run_numfmt(input.as_bytes(), &mut output, &config).unwrap();
    let result = String::from_utf8(output).unwrap();
    assert_eq!(result, "1000\02000000\0");
}

// ──────────────────────────────────────────────────
// Parse helpers tests
// ──────────────────────────────────────────────────

#[test]
fn test_parse_scale_unit() {
    assert_eq!(parse_scale_unit("none").unwrap(), ScaleUnit::None);
    assert_eq!(parse_scale_unit("si").unwrap(), ScaleUnit::Si);
    assert_eq!(parse_scale_unit("iec").unwrap(), ScaleUnit::Iec);
    assert_eq!(parse_scale_unit("iec-i").unwrap(), ScaleUnit::IecI);
    assert_eq!(parse_scale_unit("auto").unwrap(), ScaleUnit::Auto);
    assert!(parse_scale_unit("invalid").is_err());
}

#[test]
fn test_parse_round_method() {
    assert_eq!(parse_round_method("up").unwrap(), RoundMethod::Up);
    assert_eq!(parse_round_method("down").unwrap(), RoundMethod::Down);
    assert_eq!(
        parse_round_method("from-zero").unwrap(),
        RoundMethod::FromZero
    );
    assert_eq!(
        parse_round_method("towards-zero").unwrap(),
        RoundMethod::TowardsZero
    );
    assert_eq!(parse_round_method("nearest").unwrap(), RoundMethod::Nearest);
    assert!(parse_round_method("invalid").is_err());
}

#[test]
fn test_parse_invalid_mode() {
    assert_eq!(parse_invalid_mode("abort").unwrap(), InvalidMode::Abort);
    assert_eq!(parse_invalid_mode("fail").unwrap(), InvalidMode::Fail);
    assert_eq!(parse_invalid_mode("warn").unwrap(), InvalidMode::Warn);
    assert_eq!(parse_invalid_mode("ignore").unwrap(), InvalidMode::Ignore);
    assert!(parse_invalid_mode("invalid").is_err());
}

#[test]
fn test_parse_fields() {
    assert_eq!(parse_fields("1").unwrap(), vec![1]);
    assert_eq!(parse_fields("2").unwrap(), vec![2]);
    assert_eq!(parse_fields("1,3").unwrap(), vec![1, 3]);
    assert_eq!(parse_fields("1,2,3").unwrap(), vec![1, 2, 3]);
    // All fields
    assert_eq!(parse_fields("-").unwrap(), Vec::<usize>::new());
    // Ranges
    assert_eq!(parse_fields("1-3").unwrap(), vec![1, 2, 3]);
    // Field 0 should error
    assert!(parse_fields("0").is_err());
}

// ──────────────────────────────────────────────────
// run_numfmt integration tests
// ──────────────────────────────────────────────────

#[test]
fn test_run_numfmt_basic() {
    let config = NumfmtConfig {
        from: ScaleUnit::Si,
        to: ScaleUnit::None,
        ..default_config()
    };

    let input = "1K\n2M\n3G\n";
    let mut output = Vec::new();
    run_numfmt(input.as_bytes(), &mut output, &config).unwrap();
    let result = String::from_utf8(output).unwrap();
    assert_eq!(result, "1000\n2000000\n3000000000\n");
}

#[test]
fn test_run_numfmt_to_si() {
    let config = NumfmtConfig {
        from: ScaleUnit::None,
        to: ScaleUnit::Si,
        ..default_config()
    };

    let input = "1000\n1000000\n";
    let mut output = Vec::new();
    run_numfmt(input.as_bytes(), &mut output, &config).unwrap();
    let result = String::from_utf8(output).unwrap();
    assert_eq!(result, "1.0K\n1.0M\n");
}

// ──────────────────────────────────────────────────
// GNU compatibility test (integration)
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_matches_gnu() {
    // Test that our output matches expected GNU numfmt output for common cases.
    // We test against known expected values rather than running GNU numfmt
    // since it may not be available in all test environments.

    // echo "1K" | numfmt --from=si
    let mut config = default_config();
    config.from = ScaleUnit::Si;
    assert_eq!(process_line("1K", &config).unwrap(), "1000");

    // echo "1000" | numfmt --to=si
    let mut config = default_config();
    config.to = ScaleUnit::Si;
    assert_eq!(process_line("1000", &config).unwrap(), "1.0K");

    // echo "1024" | numfmt --to=iec
    let mut config = default_config();
    config.to = ScaleUnit::Iec;
    assert_eq!(process_line("1024", &config).unwrap(), "1.0K");

    // echo "1024" | numfmt --to=iec-i
    let mut config = default_config();
    config.to = ScaleUnit::IecI;
    assert_eq!(process_line("1024", &config).unwrap(), "1.0Ki");

    // echo "1000000" | numfmt --to=si
    let mut config = default_config();
    config.to = ScaleUnit::Si;
    assert_eq!(process_line("1000000", &config).unwrap(), "1.0M");

    // echo "1048576" | numfmt --to=iec
    let mut config = default_config();
    config.to = ScaleUnit::Iec;
    assert_eq!(process_line("1048576", &config).unwrap(), "1.0M");

    // Large values
    let mut config = default_config();
    config.to = ScaleUnit::Si;
    assert_eq!(
        process_line("1000000000", &config).unwrap(),
        "1.0G"
    );

    // Passthrough when no conversion needed
    let config = default_config();
    assert_eq!(process_line("42", &config).unwrap(), "42");
}

// ──────────────────────────────────────────────────
// Edge cases
// ──────────────────────────────────────────────────

#[test]
fn test_numfmt_zero() {
    let mut config = default_config();
    config.to = ScaleUnit::Si;
    assert_eq!(process_line("0", &config).unwrap(), "0");
}

#[test]
fn test_numfmt_negative() {
    let mut config = default_config();
    config.to = ScaleUnit::Si;

    let result = process_line("-1000", &config).unwrap();
    assert_eq!(result, "-1.0K");
}

#[test]
fn test_numfmt_large_si() {
    let mut config = default_config();
    config.from = ScaleUnit::Si;
    config.to = ScaleUnit::None;

    assert_eq!(process_line("1P", &config).unwrap(), "1000000000000000");
    assert_eq!(
        process_line("1E", &config).unwrap(),
        "1000000000000000000"
    );
}

#[test]
fn test_numfmt_delimiter() {
    let mut config = default_config();
    config.from = ScaleUnit::Si;
    config.to = ScaleUnit::None;
    config.delimiter = Some(':');
    config.field = vec![2];

    let result = process_line("name:1K:info", &config).unwrap();
    assert_eq!(result, "name:1000:info");
}
