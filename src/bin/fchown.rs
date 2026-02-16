#[cfg(not(unix))]
fn main() {
    eprintln!("chown: only available on Unix");
    std::process::exit(1);
}

// fchown -- change file owner and group
//
// Usage: chown [OPTION]... [OWNER][:[GROUP]] FILE...
//        chown [OPTION]... --reference=RFILE FILE...

#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "chown";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut config = coreutils_rs::chown::ChownConfig::default();
    let mut reference: Option<String> = None;
    let mut from_spec: Option<String> = None;
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
            "-H" => config.symlink_follow = coreutils_rs::chown::SymlinkFollow::CommandLine,
            "-L" => config.symlink_follow = coreutils_rs::chown::SymlinkFollow::Always,
            "-P" => config.symlink_follow = coreutils_rs::chown::SymlinkFollow::Never,
            "--" => saw_dashdash = true,
            s if s.starts_with("--reference=") => {
                reference = Some(s["--reference=".len()..].to_string());
            }
            s if s.starts_with("--from=") => {
                from_spec = Some(s["--from=".len()..].to_string());
            }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                // Combined short flags like -Rcfv
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
                            config.symlink_follow = coreutils_rs::chown::SymlinkFollow::Always;
                        }
                        'P' => {
                            config.symlink_follow = coreutils_rs::chown::SymlinkFollow::Never;
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

    // Parse --from spec
    if let Some(ref spec) = from_spec {
        match parse_from_spec(spec) {
            Ok((u, g)) => {
                config.from_owner = u;
                config.from_group = g;
            }
            Err(e) => {
                eprintln!("{}: {}", TOOL_NAME, e);
                process::exit(1);
            }
        }
    }

    // Determine uid/gid from reference or positional[0]
    let (uid, gid, file_start) = if let Some(ref rfile) = reference {
        match coreutils_rs::chown::get_reference_ids(std::path::Path::new(rfile)) {
            Ok((u, g)) => (Some(u), Some(g), 0),
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
        let spec = &positional[0];
        match coreutils_rs::chown::parse_owner_spec(spec) {
            Ok((u, g)) => (u, g, 1),
            Err(e) => {
                eprintln!("{}: {}", TOOL_NAME, e);
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
            errors += coreutils_rs::chown::chown_recursive(
                path, uid, gid, &config, true, TOOL_NAME,
            );
        } else {
            match coreutils_rs::chown::chown_file(path, uid, gid, &config) {
                Ok(_) => {}
                Err(e) => {
                    if !config.silent {
                        eprintln!(
                            "{}: changing ownership of '{}': {}",
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
fn parse_from_spec(spec: &str) -> Result<(Option<u32>, Option<u32>), String> {
    if spec.is_empty() {
        return Ok((None, None));
    }
    if let Some(idx) = spec.find(':') {
        let user_part = &spec[..idx];
        let group_part = &spec[idx + 1..];
        let uid = if user_part.is_empty() {
            None
        } else {
            Some(
                coreutils_rs::chown::resolve_user(user_part)
                    .ok_or_else(|| format!("invalid user: '{}'", user_part))?,
            )
        };
        let gid = if group_part.is_empty() {
            None
        } else {
            Some(
                coreutils_rs::chown::resolve_group(group_part)
                    .ok_or_else(|| format!("invalid group: '{}'", group_part))?,
            )
        };
        Ok((uid, gid))
    } else {
        let uid = coreutils_rs::chown::resolve_user(spec)
            .ok_or_else(|| format!("invalid user: '{}'", spec))?;
        Ok((Some(uid), None))
    }
}

#[cfg(unix)]
fn print_help() {
    println!("Usage: {} [OPTION]... [OWNER][:[GROUP]] FILE...", TOOL_NAME);
    println!("  or:  {} [OPTION]... --reference=RFILE FILE...", TOOL_NAME);
    println!("Change the owner and/or group of each FILE to OWNER and/or GROUP.");
    println!("With --reference, change the owner and group of each FILE to those of RFILE.");
    println!();
    println!("  -c, --changes          like verbose but report only when a change is made");
    println!("  -f, --silent, --quiet   suppress most error messages");
    println!("  -v, --verbose          output a diagnostic for every file processed");
    println!("      --dereference      affect the referent of each symbolic link (default)");
    println!("  -h, --no-dereference   affect symbolic links instead of any referenced file");
    println!("      --from=CURRENT_OWNER:CURRENT_GROUP");
    println!("                         change the owner and/or group of each file only if");
    println!("                         its current owner and/or group match those specified");
    println!("      --no-preserve-root  do not treat '/' specially (the default)");
    println!("      --preserve-root    fail to operate recursively on '/'");
    println!("      --reference=RFILE  use RFILE's owner and group rather than specifying");
    println!("                         OWNER:GROUP values");
    println!("  -R, --recursive        operate on files and directories recursively");
    println!();
    println!("The following options modify how a hierarchy is traversed when -R is specified:");
    println!("  -H                     if a command line argument is a symbolic link to a");
    println!("                         directory, traverse it");
    println!("  -L                     traverse every symbolic link to a directory encountered");
    println!("  -P                     do not traverse any symbolic links (default)");
    println!();
    println!("Owner is unchanged if missing.  Group is unchanged if missing, but changed");
    println!("to login group if implied by a ':' following a symbolic OWNER.");
    println!("OWNER and GROUP may be numeric as well as symbolic.");
    println!();
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
}
