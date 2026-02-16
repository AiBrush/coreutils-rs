use std::io::{self, Write};
use std::process;

use coreutils_rs::common::reset_sigpipe;
use coreutils_rs::echo::{echo_output, parse_echo_args};

fn main() {
    reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let (config, text_args) = parse_echo_args(&args);
    let output = echo_output(text_args, &config);

    let stdout = io::stdout();
    let mut out = stdout.lock();
    if let Err(e) = out.write_all(&output) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            process::exit(0);
        }
        eprintln!("echo: write error: {}", e);
        process::exit(1);
    }
}
