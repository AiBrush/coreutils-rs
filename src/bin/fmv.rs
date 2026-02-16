// fmv -- move (rename) files
//
// Usage: mv [OPTION]... [-T] SOURCE DEST
//        mv [OPTION]... SOURCE... DIRECTORY
//        mv [OPTION]... -t DIRECTORY SOURCE...

use std::path::Path;
use std::process;

use coreutils_rs::mv::{
    mv_file, parse_backup_mode, strip_trailing_slashes, BackupMode, MvConfig,
};

const TOOL_NAME: &str = "mv";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut config = MvConfig::default();
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
            "--" => saw_dashdash = true,
            "-f" | "--force" => {
                config.force = true;
                config.interactive = false;
                config.no_clobber = false;
            }
            "-i" | "--interactive" => {
                config.interactive = true;
                config.force = false;
                config.no_clobber = false;
            }
            "-n" | "--no-clobber" => {
                config.no_clobber = true;
                config.force = false;
                config.interactive = false;
            }
            "-v" | "--verbose" => config.verbose = true,
            "-u" | "--update" => config.update = true,
            "-b" => config.backup = Some(BackupMode::Simple),
            "--strip-trailing-slashes" => config.strip_trailing_slashes = true,
            "-T" | "--no-target-directory" => config.no_target_directory = true,
            "-t" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 't'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                config.target_directory = Some(args[i].clone());
            }
            "-S" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'S'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                config.suffix = args[i].clone();
            }
            _ if arg.starts_with("--backup=") => {
                let val = &arg["--backup=".len()..];
                match parse_backup_mode(val) {
                    Some(mode) => config.backup = Some(mode),
                    None => {
                        eprintln!(
                            "{}: invalid backup type '{}'",
                            TOOL_NAME, val
                        );
                        process::exit(1);
                    }
                }
            }
            "--backup" => config.backup = Some(BackupMode::Existing),
            _ if arg.starts_with("--target-directory=") => {
                config.target_directory =
                    Some(arg["--target-directory=".len()..].to_string());
            }
            _ if arg.starts_with("--suffix=") => {
                config.suffix = arg["--suffix=".len()..].to_string();
            }
            _ if arg.starts_with("-S") && arg.len() > 2 => {
                config.suffix = arg[2..].to_string();
            }
            _ if arg.starts_with("-t") && arg.len() > 2 => {
                config.target_directory = Some(arg[2..].to_string());
            }
            _ if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") => {
                // Combined short flags like -fvn
                let chars: Vec<char> = arg[1..].chars().collect();
                let mut j = 0;
                while j < chars.len() {
                    match chars[j] {
                        'f' => {
                            config.force = true;
                            config.interactive = false;
                            config.no_clobber = false;
                        }
                        'i' => {
                            config.interactive = true;
                            config.force = false;
                            config.no_clobber = false;
                        }
                        'n' => {
                            config.no_clobber = true;
                            config.force = false;
                            config.interactive = false;
                        }
                        'v' => config.verbose = true,
                        'u' => config.update = true,
                        'b' => config.backup = Some(BackupMode::Simple),
                        'T' => config.no_target_directory = true,
                        'S' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!(
                                        "{}: option requires an argument -- 'S'",
                                        TOOL_NAME
                                    );
                                    eprintln!(
                                        "Try '{} --help' for more information.",
                                        TOOL_NAME
                                    );
                                    process::exit(1);
                                }
                                config.suffix = args[i].clone();
                            } else {
                                config.suffix = rest;
                            }
                            break;
                        }
                        't' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!(
                                        "{}: option requires an argument -- 't'",
                                        TOOL_NAME
                                    );
                                    eprintln!(
                                        "Try '{} --help' for more information.",
                                        TOOL_NAME
                                    );
                                    process::exit(1);
                                }
                                config.target_directory = Some(args[i].clone());
                            } else {
                                config.target_directory = Some(rest);
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
        eprintln!("{}: missing file operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    // Strip trailing slashes if requested
    if config.strip_trailing_slashes {
        for op in &mut operands {
            *op = strip_trailing_slashes(op).to_string();
        }
    }

    let mut exit_code = 0;

    if let Some(ref dir) = config.target_directory {
        // -t DIRECTORY SOURCE...
        if !Path::new(dir).is_dir() {
            eprintln!(
                "{}: target '{}' is not a directory",
                TOOL_NAME, dir
            );
            process::exit(1);
        }
        for source in &operands {
            let src_path = Path::new(source);
            if !src_path.exists() && src_path.symlink_metadata().is_err() {
                eprintln!(
                    "{}: cannot stat '{}': No such file or directory",
                    TOOL_NAME, source
                );
                exit_code = 1;
                continue;
            }
            let basename = src_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| source.clone());
            let dst = Path::new(dir).join(&basename);
            if let Err(e) = mv_file(src_path, &dst, &config) {
                eprintln!(
                    "{}: cannot move '{}' to '{}': {}",
                    TOOL_NAME,
                    source,
                    dst.display(),
                    coreutils_rs::common::io_error_msg(&e)
                );
                exit_code = 1;
            }
        }
    } else if config.no_target_directory {
        // -T: exactly two operands, treat DEST as a normal file
        if operands.len() < 2 {
            eprintln!(
                "{}: missing destination file operand after '{}'",
                TOOL_NAME, operands[0]
            );
            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
            process::exit(1);
        }
        if operands.len() > 2 {
            eprintln!("{}: extra operand '{}'", TOOL_NAME, operands[2]);
            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
            process::exit(1);
        }
        let src = Path::new(&operands[0]);
        let dst = Path::new(&operands[1]);
        if !src.exists() && src.symlink_metadata().is_err() {
            eprintln!(
                "{}: cannot stat '{}': No such file or directory",
                TOOL_NAME, operands[0]
            );
            process::exit(1);
        }
        if let Err(e) = mv_file(src, dst, &config) {
            eprintln!(
                "{}: cannot move '{}' to '{}': {}",
                TOOL_NAME,
                operands[0],
                operands[1],
                coreutils_rs::common::io_error_msg(&e)
            );
            process::exit(1);
        }
    } else if operands.len() == 1 {
        eprintln!(
            "{}: missing destination file operand after '{}'",
            TOOL_NAME, operands[0]
        );
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    } else if operands.len() == 2 {
        let src = Path::new(&operands[0]);
        let dst_str = &operands[1];
        let dst = Path::new(dst_str);

        if !src.exists() && src.symlink_metadata().is_err() {
            eprintln!(
                "{}: cannot stat '{}': No such file or directory",
                TOOL_NAME, operands[0]
            );
            process::exit(1);
        }

        if dst.is_dir() {
            // Move into directory
            let basename = src
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| operands[0].clone());
            let final_dst = dst.join(&basename);
            if let Err(e) = mv_file(src, &final_dst, &config) {
                eprintln!(
                    "{}: cannot move '{}' to '{}': {}",
                    TOOL_NAME,
                    operands[0],
                    final_dst.display(),
                    coreutils_rs::common::io_error_msg(&e)
                );
                exit_code = 1;
            }
        } else if let Err(e) = mv_file(src, dst, &config) {
            eprintln!(
                "{}: cannot move '{}' to '{}': {}",
                TOOL_NAME,
                operands[0],
                operands[1],
                coreutils_rs::common::io_error_msg(&e)
            );
            exit_code = 1;
        }
    } else {
        // Multiple operands: last must be a directory
        let dir = &operands[operands.len() - 1];
        if !Path::new(dir).is_dir() {
            eprintln!(
                "{}: target '{}' is not a directory",
                TOOL_NAME, dir
            );
            process::exit(1);
        }
        for source in &operands[..operands.len() - 1] {
            let src_path = Path::new(source);
            if !src_path.exists() && src_path.symlink_metadata().is_err() {
                eprintln!(
                    "{}: cannot stat '{}': No such file or directory",
                    TOOL_NAME, source
                );
                exit_code = 1;
                continue;
            }
            let basename = src_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| source.clone());
            let final_dst = Path::new(dir).join(&basename);
            if let Err(e) = mv_file(src_path, &final_dst, &config) {
                eprintln!(
                    "{}: cannot move '{}' to '{}': {}",
                    TOOL_NAME,
                    source,
                    final_dst.display(),
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

fn print_help() {
    println!("Usage: {} [OPTION]... [-T] SOURCE DEST", TOOL_NAME);
    println!("  or:  {} [OPTION]... SOURCE... DIRECTORY", TOOL_NAME);
    println!("  or:  {} [OPTION]... -t DIRECTORY SOURCE...", TOOL_NAME);
    println!("Rename SOURCE to DEST, or move SOURCE(s) to DIRECTORY.");
    println!();
    println!("  -b                           like --backup but does not accept an argument");
    println!("      --backup[=CONTROL]       make a backup of each existing destination file");
    println!("  -f, --force                  do not prompt before overwriting");
    println!("  -i, --interactive            prompt before overwrite");
    println!("  -n, --no-clobber             do not overwrite an existing file");
    println!("      --strip-trailing-slashes  remove any trailing slashes from each SOURCE");
    println!("  -S, --suffix=SUFFIX          override the usual backup suffix");
    println!("  -t, --target-directory=DIRECTORY  move all SOURCE arguments into DIRECTORY");
    println!("  -T, --no-target-directory    treat DEST as a normal file");
    println!("  -u, --update                 move only when the SOURCE file is newer");
    println!("                                 than the destination file or when the");
    println!("                                 destination file is missing");
    println!("  -v, --verbose                explain what is being done");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
    println!();
    println!("The backup suffix is '~', unless set with --suffix or SIMPLE_BACKUP_SUFFIX.");
    println!(
        "The version control method may be selected via the --backup option or through"
    );
    println!("the VERSION_CONTROL environment variable.  Here are the values:");
    println!();
    println!("  none, off       never make backups (even if --backup is given)");
    println!("  numbered, t     make numbered backups");
    println!("  existing, nil   numbered if numbered backups exist, simple otherwise");
    println!("  simple, never   always make simple backups");
}
