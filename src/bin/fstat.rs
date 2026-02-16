#[cfg(not(unix))]
fn main() {
    eprintln!("stat: only available on Unix");
    std::process::exit(1);
}

// fstat -- display file or filesystem status
//
// Usage: stat [OPTION]... FILE...

#[cfg(unix)]
use std::process;

#[cfg(unix)]
use coreutils_rs::stat::StatConfig;

#[cfg(unix)]
const TOOL_NAME: &str = "stat";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut dereference = false;
    let mut filesystem = false;
    let mut format: Option<String> = None;
    let mut printf_format: Option<String> = None;
    let mut terse = false;
    let mut operands: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if saw_dashdash {
            operands.push(arg.clone());
            i += 1;
            continue;
        }
        match arg.as_str() {
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "-L" | "--dereference" => dereference = true,
            "-f" | "--file-system" => filesystem = true,
            "-t" | "--terse" => terse = true,
            "-c" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'c'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                format = Some(args[i].clone());
            }
            "--" => saw_dashdash = true,
            _ if arg.starts_with("--format=") => {
                format = Some(arg["--format=".len()..].to_string());
            }
            _ if arg.starts_with("--printf=") => {
                printf_format = Some(arg["--printf=".len()..].to_string());
            }
            _ if arg.starts_with("-c") && arg.len() > 2 => {
                format = Some(arg[2..].to_string());
            }
            _ if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") => {
                // Combined short flags
                let chars: Vec<char> = arg[1..].chars().collect();
                let mut j = 0;
                while j < chars.len() {
                    match chars[j] {
                        'L' => dereference = true,
                        'f' => filesystem = true,
                        't' => terse = true,
                        'c' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!(
                                        "{}: option requires an argument -- 'c'",
                                        TOOL_NAME
                                    );
                                    eprintln!(
                                        "Try '{} --help' for more information.",
                                        TOOL_NAME
                                    );
                                    process::exit(1);
                                }
                                format = Some(args[i].clone());
                            } else {
                                format = Some(rest);
                            }
                            break;
                        }
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, chars[j]);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    j += 1;
                }
            }
            _ => operands.push(arg.clone()),
        }
        i += 1;
    }

    if operands.is_empty() {
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let config = StatConfig {
        dereference,
        filesystem,
        format,
        printf_format,
        terse,
    };

    let mut exit_code = 0;

    for path in &operands {
        match coreutils_rs::stat::stat_file(path, &config) {
            Ok(output) => {
                print!("{}", output);
            }
            Err(e) => {
                eprintln!(
                    "{}: cannot stat '{}': {}",
                    TOOL_NAME,
                    path,
                    coreutils_rs::common::io_error_msg(&e)
                );
                exit_code = 1;
            }
        }
    }

    if exit_code != 0 {
        process::exit(exit_code);
    }
}

#[cfg(unix)]
fn print_help() {
    println!("Usage: {} [OPTION]... FILE...", TOOL_NAME);
    println!("Display file or file system status.");
    println!();
    println!("  -L, --dereference     follow links");
    println!("  -f, --file-system     display file system status instead of file status");
    println!(
        "  -c, --format=FORMAT   use the specified FORMAT instead of the default;"
    );
    println!("                          output a newline after each use of FORMAT");
    println!(
        "      --printf=FORMAT   like --format, but interpret backslash escapes,"
    );
    println!("                          and do not output a mandatory trailing newline;");
    println!("                          if you want a newline, include \\n in FORMAT");
    println!("  -t, --terse           print the information in terse form");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
    println!();
    println!("The valid format sequences for files (without --file-system):");
    println!("  %a   access rights in octal");
    println!("  %A   access rights in human readable form");
    println!("  %b   number of blocks allocated (see %B)");
    println!("  %B   the size in bytes of each block reported by %b");
    println!("  %d   device number in decimal");
    println!("  %D   device number in hex");
    println!("  %f   raw mode in hex");
    println!("  %F   file type");
    println!("  %g   group ID of owner");
    println!("  %G   group name of owner");
    println!("  %h   number of hard links");
    println!("  %i   inode number");
    println!("  %m   mount point");
    println!("  %n   file name");
    println!("  %N   quoted file name with dereference if symbolic link");
    println!("  %o   optimal I/O transfer size hint");
    println!("  %s   total size, in bytes");
    println!("  %t   major device type in hex, for character/block device special files");
    println!("  %T   minor device type in hex, for character/block device special files");
    println!("  %u   user ID of owner");
    println!("  %U   user name of owner");
    println!("  %w   time of file birth, human-readable; - if unknown");
    println!("  %W   time of file birth, seconds since Epoch; 0 if unknown");
    println!("  %x   time of last access, human-readable");
    println!("  %X   time of last access, seconds since Epoch");
    println!("  %y   time of last data modification, human-readable");
    println!("  %Y   time of last data modification, seconds since Epoch");
    println!("  %z   time of last status change, human-readable");
    println!("  %Z   time of last status change, seconds since Epoch");
    println!();
    println!("Valid format sequences for file systems:");
    println!("  %a   free blocks available to non-superuser");
    println!("  %b   total data blocks in file system");
    println!("  %c   total file nodes in file system");
    println!("  %d   free file nodes in file system");
    println!("  %f   free blocks in file system");
    println!("  %i   file system ID in hex");
    println!("  %l   maximum length of filenames");
    println!("  %n   file name");
    println!("  %s   block size (for faster transfers)");
    println!("  %S   fundamental block size (for block counts)");
    println!("  %t   file system type in hex");
    println!("  %T   file system type in human readable form");
}
