// finstall -- copy files and set attributes
//
// Usage: install [OPTION]... [-T] SOURCE DEST
//        install [OPTION]... SOURCE... DIRECTORY
//        install [OPTION]... -t DIRECTORY SOURCE...
//        install [OPTION]... -d DIRECTORY...

use std::path::Path;
use std::process;

use coreutils_rs::install::{
    install_directories, install_file, parse_backup_mode, parse_mode,
    BackupMode, InstallConfig,
};

const TOOL_NAME: &str = "install";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut config = InstallConfig::default();
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
            "-b" => config.backup = Some(BackupMode::Simple),
            "-C" | "--compare" => config.compare = true,
            "-d" | "--directory" => config.directory_mode = true,
            "-D" => config.create_leading = true,
            "-p" | "--preserve-timestamps" => config.preserve_timestamps = true,
            "-s" | "--strip" => config.strip = true,
            "-v" | "--verbose" => config.verbose = true,
            "-T" | "--no-target-directory" => config.no_target_directory = true,
            "-m" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'm'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                match parse_mode(&args[i]) {
                    Ok(m) => config.mode = m,
                    Err(e) => {
                        eprintln!("{}: {}", TOOL_NAME, e);
                        process::exit(1);
                    }
                }
            }
            "-g" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'g'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                config.group = Some(args[i].clone());
            }
            "-o" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'o'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                config.owner = Some(args[i].clone());
            }
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
                        eprintln!("{}: invalid backup type '{}'", TOOL_NAME, val);
                        process::exit(1);
                    }
                }
            }
            "--backup" => config.backup = Some(BackupMode::Existing),
            _ if arg.starts_with("--mode=") => {
                let val = &arg["--mode=".len()..];
                match parse_mode(val) {
                    Ok(m) => config.mode = m,
                    Err(e) => {
                        eprintln!("{}: {}", TOOL_NAME, e);
                        process::exit(1);
                    }
                }
            }
            _ if arg.starts_with("--group=") => {
                config.group = Some(arg["--group=".len()..].to_string());
            }
            _ if arg.starts_with("--owner=") => {
                config.owner = Some(arg["--owner=".len()..].to_string());
            }
            _ if arg.starts_with("--target-directory=") => {
                config.target_directory =
                    Some(arg["--target-directory=".len()..].to_string());
            }
            _ if arg.starts_with("--suffix=") => {
                config.suffix = arg["--suffix=".len()..].to_string();
            }
            _ if arg.starts_with("--strip-program=") => {
                config.strip_program = arg["--strip-program=".len()..].to_string();
            }
            _ if arg.starts_with("-m") && arg.len() > 2 => {
                let val = &arg[2..];
                match parse_mode(val) {
                    Ok(m) => config.mode = m,
                    Err(e) => {
                        eprintln!("{}: {}", TOOL_NAME, e);
                        process::exit(1);
                    }
                }
            }
            _ if arg.starts_with("-g") && arg.len() > 2 => {
                config.group = Some(arg[2..].to_string());
            }
            _ if arg.starts_with("-o") && arg.len() > 2 => {
                config.owner = Some(arg[2..].to_string());
            }
            _ if arg.starts_with("-t") && arg.len() > 2 => {
                config.target_directory = Some(arg[2..].to_string());
            }
            _ if arg.starts_with("-S") && arg.len() > 2 => {
                config.suffix = arg[2..].to_string();
            }
            _ if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") => {
                // Combined short flags
                let chars: Vec<char> = arg[1..].chars().collect();
                let mut j = 0;
                while j < chars.len() {
                    match chars[j] {
                        'b' => config.backup = Some(BackupMode::Simple),
                        'C' => config.compare = true,
                        'd' => config.directory_mode = true,
                        'D' => config.create_leading = true,
                        'p' => config.preserve_timestamps = true,
                        's' => config.strip = true,
                        'v' => config.verbose = true,
                        'T' => config.no_target_directory = true,
                        'm' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!(
                                        "{}: option requires an argument -- 'm'",
                                        TOOL_NAME
                                    );
                                    process::exit(1);
                                }
                                match parse_mode(&args[i]) {
                                    Ok(m) => config.mode = m,
                                    Err(e) => {
                                        eprintln!("{}: {}", TOOL_NAME, e);
                                        process::exit(1);
                                    }
                                }
                            } else {
                                match parse_mode(&rest) {
                                    Ok(m) => config.mode = m,
                                    Err(e) => {
                                        eprintln!("{}: {}", TOOL_NAME, e);
                                        process::exit(1);
                                    }
                                }
                            }
                            break;
                        }
                        'g' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!(
                                        "{}: option requires an argument -- 'g'",
                                        TOOL_NAME
                                    );
                                    process::exit(1);
                                }
                                config.group = Some(args[i].clone());
                            } else {
                                config.group = Some(rest);
                            }
                            break;
                        }
                        'o' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!(
                                        "{}: option requires an argument -- 'o'",
                                        TOOL_NAME
                                    );
                                    process::exit(1);
                                }
                                config.owner = Some(args[i].clone());
                            } else {
                                config.owner = Some(rest);
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
                                    process::exit(1);
                                }
                                config.target_directory = Some(args[i].clone());
                            } else {
                                config.target_directory = Some(rest);
                            }
                            break;
                        }
                        'S' => {
                            let rest: String = chars[j + 1..].iter().collect();
                            if rest.is_empty() {
                                i += 1;
                                if i >= args.len() {
                                    eprintln!(
                                        "{}: option requires an argument -- 'S'",
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

    // -d mode: create directories
    if config.directory_mode {
        if operands.is_empty() {
            eprintln!("{}: missing file operand", TOOL_NAME);
            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
            process::exit(1);
        }
        let dirs: Vec<&Path> = operands.iter().map(|s| Path::new(s.as_str())).collect();
        if let Err(e) = install_directories(&dirs, &config) {
            eprintln!(
                "{}: {}",
                TOOL_NAME,
                coreutils_rs::common::io_error_msg(&e)
            );
            process::exit(1);
        }
        return;
    }

    if operands.is_empty() {
        eprintln!("{}: missing file operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    let mut exit_code = 0;

    if let Some(ref dir) = config.target_directory {
        // -t DIRECTORY SOURCE...
        if !Path::new(dir).is_dir() {
            eprintln!("{}: target '{}' is not a directory", TOOL_NAME, dir);
            process::exit(1);
        }
        for source in &operands {
            let src_path = Path::new(source);
            let basename = src_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| source.clone());
            let dst = Path::new(dir).join(&basename);
            if let Err(e) = install_file(src_path, &dst, &config) {
                eprintln!(
                    "{}: cannot install '{}' to '{}': {}",
                    TOOL_NAME,
                    source,
                    dst.display(),
                    coreutils_rs::common::io_error_msg(&e)
                );
                exit_code = 1;
            }
        }
    } else if config.no_target_directory {
        // -T: exactly two operands
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
        if let Err(e) = install_file(src, dst, &config) {
            eprintln!(
                "{}: cannot install '{}' to '{}': {}",
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
        let dst = Path::new(&operands[1]);

        if dst.is_dir() {
            // Install into directory
            let basename = src
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| operands[0].clone());
            let final_dst = dst.join(&basename);
            if let Err(e) = install_file(src, &final_dst, &config) {
                eprintln!(
                    "{}: cannot install '{}' to '{}': {}",
                    TOOL_NAME,
                    operands[0],
                    final_dst.display(),
                    coreutils_rs::common::io_error_msg(&e)
                );
                exit_code = 1;
            }
        } else {
            // -D: create leading dirs
            if config.create_leading
                && let Some(parent) = dst.parent()
                && !parent.as_os_str().is_empty()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                eprintln!(
                    "{}: cannot create directory '{}': {}",
                    TOOL_NAME,
                    parent.display(),
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
            if let Err(e) = install_file(src, dst, &config) {
                eprintln!(
                    "{}: cannot install '{}' to '{}': {}",
                    TOOL_NAME,
                    operands[0],
                    operands[1],
                    coreutils_rs::common::io_error_msg(&e)
                );
                exit_code = 1;
            }
        }
    } else {
        // Multiple operands: last must be a directory
        let dir = &operands[operands.len() - 1];
        if !Path::new(dir).is_dir() {
            eprintln!("{}: target '{}' is not a directory", TOOL_NAME, dir);
            process::exit(1);
        }
        for source in &operands[..operands.len() - 1] {
            let src_path = Path::new(source);
            let basename = src_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| source.clone());
            let final_dst = Path::new(dir).join(&basename);
            if let Err(e) = install_file(src_path, &final_dst, &config) {
                eprintln!(
                    "{}: cannot install '{}' to '{}': {}",
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
    println!("  or:  {} [OPTION]... -d DIRECTORY...", TOOL_NAME);
    println!();
    println!(
        "This install program copies files (often just compiled) into destination"
    );
    println!("locations you choose.  If you want to download and install a ready-to-use");
    println!("package on a GNU/Linux system, you should instead be using a package manager");
    println!("like yum(1) or apt-get(1).");
    println!();
    println!("In the first three forms, copy SOURCE to DEST or multiple SOURCE(s) to");
    println!("the existing DIRECTORY, while setting permission modes and owner/group.");
    println!("In the 4th form, create all components of the given DIRECTORY(ies).");
    println!();
    println!("  -b                         like --backup but does not accept an argument");
    println!("      --backup[=CONTROL]     make a backup of each existing destination file");
    println!("  -C, --compare              compare each pair of source and destination files,");
    println!("                               and in some cases, do not modify the destination");
    println!("  -d, --directory            treat all arguments as directory names; create all");
    println!("                               components of the specified directories");
    println!("  -D                         create all leading components of DEST except the");
    println!("                               last, or all components of --target-directory,");
    println!("                               then copy SOURCE to DEST");
    println!("  -g, --group=GROUP          set group ownership, instead of process' current group");
    println!("  -m, --mode=MODE            set permission mode (as in chmod), instead of rwxr-xr-x");
    println!("  -o, --owner=OWNER          set ownership (super-user only)");
    println!("  -p, --preserve-timestamps  apply access/modification times of SOURCE files");
    println!("                               to corresponding destination files");
    println!("  -s, --strip                strip symbol tables");
    println!("      --strip-program=PROGRAM  program used to strip binaries");
    println!("  -S, --suffix=SUFFIX        override the usual backup suffix");
    println!("  -t, --target-directory=DIRECTORY  copy all SOURCE arguments into DIRECTORY");
    println!("  -T, --no-target-directory   treat DEST as a normal file");
    println!("  -v, --verbose              print the name of each directory as it is created");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
}
