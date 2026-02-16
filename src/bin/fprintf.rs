// fprintf -- format and print data
//
// Usage: printf FORMAT [ARGUMENT...]

use std::io::Write;
use std::process;

const TOOL_NAME: &str = "printf";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("Usage: {} FORMAT [ARGUMENT...]", TOOL_NAME);
    println!("  or:  {} OPTION", TOOL_NAME);
    println!();
    println!("Print ARGUMENT(s) according to FORMAT, or execute according to OPTION:");
    println!();
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
    println!();
    println!("FORMAT controls the output as in C printf.  Interpreted sequences are:");
    println!();
    println!("  \\\"      double quote");
    println!("  \\\\      backslash");
    println!("  \\a      alert (BEL)");
    println!("  \\b      backspace");
    println!("  \\c      produce no further output");
    println!("  \\e      escape");
    println!("  \\f      form feed");
    println!("  \\n      new line");
    println!("  \\r      carriage return");
    println!("  \\t      horizontal tab");
    println!("  \\v      vertical tab");
    println!("  \\NNN    byte with octal value NNN (1 to 3 digits)");
    println!("  \\xHH    byte with hexadecimal value HH (1 to 2 digits)");
    println!("  \\uHHHH  Unicode character with hex value HHHH (1 to 4 digits)");
    println!("  \\UHHHHHHHH  Unicode character with hex value HHHHHHHH (1 to 8 digits)");
    println!("  %%      a single %");
    println!();
    println!("  %b      ARGUMENT as a string with '\\' escapes interpreted");
    println!();
    println!("and all C format specifications ending with one of diouxXeEfgGcs.");
    println!();
    println!("NOTE: your shell may have its own version of printf, which usually supersedes");
    println!("the version described here.  Please refer to your shell's documentation");
    println!("for details about the options it supports.");
}

fn print_version() {
    println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    match args[0].as_str() {
        "--help" => {
            print_help();
            return;
        }
        "--version" => {
            print_version();
            return;
        }
        _ => {}
    }

    let format = &args[0];
    let arg_strs: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();

    let output = coreutils_rs::printf::process_format_string(format, &arg_strs);

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    if let Err(e) = handle.write_all(&output) {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            process::exit(0);
        }
        eprintln!("{}: write error: {}", TOOL_NAME, e);
        process::exit(1);
    }
}
