// ffactor -- print prime factors of each number
//
// Usage: factor [NUMBER]...
//        (reads from stdin if no arguments given)

use std::io::{self, BufRead, BufWriter, Write};
use std::process;

use coreutils_rs::factor;

const TOOL_NAME: &str = "factor";

fn print_help() {
    print!(
        "Usage: {0} [NUMBER]...\n\
         Print the prime factors of each specified integer.\n\n\
         \x20     --help     display this help and exit\n\
         \x20     --version  output version information and exit\n\n\
         Print the prime factors of each specified integer NUMBER. If none\n\
         are specified on the command line, read them from standard input.\n",
        TOOL_NAME
    );
}

fn print_version() {
    println!("{} (fcoreutils) {}", TOOL_NAME, env!("CARGO_PKG_VERSION"));
}

/// Process a single token: parse as u128, print factors, return true on success.
fn process_number(token: &str, out: &mut impl Write) -> bool {
    match token.parse::<u128>() {
        Ok(n) => {
            let line = factor::format_factors(n);
            if writeln!(out, "{}", line).is_err() {
                // Broken pipe or write error; exit cleanly
                process::exit(0);
            }
            true
        }
        Err(_) => {
            eprintln!(
                "{}: \u{2018}{}\u{2019} is not a valid positive integer",
                TOOL_NAME, token
            );
            false
        }
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // Parse options (only --help and --version)
    let mut numbers: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    for arg in &args {
        if saw_dashdash {
            numbers.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" => {
                saw_dashdash = true;
            }
            "--help" => {
                print_help();
                process::exit(0);
            }
            "--version" => {
                print_version();
                process::exit(0);
            }
            _ => {
                if arg.starts_with("--") {
                    eprintln!("{}: unrecognized option \u{2018}{}\u{2019}", TOOL_NAME, arg);
                    process::exit(1);
                }
                numbers.push(arg.clone());
            }
        }
    }

    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(256 * 1024, stdout.lock());
    let mut had_error = false;

    if numbers.is_empty() {
        // Read from stdin
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    let trimmed = l.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    // GNU factor splits on whitespace, allowing multiple numbers per line
                    for token in trimmed.split_whitespace() {
                        if !process_number(token, &mut out) {
                            had_error = true;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("{}: read error: {}", TOOL_NAME, e);
                    had_error = true;
                    break;
                }
            }
        }
    } else {
        for num_str in &numbers {
            if !process_number(num_str, &mut out) {
                had_error = true;
            }
        }
    }

    if out.flush().is_err() {
        process::exit(0);
    }

    if had_error {
        process::exit(1);
    }
}
