// frm — remove files or directories
//
// Usage: rm [OPTION]... [FILE]...

use std::process;

use coreutils_rs::rm::{InteractiveMode, PreserveRoot, RmConfig};

const TOOL_NAME: &str = "rm";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("Usage: {} [OPTION]... [FILE]...", TOOL_NAME);
    println!("Remove (unlink) the FILE(s).");
    println!();
    println!("  -f, --force           ignore nonexistent files and arguments, never prompt");
    println!("  -i                    prompt before every removal");
    println!(
        "  -I                    prompt once before removing more than three files, or"
    );
    println!("                          when removing recursively");
    println!(
        "      --interactive[=WHEN]  prompt according to WHEN: never, once (-I), or"
    );
    println!("                          always (-i); without WHEN, prompt always");
    println!(
        "      --one-file-system  when removing a hierarchy recursively, skip any"
    );
    println!("                          directory that is on a file system different from");
    println!("                          that of the corresponding command line argument");
    println!(
        "      --no-preserve-root  do not treat '/' specially"
    );
    println!(
        "      --preserve-root[=all]  do not remove '/' (default); with 'all',"
    );
    println!("                          reject any command line argument on a separate device");
    println!(
        "  -r, -R, --recursive   remove directories and their contents recursively"
    );
    println!("  -d, --dir             remove empty directories");
    println!("  -v, --verbose         explain what is being done");
    println!("      --help            display this help and exit");
    println!("      --version         output version information and exit");
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut config = RmConfig::default();
    let mut files: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        if saw_dashdash {
            files.push(arg.clone());
            i += 1;
            continue;
        }

        match arg.as_str() {
            "--" => {
                saw_dashdash = true;
            }
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "--force" => config.force = true,
            "--recursive" => config.recursive = true,
            "--dir" => config.dir = true,
            "--verbose" => config.verbose = true,
            "--one-file-system" => config.one_file_system = true,
            "--no-preserve-root" => config.preserve_root = PreserveRoot::No,
            "--preserve-root" => config.preserve_root = PreserveRoot::Yes,
            "--preserve-root=all" => config.preserve_root = PreserveRoot::All,
            "--interactive" => config.interactive = InteractiveMode::Always,
            s if s.starts_with("--interactive=") => {
                let val = &s["--interactive=".len()..];
                match val {
                    "never" => config.interactive = InteractiveMode::Never,
                    "once" => config.interactive = InteractiveMode::Once,
                    "always" => config.interactive = InteractiveMode::Always,
                    _ => {
                        eprintln!(
                            "{}: invalid argument '{}' for '--interactive'",
                            TOOL_NAME, val
                        );
                        process::exit(1);
                    }
                }
            }
            // Short options: may be combined (e.g. -rf, -riv)
            s if s.starts_with('-') && !s.starts_with("--") && s.len() > 1 => {
                for ch in s[1..].chars() {
                    match ch {
                        'f' => {
                            config.force = true;
                            // -f cancels prior -i/-I
                            config.interactive = InteractiveMode::Never;
                        }
                        'i' => {
                            config.interactive = InteractiveMode::Always;
                            // -i cancels -f
                            config.force = false;
                        }
                        'I' => {
                            config.interactive = InteractiveMode::Once;
                            // -I cancels -f
                            config.force = false;
                        }
                        'r' | 'R' => config.recursive = true,
                        'd' => config.dir = true,
                        'v' => config.verbose = true,
                        _ => {
                            eprintln!(
                                "{}: invalid option -- '{}'",
                                TOOL_NAME, ch
                            );
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            _ => files.push(arg.clone()),
        }

        i += 1;
    }

    // GNU rm: with no operands (and no -f), print usage error.
    if files.is_empty() {
        if config.force {
            // rm -f with no operands is a successful no-op.
            return;
        }
        eprintln!("{}: missing operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    // -I prompts once before removing more than 3 files or when recursive.
    if config.interactive == InteractiveMode::Once {
        let should_prompt = files.len() > 3 || config.recursive;
        if should_prompt {
            eprint!(
                "{}: remove {} argument{}? ",
                TOOL_NAME,
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            );
            let mut answer = String::new();
            if std::io::stdin().read_line(&mut answer).is_err() {
                process::exit(1);
            }
            let trimmed = answer.trim();
            if !trimmed.eq_ignore_ascii_case("y") && !trimmed.eq_ignore_ascii_case("yes") {
                process::exit(0);
            }
        }
    }

    let mut exit_code = 0;
    for file in &files {
        let path = std::path::Path::new(file);
        match coreutils_rs::rm::rm_path(path, &config) {
            Ok(true) => {}
            Ok(false) => exit_code = 1,
            Err(e) => {
                eprintln!("{}: cannot remove '{}': {}", TOOL_NAME, file, e);
                exit_code = 1;
            }
        }
    }

    if exit_code != 0 {
        process::exit(exit_code);
    }
}
