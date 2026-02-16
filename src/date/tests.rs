use super::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Helper: create a fixed SystemTime for reproducible tests.
fn fixed_time() -> SystemTime {
    // 2024-01-15 10:30:00 UTC = 1705314600
    UNIX_EPOCH + Duration::from_secs(1_705_314_600)
}

#[test]
fn test_date_runs() {
    // Verify that format_date doesn't panic with basic inputs.
    let now = SystemTime::now();
    let result = format_date(&now, default_format(), false);
    assert!(!result.is_empty(), "Default format should produce non-empty output");
}

#[test]
fn test_date_format() {
    let time = fixed_time();
    let result = format_date(&time, "%Y-%m-%d", true);
    assert_eq!(result, "2024-01-15");
}

#[test]
fn test_date_utc() {
    let time = fixed_time();
    // UTC hour should be 10
    let result = format_date(&time, "%H", true);
    assert_eq!(result, "10");
}

#[test]
fn test_date_iso() {
    let time = fixed_time();
    let result = format_iso(&time, &IsoFormat::Date, true);
    assert_eq!(result, "2024-01-15");

    let result_sec = format_iso(&time, &IsoFormat::Seconds, true);
    assert!(
        result_sec.starts_with("2024-01-15T10:30:00"),
        "ISO seconds should start with date and time: '{}'",
        result_sec
    );
    assert!(
        result_sec.contains("+00:00"),
        "UTC ISO should contain +00:00: '{}'",
        result_sec
    );
}

#[test]
fn test_date_rfc_email() {
    let time = fixed_time();
    let result = format_rfc_email(&time, true);
    // Should be like: "Mon, 15 Jan 2024 10:30:00 +0000"
    assert!(
        result.contains("15 Jan 2024"),
        "RFC email should contain date: '{}'",
        result
    );
    assert!(
        result.contains("10:30:00"),
        "RFC email should contain time: '{}'",
        result
    );
    assert!(
        result.contains("+0000"),
        "UTC RFC email should contain +0000: '{}'",
        result
    );
}

#[test]
fn test_date_rfc_3339() {
    let time = fixed_time();
    let result = format_rfc3339(&time, &Rfc3339Format::Date, true);
    assert_eq!(result, "2024-01-15");

    let result_sec = format_rfc3339(&time, &Rfc3339Format::Seconds, true);
    assert!(
        result_sec.starts_with("2024-01-15 10:30:00"),
        "RFC 3339 seconds should start with date and time: '{}'",
        result_sec
    );
}

#[test]
fn test_date_reference() {
    // Test that file_mod_time works for an existing file.
    // We use Cargo.toml which should always exist in our project.
    let result = file_mod_time("Cargo.toml");
    assert!(result.is_ok(), "Should read mod time of Cargo.toml");

    let mod_time = result.unwrap();
    // The mod time should be after the Unix epoch
    assert!(
        mod_time > UNIX_EPOCH,
        "Mod time should be after epoch"
    );
}

#[test]
fn test_date_epoch() {
    let time = fixed_time();
    let result = format_date(&time, "%s", true);
    assert_eq!(result, "1705314600");
}

#[test]
fn test_date_matches_gnu_format() {
    let time = fixed_time();

    // Test various format specifiers that should match GNU date output
    // %Y = 4-digit year
    assert_eq!(format_date(&time, "%Y", true), "2024");
    // %m = month (01-12)
    assert_eq!(format_date(&time, "%m", true), "01");
    // %d = day (01-31)
    assert_eq!(format_date(&time, "%d", true), "15");
    // %H = hour (00-23)
    assert_eq!(format_date(&time, "%H", true), "10");
    // %M = minute (00-59)
    assert_eq!(format_date(&time, "%M", true), "30");
    // %S = second (00-59)
    assert_eq!(format_date(&time, "%S", true), "00");
    // %F = %Y-%m-%d
    assert_eq!(format_date(&time, "%F", true), "2024-01-15");
    // %T = %H:%M:%S
    assert_eq!(format_date(&time, "%T", true), "10:30:00");
    // %D = %m/%d/%y
    assert_eq!(format_date(&time, "%D", true), "01/15/24");
    // %R = %H:%M
    assert_eq!(format_date(&time, "%R", true), "10:30");

    // %N = nanoseconds (should be 000000000 for our fixed time)
    let result_n = format_date(&time, "%N", true);
    assert_eq!(result_n, "000000000");

    // %n = newline
    assert_eq!(format_date(&time, "%n", true), "\n");

    // %t = tab
    assert_eq!(format_date(&time, "%t", true), "\t");

    // %% = literal %
    assert_eq!(format_date(&time, "%%", true), "%");
}

#[test]
fn test_parse_date_string_epoch() {
    let result = parse_date_string("@0").unwrap();
    assert_eq!(result, UNIX_EPOCH);

    let result = parse_date_string("@1705314600").unwrap();
    assert_eq!(result, fixed_time());
}

#[test]
fn test_parse_date_string_relative() {
    let now = SystemTime::now();

    let yesterday = parse_date_string("yesterday").unwrap();
    let expected = now - Duration::from_secs(86400);
    // Allow 2 second tolerance for timing
    let diff = yesterday
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .abs_diff(
            expected
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
    assert!(diff <= 2, "yesterday should be ~24h ago, diff: {}", diff);

    let tomorrow = parse_date_string("tomorrow").unwrap();
    let expected = now + Duration::from_secs(86400);
    let diff = tomorrow
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .abs_diff(
            expected
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
    assert!(diff <= 2, "tomorrow should be ~24h from now, diff: {}", diff);
}

#[test]
fn test_parse_date_string_iso() {
    let result = parse_date_string("2024-01-15 10:30:00").unwrap();
    // We can't easily check exact value since mktime uses local timezone,
    // but it should at least parse successfully
    assert!(result > UNIX_EPOCH, "Parsed date should be after epoch");
}

#[test]
fn test_parse_date_string_relative_offset() {
    let result = parse_date_string("1 day ago");
    assert!(result.is_ok(), "Should parse '1 day ago'");

    let result = parse_date_string("2 hours ago");
    assert!(result.is_ok(), "Should parse '2 hours ago'");
}

#[test]
fn test_iso_format_parse() {
    assert_eq!(parse_iso_format("date").unwrap(), IsoFormat::Date);
    assert_eq!(parse_iso_format("hours").unwrap(), IsoFormat::Hours);
    assert_eq!(parse_iso_format("minutes").unwrap(), IsoFormat::Minutes);
    assert_eq!(parse_iso_format("seconds").unwrap(), IsoFormat::Seconds);
    assert_eq!(parse_iso_format("ns").unwrap(), IsoFormat::Ns);
    assert!(parse_iso_format("invalid").is_err());
}

#[test]
fn test_rfc3339_format_parse() {
    assert_eq!(parse_rfc3339_format("date").unwrap(), Rfc3339Format::Date);
    assert_eq!(
        parse_rfc3339_format("seconds").unwrap(),
        Rfc3339Format::Seconds
    );
    assert_eq!(parse_rfc3339_format("ns").unwrap(), Rfc3339Format::Ns);
    assert!(parse_rfc3339_format("invalid").is_err());
}
