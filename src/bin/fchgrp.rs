#[cfg(not(unix))]
fn main() {
    eprintln!("chgrp: only available on Unix");
    std::process::exit(1);
}

// fchgrp -- change group ownership
//
// Usage: chgrp [OPTION]... GROUP FILE...
//        chgrp [OPTION]... --reference=RFILE FILE...

#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "chgrp";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut config = coreutils_rs::chgrp::ChgrpConfig::default();
    let mut reference: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut saw_dashdash = false;
    let mut no_preserve_root = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        if saw_dashdash {
            positional.push(arg.clone());
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
            "-c" | "--changes" => config.changes = true,
            "-f" | "--silent" | "--quiet" => config.silent = true,
            "-v" | "--verbose" => config.verbose = true,
            "--dereference" => config.no_dereference = false,
            "-h" | "--no-dereference" => config.no_dereference = true,
            "--preserve-root" => config.preserve_root = true,
            "--no-preserve-root" => no_preserve_root = true,
            "-R" | "--recursive" => config.recursive = true,
            "-H" => {
                config.symlink_follow = coreutils_rs::chown::SymlinkFollow::CommandLine;
            }
            "-L" => {
                config.symlink_follow = coreutils_rs::chown::SymlinkFollow::Always;
            }
            "-P" => {
                config.symlink_follow = coreutils_rs::chown::SymlinkFollow::Never;
            }
            "--" => saw_dashdash = true,
            s if s.starts_with("--reference=") => {
                reference = Some(s["--reference=".len()..].to_string());
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                for ch in s[1..].chars() {
                    match ch {
                        'c' => config.changes = true,
                        'f' => config.silent = true,
                        'v' => config.verbose = true,
                        'h' => config.no_dereference = true,
                        'R' => config.recursive = true,
                        'H' => {
                            config.symlink_follow =
                                coreutils_rs::chown::SymlinkFollow::CommandLine;
                        }
                        'L' => {
                            config.symlink_follow =
                                coreutils_rs::chown::SymlinkFollow::Always;
                        }
                        'P' => {
                            config.symlink_follow =
                                coreutils_rs::chown::SymlinkFollow::Never;
                        }
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            _ => positional.push(arg.clone()),
        }
        i += 1;
    }

    if no_preserve_root {
        config.preserve_root = false;
    }

    // Determine gid from reference or positional[0]
    let (gid, file_start) = if let Some(ref rfile) = reference {
        match coreutils_rs::chown::get_reference_ids(std::path::Path::new(rfile)) {
            Ok((_u, g)) => (g, 0),
            Err(e) => {
                eprintln!(
                    "{}: failed to get attributes of '{}': {}",
                    TOOL_NAME,
                    rfile,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        }
    } else {
        if positional.is_empty() {
            eprintln!("{}: missing operand", TOOL_NAME);
            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
            process::exit(1);
        }
        let group_spec = &positional[0];
        match coreutils_rs::chown::resolve_group(group_spec) {
            Some(g) => (g, 1),
            None => {
                eprintln!("{}: invalid group: '{}'", TOOL_NAME, group_spec);
                process::exit(1);
            }
        }
    };

    let files = &positional[file_start..];
    if files.is_empty() {
        eprintln!(
            "{}: missing operand after '{}'",
            TOOL_NAME,
            positional.first().map(|s| s.as_str()).unwrap_or("")
        );
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let mut errors = 0;
    for file in files {
        let path = std::path::Path::new(file);
        if config.recursive {
            errors += coreutils_rs::chgrp::chgrp_recursive(
                path, gid, &config, true, TOOL_NAME,
            );
        } else {
            match coreutils_rs::chgrp::chgrp_file(path, gid, &config) {
                Ok(_) => {}
                Err(e) => {
                    if !config.silent {
                        eprintln!(
                            "{}: changing group of '{}': {}",
                            TOOL_NAME,
                            file,
                            coreutils_rs::common::io_error_msg(&e)
                        );
                    }
                    errors += 1;
                }
            }
        }
    }

    if errors > 0 {
        process::exit(1);
    }
}

#[cfg(unix)]
fn print_help() {
    println!("Usage: {} [OPTION]... GROUP FILE...", TOOL_NAME);
    println!("  or:  {} [OPTION]... --reference=RFILE FILE...", TOOL_NAME);
    println!("Change the group of each FILE to GROUP.");
    println!("With --reference, change the group of each FILE to that of RFILE.");
    println!();
    println!("  -c, --changes          like verbose but report only when a change is made");
    println!("  -f, --silent, --quiet   suppress most error messages");
    println!("  -v, --verbose          output a diagnostic for every file processed");
    println!("      --dereference      affect the referent of each symbolic link (default)");
    println!("  -h, --no-dereference   affect symbolic links instead of any referenced file");
    println!("      --no-preserve-root  do not treat '/' specially (the default)");
    println!("      --preserve-root    fail to operate recursively on '/'");
    println!("      --reference=RFILE  use RFILE's group rather than specifying a GROUP value");
    println!("  -R, --recursive        operate on files and directories recursively");
    println!();
    println!("The following options modify how a hierarchy is traversed when -R is specified:");
    println!("  -H                     if a command line argument is a symbolic link to a");
    println!("                         directory, traverse it");
    println!("  -L                     traverse every symbolic link to a directory encountered");
    println!("  -P                     do not traverse any symbolic links (default)");
    println!();
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
}
