// fsleep — delay for a specified amount of time
//
// Usage: sleep NUMBER[SUFFIX]...
// Pause for NUMBER seconds. SUFFIX may be 's' (seconds, default), 'm' (minutes),
// 'h' (hours), or 'd' (days). Multiple arguments are summed. NUMBER may be float.

use std::process;
use std::time::Duration;

const TOOL_NAME: &str = "sleep";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    if args.len() == 1 {
        match args[0].as_str() {
            "--help" => {
                println!("Usage: {} NUMBER[smhd]...", TOOL_NAME);
                println!("  or:  {} OPTION", TOOL_NAME);
                println!("Pause for NUMBER seconds.  SUFFIX may be 's' for seconds (the default),");
                println!("'m' for minutes, 'h' for hours or 'd' for days.  NUMBER need not be an");
                println!("integer.  Given two or more arguments, pause for the amount of time");
                println!("specified by the sum of their values.");
                println!();
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            _ => {}
        }
    }

    let mut total_secs: f64 = 0.0;

    for arg in &args {
        match parse_duration(arg) {
            Ok(secs) => total_secs += secs,
            Err(msg) => {
                eprintln!("{}: invalid time interval '{}'", TOOL_NAME, msg);
                eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                process::exit(1);
            }
        }
    }

    if total_secs < 0.0 {
        eprintln!("{}: invalid time interval", TOOL_NAME);
        process::exit(1);
    }

    if total_secs == f64::INFINITY {
        // sleep infinity — just loop forever
        loop {
            std::thread::sleep(Duration::from_secs(86400));
        }
    }

    if total_secs > 0.0 {
        std::thread::sleep(Duration::from_secs_f64(total_secs));
    }
}

fn parse_duration(s: &str) -> Result<f64, String> {
    if s == "infinity" || s == "inf" {
        return Ok(f64::INFINITY);
    }

    let (num_str, multiplier) = if let Some(stripped) = s.strip_suffix('s') {
        (stripped, 1.0)
    } else if let Some(stripped) = s.strip_suffix('m') {
        (stripped, 60.0)
    } else if let Some(stripped) = s.strip_suffix('h') {
        (stripped, 3600.0)
    } else if let Some(stripped) = s.strip_suffix('d') {
        (stripped, 86400.0)
    } else {
        (s, 1.0)
    };

    let num: f64 = num_str.parse().map_err(|_| s.to_string())?;
    if num.is_nan() || num < 0.0 {
        return Err(s.to_string());
    }
    Ok(num * multiplier)
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::Instant;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fsleep");
        Command::new(path)
    }

    #[test]
    fn test_sleep_seconds() {
        let start = Instant::now();
        let output = cmd().arg("0").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(
            start.elapsed().as_millis() < 500,
            "sleep 0 should complete quickly"
        );
    }

    #[test]
    fn test_sleep_float() {
        let start = Instant::now();
        let output = cmd().arg("0.01").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(start.elapsed().as_millis() < 1000);
    }

    #[test]
    fn test_sleep_suffix_m() {
        // 0.001 minutes = 0.06 seconds
        let start = Instant::now();
        let output = cmd().arg("0.001m").output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(start.elapsed().as_millis() < 1000);
    }

    #[test]
    fn test_sleep_multiple_args() {
        let start = Instant::now();
        let output = cmd().args(["0.01", "0.01"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(start.elapsed().as_millis() < 1000);
    }

    #[test]
    fn test_sleep_invalid_arg() {
        let output = cmd().arg("abc").output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_sleep_matches_gnu() {
        // Both should handle "0" the same way
        let gnu = Command::new("sleep").arg("0").output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg("0").output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
        }
    }
}
