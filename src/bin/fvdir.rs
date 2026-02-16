#[cfg(not(unix))]
fn main() {
    eprintln!("vdir: only available on Unix");
    std::process::exit(1);
}

// fvdir -- list directory contents in long format with C-style escapes
//
// vdir is equivalent to: ls -l -b
//
// Since there is no ls module in this codebase, we delegate to the system ls
// with the appropriate default flags.

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // Handle --help and --version ourselves
    for arg in &args {
        match arg.as_str() {
            "--help" => {
                println!("Usage: vdir [OPTION]... [FILE]...");
                println!("List directory contents.");
                println!();
                println!("Equivalent to ls -l -b (long format with C-style escapes).");
                println!("All ls options are accepted.");
                println!();
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                return;
            }
            "--version" => {
                println!("vdir (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => {}
        }
    }

    // Build ls command with default flags: -l (long) -b (C-style escapes)
    // User-supplied args come after and can override defaults.
    let mut cmd_args: Vec<&str> = vec!["-l", "-b"];
    for arg in &args {
        cmd_args.push(arg.as_str());
    }

    let err = std::process::Command::new("ls")
        .args(&cmd_args)
        .exec();

    // If exec returns, it failed
    eprintln!(
        "vdir: failed to execute 'ls': {}",
        coreutils_rs::common::io_error_msg(&err)
    );
    std::process::exit(127);
}
