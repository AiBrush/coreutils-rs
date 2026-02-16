use std::path::Path;
use std::process;

const TOOL_NAME: &str = "shred";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!(
        "Usage: {} [OPTION]... FILE...",
        TOOL_NAME
    );
    println!("Overwrite the specified FILE(s) repeatedly, in order to make it harder");
    println!("for even very expensive hardware probing to recover the data.");
    println!();
    println!("If FILE is -, shred standard output.");
    println!();
    println!("  -f, --force        change permissions to allow writing if necessary");
    println!("  -n, --iterations=N overwrite N times instead of the default (3)");
    println!("  -s, --size=N       shred this many bytes (suffixes like K, M, G accepted)");
    println!("  -u                 deallocate and remove file after overwriting");
    println!("      --remove[=HOW] like -u but give control on HOW to delete;  See below");
    println!("  -v, --verbose      show progress");
    println!("  -x, --exact        do not round file sizes up to the next full block;");
    println!("                       this is the default for non-regular files");
    println!("  -z, --zero         add a final overwrite with zeros to hide shredding");
    println!("      --help         display this help and exit");
    println!("      --version      output version information and exit");
    println!();
    println!("Delete FILE(s) if --remove (-u) is specified.  The default is not to remove");
    println!("the files because it is common to operate on device files like /dev/hda,");
    println!("and those files usually should not be removed.");
    println!();
    println!("CAUTION: shred assumes the file system and hardware overwrite data in place.");
    println!("This is not true on journaled, log-structured, or copy-on-write file systems.");
    println!();
    println!("HOW values for --remove:");
    println!("  'unlink'   => use a standard unlink call");
    println!("  'wipe'     => also first obfuscate bytes in the name");
    println!("  'wipesync' => also sync each obfuscated name to the device");
    println!("The default mode is 'wipesync', but note it can be expensive.");
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut config = coreutils_rs::shred::ShredConfig::default();
    let mut files: Vec<String> = Vec::new();
    let mut saw_dashdash = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        if saw_dashdash {
            files.push(arg.clone());
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
            "--" => {
                saw_dashdash = true;
            }
            "-f" | "--force" => {
                config.force = true;
            }
            "-v" | "--verbose" => {
                config.verbose = true;
            }
            "-x" | "--exact" => {
                config.exact = true;
            }
            "-z" | "--zero" => {
                config.zero_pass = true;
            }
            "-u" => {
                config.remove = Some(coreutils_rs::shred::RemoveMode::WipeSync);
            }
            "-n" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'n'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                config.iterations = match args[i].parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!(
                            "{}: invalid number of passes: '{}'",
                            TOOL_NAME, args[i]
                        );
                        process::exit(1);
                    }
                };
            }
            "-s" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 's'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                config.size = match coreutils_rs::shred::parse_size(&args[i]) {
                    Ok(n) => Some(n),
                    Err(e) => {
                        eprintln!("{}: {}", TOOL_NAME, e);
                        process::exit(1);
                    }
                };
            }
            _ if arg.starts_with("--iterations=") => {
                let val = &arg["--iterations=".len()..];
                config.iterations = match val.parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!(
                            "{}: invalid number of passes: '{}'",
                            TOOL_NAME, val
                        );
                        process::exit(1);
                    }
                };
            }
            _ if arg.starts_with("--size=") => {
                let val = &arg["--size=".len()..];
                config.size = match coreutils_rs::shred::parse_size(val) {
                    Ok(n) => Some(n),
                    Err(e) => {
                        eprintln!("{}: {}", TOOL_NAME, e);
                        process::exit(1);
                    }
                };
            }
            _ if arg.starts_with("--remove") => {
                match coreutils_rs::shred::parse_remove_mode(arg) {
                    Ok(mode) => config.remove = Some(mode),
                    Err(e) => {
                        eprintln!("{}: {}", TOOL_NAME, e);
                        process::exit(1);
                    }
                }
            }
            _ if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") => {
                // Parse combined short flags like -vfz
                let chars: Vec<char> = arg[1..].chars().collect();
                let mut j = 0;
                while j < chars.len() {
                    match chars[j] {
                        'f' => config.force = true,
                        'v' => config.verbose = true,
                        'x' => config.exact = true,
                        'z' => config.zero_pass = true,
                        'u' => {
                            config.remove =
                                Some(coreutils_rs::shred::RemoveMode::WipeSync);
                        }
                        'n' => {
                            // Rest of this arg or next arg is the count
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!(
                                        "{}: option requires an argument -- 'n'",
                                        TOOL_NAME
                                    );
                                    process::exit(1);
                                }
                                config.iterations = match args[i].parse() {
                                    Ok(n) => n,
                                    Err(_) => {
                                        eprintln!(
                                            "{}: invalid number of passes: '{}'",
                                            TOOL_NAME, args[i]
                                        );
                                        process::exit(1);
                                    }
                                };
                            } else {
                                config.iterations = match rest.parse() {
                                    Ok(n) => n,
                                    Err(_) => {
                                        eprintln!(
                                            "{}: invalid number of passes: '{}'",
                                            TOOL_NAME, rest
                                        );
                                        process::exit(1);
                                    }
                                };
                            }
                            break;
                        }
                        's' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!(
                                        "{}: option requires an argument -- 's'",
                                        TOOL_NAME
                                    );
                                    process::exit(1);
                                }
                                config.size =
                                    match coreutils_rs::shred::parse_size(&args[i]) {
                                        Ok(n) => Some(n),
                                        Err(e) => {
                                            eprintln!("{}: {}", TOOL_NAME, e);
                                            process::exit(1);
                                        }
                                    };
                            } else {
                                config.size =
                                    match coreutils_rs::shred::parse_size(&rest) {
                                        Ok(n) => Some(n),
                                        Err(e) => {
                                            eprintln!("{}: {}", TOOL_NAME, e);
                                            process::exit(1);
                                        }
                                    };
                            }
                            break;
                        }
                        _ => {
                            eprintln!(
                                "{}: invalid option -- '{}'",
                                TOOL_NAME, chars[j]
                            );
                            eprintln!(
                                "Try '{} --help' for more information.",
                                TOOL_NAME
                            );
                            process::exit(1);
                        }
                    }
                    j += 1;
                }
            }
            _ => {
                files.push(arg.clone());
            }
        }
        i += 1;
    }

    if files.is_empty() {
        eprintln!("{}: missing file operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let mut exit_code = 0;
    for file in &files {
        let path = Path::new(file);
        if let Err(e) = coreutils_rs::shred::shred_file(path, &config) {
            eprintln!(
                "{}: {}: {}",
                TOOL_NAME,
                file,
                coreutils_rs::common::io_error_msg(&e)
            );
            exit_code = 1;
        }
    }

    if exit_code != 0 {
        process::exit(exit_code);
    }
}
