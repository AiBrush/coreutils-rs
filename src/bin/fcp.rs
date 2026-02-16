use std::process;

use coreutils_rs::common::reset_sigpipe;
use coreutils_rs::cp::{
    apply_preserve, parse_backup_mode, parse_reflink_mode, CpConfig, DerefMode,
};

const TOOL_NAME: &str = "cp";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    print!(
        "\
Usage: cp [OPTION]... [-T] SOURCE DEST
  or:  cp [OPTION]... SOURCE... DIRECTORY
  or:  cp [OPTION]... -t DIRECTORY SOURCE...
Copy SOURCE to DEST, or multiple SOURCE(s) to DIRECTORY.

  -a, --archive              same as -dR --preserve=all
  -b                         like --backup but does not accept an argument
      --backup[=CONTROL]     make a backup of each existing destination file
  -d                         same as --no-dereference --preserve=links
  -f, --force                if an existing destination file cannot be
                               opened, remove it and try again
  -i, --interactive          prompt before overwrite
  -H                         follow command-line symbolic links in SOURCE
  -l, --link                 hard link files instead of copying
  -L, --dereference          always follow symbolic links in SOURCE
  -n, --no-clobber           do not overwrite an existing file
  -P, --no-dereference       never follow symbolic links in SOURCE
  -p                         same as --preserve=mode,ownership,timestamps
      --preserve[=ATTR_LIST] preserve the specified attributes (default:
                               mode,ownership,timestamps)
  -R, -r, --recursive        copy directories recursively
      --reflink[=WHEN]       control clone/CoW copies (auto, always, never)
  -s, --symbolic-link        make symbolic links instead of copying
  -S, --suffix=SUFFIX        override the usual backup suffix
  -t, --target-directory=DIR copy all SOURCE arguments into DIRECTORY
  -T, --no-target-directory  treat DEST as a normal file
  -u, --update               copy only when the SOURCE file is newer
  -v, --verbose              explain what is being done
  -x, --one-file-system      stay on this file system
      --help                 display this help and exit
      --version              output version information and exit
"
    );
}

fn main() {
    reset_sigpipe();

    let mut config = CpConfig::default();
    let mut positional: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        if saw_dashdash {
            positional.push(arg.clone());
            i += 1;
            continue;
        }

        if arg == "--" {
            saw_dashdash = true;
            i += 1;
            continue;
        }

        // Long options
        if arg.starts_with("--") {
            if let Some(eq_pos) = arg.find('=') {
                let (key, val) = arg.split_at(eq_pos);
                let val = &val[1..]; // skip '='
                match key {
                    "--backup" => match parse_backup_mode(val) {
                        Ok(m) => config.backup = Some(m),
                        Err(e) => {
                            eprintln!("cp: {}", e);
                            process::exit(1);
                        }
                    },
                    "--preserve" => apply_preserve(val, &mut config),
                    "--reflink" => match parse_reflink_mode(val) {
                        Ok(m) => config.reflink = m,
                        Err(e) => {
                            eprintln!("cp: {}", e);
                            process::exit(1);
                        }
                    },
                    "--suffix" => config.suffix = val.to_string(),
                    "--target-directory" => config.target_directory = Some(val.to_string()),
                    _ => {
                        eprintln!("cp: unrecognized option '{}'", arg);
                        eprintln!("Try 'cp --help' for more information.");
                        process::exit(1);
                    }
                }
                i += 1;
                continue;
            }

            match arg.as_str() {
                "--help" => {
                    print_help();
                    process::exit(0);
                }
                "--version" => {
                    println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                    process::exit(0);
                }
                "--archive" => {
                    config.dereference = DerefMode::Never;
                    config.recursive = true;
                    config.preserve_mode = true;
                    config.preserve_ownership = true;
                    config.preserve_timestamps = true;
                }
                "--backup" => {
                    config.backup = Some(coreutils_rs::cp::BackupMode::Existing);
                }
                "--force" => config.force = true,
                "--interactive" => config.interactive = true,
                "--link" => config.link = true,
                "--dereference" => config.dereference = DerefMode::Always,
                "--no-clobber" => config.no_clobber = true,
                "--no-dereference" => config.dereference = DerefMode::Never,
                "--preserve" => {
                    apply_preserve("mode,ownership,timestamps", &mut config);
                }
                "--recursive" => config.recursive = true,
                "--reflink" => config.reflink = coreutils_rs::cp::ReflinkMode::Auto,
                "--symbolic-link" => config.symbolic_link = true,
                "--no-target-directory" => config.no_target_directory = true,
                "--update" => config.update = true,
                "--verbose" => config.verbose = true,
                "--one-file-system" => config.one_file_system = true,
                _ => {
                    eprintln!("cp: unrecognized option '{}'", arg);
                    eprintln!("Try 'cp --help' for more information.");
                    process::exit(1);
                }
            }
            i += 1;
            continue;
        }

        // Short options
        if arg.starts_with('-') && arg.len() > 1 {
            let bytes = arg.as_bytes();
            let mut j = 1;
            while j < bytes.len() {
                match bytes[j] {
                    b'a' => {
                        config.dereference = DerefMode::Never;
                        config.recursive = true;
                        config.preserve_mode = true;
                        config.preserve_ownership = true;
                        config.preserve_timestamps = true;
                    }
                    b'b' => {
                        config.backup = Some(coreutils_rs::cp::BackupMode::Existing);
                    }
                    b'd' => {
                        config.dereference = DerefMode::Never;
                        // --preserve=links is acknowledged but links preservation
                        // is not yet fully implemented.
                    }
                    b'f' => config.force = true,
                    b'i' => config.interactive = true,
                    b'H' => config.dereference = DerefMode::CommandLine,
                    b'l' => config.link = true,
                    b'L' => config.dereference = DerefMode::Always,
                    b'n' => config.no_clobber = true,
                    b'P' => config.dereference = DerefMode::Never,
                    b'p' => {
                        config.preserve_mode = true;
                        config.preserve_ownership = true;
                        config.preserve_timestamps = true;
                    }
                    b'R' | b'r' => config.recursive = true,
                    b's' => config.symbolic_link = true,
                    b'S' => {
                        // -S SUFFIX: value is either rest of this arg or next arg.
                        let rest = &arg[(j + 1)..];
                        if !rest.is_empty() {
                            config.suffix = rest.to_string();
                        } else {
                            i += 1;
                            if i >= args.len() {
                                eprintln!("cp: option requires an argument -- 'S'");
                                process::exit(1);
                            }
                            config.suffix = args[i].clone();
                        }
                        j = bytes.len(); // consumed rest
                        continue;
                    }
                    b't' => {
                        // -t DIR
                        let rest = &arg[(j + 1)..];
                        if !rest.is_empty() {
                            config.target_directory = Some(rest.to_string());
                        } else {
                            i += 1;
                            if i >= args.len() {
                                eprintln!("cp: option requires an argument -- 't'");
                                process::exit(1);
                            }
                            config.target_directory = Some(args[i].clone());
                        }
                        j = bytes.len();
                        continue;
                    }
                    b'T' => config.no_target_directory = true,
                    b'u' => config.update = true,
                    b'v' => config.verbose = true,
                    b'x' => config.one_file_system = true,
                    _ => {
                        eprintln!("cp: invalid option -- '{}'", bytes[j] as char);
                        eprintln!("Try 'cp --help' for more information.");
                        process::exit(1);
                    }
                }
                j += 1;
            }
            i += 1;
            continue;
        }

        // Positional argument
        positional.push(arg.clone());
        i += 1;
    }

    if positional.is_empty() {
        eprintln!("cp: missing file operand");
        eprintln!("Try 'cp --help' for more information.");
        process::exit(1);
    }

    // If --target-directory is set, all positional args are sources.
    let (sources, dest) = if config.target_directory.is_some() {
        (positional.as_slice(), None)
    } else if positional.len() == 1 {
        eprintln!(
            "cp: missing destination file operand after '{}'",
            positional[0]
        );
        eprintln!("Try 'cp --help' for more information.");
        process::exit(1);
    } else {
        let (srcs, dst) = positional.split_at(positional.len() - 1);
        (srcs, Some(dst[0].as_str()))
    };

    let (errors, had_error) = coreutils_rs::cp::run_cp(sources, dest, &config);
    for e in &errors {
        eprintln!("{}", e);
    }

    if had_error {
        process::exit(1);
    }
}
