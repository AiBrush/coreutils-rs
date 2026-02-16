// fstdbuf -- run a command with modified buffering for its standard streams
//
// Usage: stdbuf [OPTION]... COMMAND [ARG]...
//
// Adjusts stdin/stdout/stderr buffering of COMMAND by setting environment
// variables and executing the command.

use std::process;

const TOOL_NAME: &str = "stdbuf";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("Usage: {} OPTION... COMMAND", TOOL_NAME);
    println!("Run COMMAND, with modified buffering operations for its standard streams.");
    println!();
    println!("  -i, --input=MODE   adjust standard input stream buffering");
    println!("  -o, --output=MODE  adjust standard output stream buffering");
    println!("  -e, --error=MODE   adjust standard error stream buffering");
    println!("      --help         display this help and exit");
    println!("      --version      output version information and exit");
    println!();
    println!("MODE can be:");
    println!("  L       line buffered");
    println!("  0       unbuffered");
    println!("  SIZE    fully buffered with SIZE bytes (supports K, M, G suffixes)");
    println!();
    println!("NOTE: This implementation sets _STDBUF_I, _STDBUF_O, _STDBUF_E");
    println!("environment variables. A full implementation requires an LD_PRELOAD library.");
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(125);
    }

    let mut input_mode: Option<coreutils_rs::stdbuf::BufferMode> = None;
    let mut output_mode: Option<coreutils_rs::stdbuf::BufferMode> = None;
    let mut error_mode: Option<coreutils_rs::stdbuf::BufferMode> = None;
    let mut command_start: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--" => {
                command_start = Some(i + 1);
                break;
            }
            s if s.starts_with("--input=") => {
                let val = &s["--input=".len()..];
                input_mode = Some(parse_mode_or_exit(val));
            }
            s if s.starts_with("--output=") => {
                let val = &s["--output=".len()..];
                output_mode = Some(parse_mode_or_exit(val));
            }
            s if s.starts_with("--error=") => {
                let val = &s["--error=".len()..];
                error_mode = Some(parse_mode_or_exit(val));
            }
            "-i" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'i'", TOOL_NAME);
                    process::exit(125);
                }
                input_mode = Some(parse_mode_or_exit(&args[i]));
            }
            "-o" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'o'", TOOL_NAME);
                    process::exit(125);
                }
                output_mode = Some(parse_mode_or_exit(&args[i]));
            }
            "-e" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'e'", TOOL_NAME);
                    process::exit(125);
                }
                error_mode = Some(parse_mode_or_exit(&args[i]));
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                // Handle combined short flags like -iL, -o0
                let chars: Vec<char> = s[1..].chars().collect();
                let mut j = 0;
                while j < chars.len() {
                    match chars[j] {
                        'i' => {
                            let val = if j + 1 < chars.len() {
                                let rest: String = chars[j + 1..].iter().collect();
                                j = chars.len(); // consume rest
                                rest
                            } else {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 'i'", TOOL_NAME);
                                    process::exit(125);
                                }
                                args[i].clone()
                            };
                            input_mode = Some(parse_mode_or_exit(&val));
                        }
                        'o' => {
                            let val = if j + 1 < chars.len() {
                                let rest: String = chars[j + 1..].iter().collect();
                                j = chars.len();
                                rest
                            } else {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 'o'", TOOL_NAME);
                                    process::exit(125);
                                }
                                args[i].clone()
                            };
                            output_mode = Some(parse_mode_or_exit(&val));
                        }
                        'e' => {
                            let val = if j + 1 < chars.len() {
                                let rest: String = chars[j + 1..].iter().collect();
                                j = chars.len();
                                rest
                            } else {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!("{}: option requires an argument -- 'e'", TOOL_NAME);
                                    process::exit(125);
                                }
                                args[i].clone()
                            };
                            error_mode = Some(parse_mode_or_exit(&val));
                        }
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, chars[j]);
                            process::exit(125);
                        }
                    }
                    j += 1;
                }
            }
            _ => {
                // First non-option argument is the command
                command_start = Some(i);
                break;
            }
        }
        i += 1;
    }

    if command_start.is_none() || command_start.unwrap() >= args.len() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(125);
    }

    // At least one mode must be specified
    if input_mode.is_none() && output_mode.is_none() && error_mode.is_none() {
        eprintln!(
            "{}: you must specify a buffering mode option",
            TOOL_NAME
        );
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(125);
    }

    let cmd_idx = command_start.unwrap();
    let config = coreutils_rs::stdbuf::StdbufConfig {
        input: input_mode,
        output: output_mode,
        error: error_mode,
        command: args[cmd_idx].clone(),
        args: args[cmd_idx + 1..].to_vec(),
    };

    if let Err(e) = coreutils_rs::stdbuf::run_stdbuf(&config) {
        eprintln!(
            "{}: failed to run '{}': {}",
            TOOL_NAME,
            config.command,
            coreutils_rs::common::io_error_msg(&e)
        );
        let code = if e.kind() == std::io::ErrorKind::NotFound {
            127
        } else {
            126
        };
        process::exit(code);
    }
}

fn parse_mode_or_exit(s: &str) -> coreutils_rs::stdbuf::BufferMode {
    coreutils_rs::stdbuf::parse_buffer_mode(s).unwrap_or_else(|msg| {
        eprintln!("{}: {}", TOOL_NAME, msg);
        process::exit(125);
    })
}
