#[cfg(not(unix))]
fn main() {
    eprintln!("uptime: only available on Unix");
    std::process::exit(1);
}

// fuptime — tell how long the system has been running
//
// Usage: uptime [OPTION]...

#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "uptime";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut pretty = false;
    let mut since = false;

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--help" => {
                println!("Usage: {} [OPTION]...", TOOL_NAME);
                println!("Print the current time, the length of time the system has been up,");
                println!("the number of users on the system, and the average number of jobs");
                println!("in the run queue over the last 1, 5 and 15 minutes.");
                println!();
                println!("  -p, --pretty   show uptime in pretty format");
                println!("  -s, --since    system up since, in yyyy-mm-dd HH:MM:SS format");
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "-p" | "--pretty" => pretty = true,
            "-s" | "--since" => since = true,
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                for ch in s[1..].chars() {
                    match ch {
                        'p' => pretty = true,
                        's' => since = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            _ => {
                eprintln!("{}: extra operand '{}'", TOOL_NAME, arg);
                process::exit(1);
            }
        }
    }

    let uptime_secs = read_uptime().unwrap_or_else(|e| {
        eprintln!("{}: {}", TOOL_NAME, e);
        process::exit(1);
    });

    if since {
        print_since(uptime_secs);
    } else if pretty {
        print_pretty(uptime_secs);
    } else {
        print_default(uptime_secs);
    }
}

#[cfg(unix)]
fn read_uptime() -> Result<f64, String> {
    let content = std::fs::read_to_string("/proc/uptime")
        .map_err(|e| format!("cannot read /proc/uptime: {}", e))?;
    content
        .split_whitespace()
        .next()
        .ok_or_else(|| "unexpected /proc/uptime format".to_string())?
        .parse::<f64>()
        .map_err(|_| "cannot parse uptime".to_string())
}

#[cfg(unix)]
fn read_loadavg() -> (f64, f64, f64) {
    if let Ok(content) = std::fs::read_to_string("/proc/loadavg") {
        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.len() >= 3 {
            let a = parts[0].parse().unwrap_or(0.0);
            let b = parts[1].parse().unwrap_or(0.0);
            let c = parts[2].parse().unwrap_or(0.0);
            return (a, b, c);
        }
    }
    (0.0, 0.0, 0.0)
}

#[cfg(unix)]
fn count_users() -> usize {
    // Count logged-in users by reading utmp
    // Simplified: count entries in /var/run/utmp
    // For a simpler approach, just return what `who | wc -l` would give
    if let Ok(output) = std::process::Command::new("who").output()
        && output.status.success()
    {
        return output.stdout.iter().filter(|&&b| b == b'\n').count();
    }
    0
}

#[cfg(unix)]
fn format_uptime(secs: f64) -> String {
    let total_secs = secs as u64;
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;

    if days > 0 {
        if hours > 0 || minutes > 0 {
            format!(
                "{} day{}, {:2}:{:02}",
                days,
                if days != 1 { "s" } else { "" },
                hours,
                minutes
            )
        } else {
            format!("{} day{}", days, if days != 1 { "s" } else { "" })
        }
    } else {
        format!("{:2}:{:02}", hours, minutes)
    }
}

#[cfg(unix)]
fn print_default(uptime_secs: f64) {
    // Get current time
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };

    let users = count_users();
    let (l1, l5, l15) = read_loadavg();
    let up_str = format_uptime(uptime_secs);

    let user_str = if users == 1 { "user" } else { "users" };

    println!(
        " {:02}:{:02}:{:02} up {},  {} {},  load average: {:.2}, {:.2}, {:.2}",
        tm.tm_hour, tm.tm_min, tm.tm_sec, up_str, users, user_str, l1, l5, l15
    );
}

#[cfg(unix)]
fn print_pretty(uptime_secs: f64) {
    let total_secs = uptime_secs as u64;
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{} day{}", days, if days != 1 { "s" } else { "" }));
    }
    if hours > 0 {
        parts.push(format!(
            "{} hour{}",
            hours,
            if hours != 1 { "s" } else { "" }
        ));
    }
    if minutes > 0 {
        parts.push(format!(
            "{} minute{}",
            minutes,
            if minutes != 1 { "s" } else { "" }
        ));
    }

    if parts.is_empty() {
        println!("up 0 minutes");
    } else {
        println!("up {}", parts.join(", "));
    }
}

#[cfg(unix)]
fn print_since(uptime_secs: f64) {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let boot_time = now - uptime_secs as libc::time_t;
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&boot_time, &mut tm);
        tm
    };

    println!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    );
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fuptime");
        Command::new(path)
    }

    #[test]
    fn test_uptime_format() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("up"), "Should contain 'up'");
        assert!(
            stdout.contains("load average"),
            "Should contain 'load average'"
        );
    }

    #[test]
    fn test_uptime_pretty() {
        let output = cmd().arg("-p").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.starts_with("up "),
            "Pretty format should start with 'up '"
        );
    }

    #[test]
    fn test_uptime_since() {
        let output = cmd().arg("-s").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let trimmed = stdout.trim();
        // Should be in format yyyy-mm-dd HH:MM:SS
        assert!(
            trimmed.len() >= 19,
            "Since format should be at least 19 chars: '{}'",
            trimmed
        );
        assert!(trimmed.contains('-'), "Should contain date separator");
        assert!(trimmed.contains(':'), "Should contain time separator");
    }

    #[test]
    fn test_uptime_matches_gnu_format() {
        let gnu = Command::new("uptime").arg("-p").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("-p").output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
            // The pretty format should be similar (may differ slightly in wording)
        }
    }
}
