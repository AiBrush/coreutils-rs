use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Configuration for the date command.
#[derive(Default)]
pub struct DateConfig {
    /// Display time described by STRING (-d).
    pub date_string: Option<String>,
    /// Read dates from a file, one per line (-f).
    pub date_file: Option<String>,
    /// ISO 8601 output format (-I).
    pub iso_format: Option<IsoFormat>,
    /// RFC 5322 / email format (-R).
    pub rfc_email: bool,
    /// RFC 3339 format.
    pub rfc_3339: Option<Rfc3339Format>,
    /// Show modification time of FILE (-r).
    pub reference_file: Option<String>,
    /// Set system time (-s). We only parse; actual setting requires root.
    pub set_string: Option<String>,
    /// Use UTC (-u).
    pub utc: bool,
    /// Custom format string (starts with +).
    pub format: Option<String>,
}

/// ISO 8601 format precision levels.
#[derive(Clone, Debug, PartialEq)]
pub enum IsoFormat {
    Date,
    Hours,
    Minutes,
    Seconds,
    Ns,
}

/// RFC 3339 format precision levels.
#[derive(Clone, Debug, PartialEq)]
pub enum Rfc3339Format {
    Date,
    Seconds,
    Ns,
}

/// Parse an ISO format precision string.
pub fn parse_iso_format(s: &str) -> Result<IsoFormat, String> {
    match s {
        "" | "date" => Ok(IsoFormat::Date),
        "hours" => Ok(IsoFormat::Hours),
        "minutes" => Ok(IsoFormat::Minutes),
        "seconds" => Ok(IsoFormat::Seconds),
        "ns" => Ok(IsoFormat::Ns),
        _ => Err(format!("invalid ISO 8601 format: '{}'", s)),
    }
}

/// Parse an RFC 3339 format precision string.
pub fn parse_rfc3339_format(s: &str) -> Result<Rfc3339Format, String> {
    match s {
        "date" => Ok(Rfc3339Format::Date),
        "seconds" => Ok(Rfc3339Format::Seconds),
        "ns" => Ok(Rfc3339Format::Ns),
        _ => Err(format!("invalid RFC 3339 format: '{}'", s)),
    }
}

/// Format a `SystemTime` using the given format string.
///
/// Uses libc `strftime` for most specifiers. Handles `%N` (nanoseconds) manually
/// since strftime does not support it.
pub fn format_date(time: &SystemTime, format: &str, utc: bool) -> String {
    let dur = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let nanos = dur.subsec_nanos();

    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if utc {
        unsafe {
            libc::gmtime_r(&secs, &mut tm);
        }
    } else {
        unsafe {
            libc::localtime_r(&secs, &mut tm);
        }
    }

    // Process the format string, handling %N specially and passing
    // everything else through to strftime.
    let mut result = String::with_capacity(format.len() * 2);
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '%' && i + 1 < chars.len() {
            match chars[i + 1] {
                'N' => {
                    // Nanoseconds (9 digits, zero-padded)
                    result.push_str(&format!("{:09}", nanos));
                    i += 2;
                }
                'q' => {
                    // Quarter (1-4), not in standard strftime
                    let month = tm.tm_mon; // 0-11
                    let quarter = (month / 3) + 1;
                    result.push_str(&quarter.to_string());
                    i += 2;
                }
                'P' => {
                    // am/pm (lowercase), not always available in strftime
                    let ampm = if tm.tm_hour < 12 { "am" } else { "pm" };
                    result.push_str(ampm);
                    i += 2;
                }
                'n' => {
                    result.push('\n');
                    i += 2;
                }
                't' => {
                    result.push('\t');
                    i += 2;
                }
                _ => {
                    // Pass this specifier to strftime
                    let spec = format!("%{}", chars[i + 1]);
                    let formatted = strftime_single(&tm, &spec);
                    result.push_str(&formatted);
                    i += 2;
                }
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Call libc strftime for a single format specifier.
fn strftime_single(tm: &libc::tm, fmt: &str) -> String {
    let c_fmt = match std::ffi::CString::new(fmt) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let mut buf = vec![0u8; 128];
    let len = unsafe {
        libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            c_fmt.as_ptr(),
            tm,
        )
    };
    if len == 0 && !fmt.is_empty() && fmt != "%%" {
        // strftime returns 0 on error or if the result is empty string
        // For %%, it legitimately returns "%"
        return String::new();
    }
    buf.truncate(len);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Format a SystemTime in ISO 8601 format.
pub fn format_iso(time: &SystemTime, precision: &IsoFormat, utc: bool) -> String {
    match precision {
        IsoFormat::Date => format_date(time, "%Y-%m-%d", utc),
        IsoFormat::Hours => {
            let date_part = format_date(time, "%Y-%m-%dT%H", utc);
            let tz = format_timezone_colon(time, utc);
            format!("{}{}", date_part, tz)
        }
        IsoFormat::Minutes => {
            let date_part = format_date(time, "%Y-%m-%dT%H:%M", utc);
            let tz = format_timezone_colon(time, utc);
            format!("{}{}", date_part, tz)
        }
        IsoFormat::Seconds => {
            let date_part = format_date(time, "%Y-%m-%dT%H:%M:%S", utc);
            let tz = format_timezone_colon(time, utc);
            format!("{}{}", date_part, tz)
        }
        IsoFormat::Ns => {
            let dur = time.duration_since(UNIX_EPOCH).unwrap_or_default();
            let nanos = dur.subsec_nanos();
            let date_part = format_date(time, "%Y-%m-%dT%H:%M:%S", utc);
            let tz = format_timezone_colon(time, utc);
            format!("{},{:09}{}", date_part, nanos, tz)
        }
    }
}

/// Format a SystemTime in RFC 5322 (email) format.
pub fn format_rfc_email(time: &SystemTime, utc: bool) -> String {
    format_date(time, "%a, %d %b %Y %H:%M:%S %z", utc)
}

/// Format a SystemTime in RFC 3339 format.
pub fn format_rfc3339(time: &SystemTime, precision: &Rfc3339Format, utc: bool) -> String {
    match precision {
        Rfc3339Format::Date => format_date(time, "%Y-%m-%d", utc),
        Rfc3339Format::Seconds => {
            let date_part = format_date(time, "%Y-%m-%d %H:%M:%S", utc);
            let tz = format_timezone_colon(time, utc);
            format!("{}{}", date_part, tz)
        }
        Rfc3339Format::Ns => {
            let dur = time.duration_since(UNIX_EPOCH).unwrap_or_default();
            let nanos = dur.subsec_nanos();
            let date_part = format_date(time, "%Y-%m-%d %H:%M:%S", utc);
            let tz = format_timezone_colon(time, utc);
            format!("{}.{:09}{}", date_part, nanos, tz)
        }
    }
}

/// Format a timezone offset with a colon (e.g., +05:30).
fn format_timezone_colon(time: &SystemTime, utc: bool) -> String {
    if utc {
        return "+00:00".to_string();
    }
    let raw = format_date(time, "%z", false);
    // raw is like "+0530" or "-0800"
    if raw.len() >= 5 {
        format!("{}:{}", &raw[..3], &raw[3..5])
    } else {
        raw
    }
}

/// Parse a date string into a SystemTime.
///
/// Supports:
/// - ISO format: "2024-01-15 10:30:00", "2024-01-15T10:30:00"
/// - Relative: "yesterday", "tomorrow", "now", "today"
/// - Relative offset: "1 day ago", "2 hours ago", "3 days", "+1 week"
/// - Epoch: "@SECONDS"
pub fn parse_date_string(s: &str) -> Result<SystemTime, String> {
    let s = s.trim();

    // Handle epoch format: @SECONDS
    if let Some(epoch_str) = s.strip_prefix('@') {
        let secs: i64 = epoch_str
            .trim()
            .parse()
            .map_err(|_| format!("invalid date '@{}'", epoch_str))?;
        if secs >= 0 {
            return Ok(UNIX_EPOCH + Duration::from_secs(secs as u64));
        } else {
            return Ok(UNIX_EPOCH - Duration::from_secs((-secs) as u64));
        }
    }

    let now = SystemTime::now();

    // Handle relative words
    match s.to_lowercase().as_str() {
        "now" | "today" => return Ok(now),
        "yesterday" => {
            return Ok(now - Duration::from_secs(86400));
        }
        "tomorrow" => {
            return Ok(now + Duration::from_secs(86400));
        }
        _ => {}
    }

    // Handle relative offsets: "N unit ago", "N unit", "+N unit"
    if let Some(result) = try_parse_relative(s, &now) {
        return Ok(result);
    }

    // Try ISO-like format: "YYYY-MM-DD[ HH:MM[:SS]]"
    if let Some(result) = try_parse_iso(s) {
        return Ok(result);
    }

    Err(format!("invalid date '{}'", s))
}

/// Try to parse a relative time expression.
fn try_parse_relative(s: &str, now: &SystemTime) -> Option<SystemTime> {
    let lower = s.to_lowercase();
    let parts: Vec<&str> = lower.split_whitespace().collect();

    if parts.len() < 2 {
        return None;
    }

    let is_ago = parts.last().map_or(false, |&p| p == "ago");
    let num_str = parts[0].trim_start_matches('+');
    let amount: i64 = num_str.parse().ok()?;

    let unit_idx = 1;
    if unit_idx >= parts.len() {
        return None;
    }
    let unit = parts[unit_idx];

    let seconds = match unit.trim_end_matches('s') {
        "second" => amount,
        "minute" => amount * 60,
        "hour" => amount * 3600,
        "day" => amount * 86400,
        "week" => amount * 86400 * 7,
        "month" => amount * 86400 * 30,
        "year" => amount * 86400 * 365,
        _ => return None,
    };

    let duration = Duration::from_secs(seconds.unsigned_abs());
    if is_ago || seconds < 0 {
        Some(*now - duration)
    } else {
        Some(*now + duration)
    }
}

/// Try to parse an ISO-like date string.
fn try_parse_iso(s: &str) -> Option<SystemTime> {
    // Split on space or T
    let s = s.replace('T', " ");
    let parts: Vec<&str> = s.splitn(2, ' ').collect();
    let date_part = parts[0];
    let time_part = if parts.len() > 1 { parts[1] } else { "00:00:00" };

    let date_fields: Vec<&str> = date_part.split('-').collect();
    if date_fields.len() != 3 {
        return None;
    }

    let year: i32 = date_fields[0].parse().ok()?;
    let month: u32 = date_fields[1].parse().ok()?;
    let day: u32 = date_fields[2].parse().ok()?;

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Parse time (strip timezone info for simplicity)
    let time_clean = time_part
        .split('+')
        .next()
        .unwrap_or(time_part)
        .split('Z')
        .next()
        .unwrap_or(time_part);
    let time_fields: Vec<&str> = time_clean.split(':').collect();
    let hour: u32 = time_fields.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minute: u32 = time_fields.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let second: u32 = time_fields
        .get(2)
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    // Convert to Unix timestamp using libc mktime
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month as i32 - 1;
    tm.tm_mday = day as i32;
    tm.tm_hour = hour as i32;
    tm.tm_min = minute as i32;
    tm.tm_sec = second as i32;
    tm.tm_isdst = -1; // Let mktime determine DST

    let epoch_secs = unsafe { libc::mktime(&mut tm) };
    if epoch_secs == -1 {
        return None;
    }

    if epoch_secs >= 0 {
        Some(UNIX_EPOCH + Duration::from_secs(epoch_secs as u64))
    } else {
        Some(UNIX_EPOCH - Duration::from_secs((-epoch_secs) as u64))
    }
}

/// Get the modification time of a file.
pub fn file_mod_time(path: &str) -> Result<SystemTime, String> {
    std::fs::metadata(path)
        .map_err(|e| format!("{}: {}", path, e))?
        .modified()
        .map_err(|e| format!("{}: {}", path, e))
}

/// Get the default date format (matches GNU date default output).
pub fn default_format() -> &'static str {
    "%a %b %e %H:%M:%S %Z %Y"
}
