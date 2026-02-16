use std::process;

use coreutils_rs::common::reset_sigpipe;
use coreutils_rs::test_cmd;

fn main() {
    reset_sigpipe();

    let all_args: Vec<String> = std::env::args().collect();
    let program = &all_args[0];

    // Determine if invoked as "[" (bracket mode).
    // Check if the binary name (last component of path) is "[".
    let invoked_as_bracket = std::path::Path::new(program)
        .file_name()
        .is_some_and(|name| name == "[");

    let args = if invoked_as_bracket {
        let rest = &all_args[1..];
        if rest.is_empty() || rest[rest.len() - 1] != "]" {
            eprintln!("[: missing ']'");
            process::exit(2);
        }
        // Strip the trailing "]"
        &rest[..rest.len() - 1]
    } else {
        &all_args[1..]
    };

    match test_cmd::evaluate(args) {
        Ok(true) => process::exit(0),
        Ok(false) => process::exit(1),
        Err(msg) => {
            eprintln!("{}", msg);
            process::exit(2);
        }
    }
}
