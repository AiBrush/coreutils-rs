use std::process;

use coreutils_rs::common::reset_sigpipe;
use coreutils_rs::dd::{self, DdConfig};

fn main() {
    reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // Handle --help and --version before parsing key=value operands
    for arg in &args {
        match arg.as_str() {
            "--help" => {
                dd::print_help();
                process::exit(0);
            }
            "--version" => {
                dd::print_version();
                process::exit(0);
            }
            _ => {}
        }
    }

    let config: DdConfig = match dd::parse_dd_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("dd: {}", e);
            process::exit(1);
        }
    };

    match dd::dd_copy(&config) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("dd: {}", e);
            process::exit(1);
        }
    }
}
