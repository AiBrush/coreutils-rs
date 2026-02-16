#[cfg(not(unix))]
fn main() {
    eprintln!("pinky: only available on Unix");
    std::process::exit(1);
}

// fpinky -- lightweight finger information lookup
//
// Usage: pinky [OPTION]... [USER]...
//
// A lightweight replacement for finger(1). Shows user login information
// from utmpx records and passwd entries.

#[cfg(unix)]
use std::process;

#[cfg(unix)]
use clap::Parser;

#[cfg(unix)]
use coreutils_rs::pinky;

#[cfg(unix)]
#[derive(Parser)]
#[command(name = "pinky", about = "Lightweight finger")]
struct Cli {
    /// produce long format output for the specified USERs
    #[arg(short = 'l')]
    long_format: bool,

    /// omit the user's home directory and shell in long format
    #[arg(short = 'b')]
    omit_home_shell: bool,

    /// omit the user's project file in long format
    #[arg(short = 'h')]
    omit_project: bool,

    /// omit the user's plan file in long format
    #[arg(short = 'p')]
    omit_plan: bool,

    /// do short format output (default)
    #[arg(short = 's')]
    short_format: bool,

    /// omit the column of full names in short format
    #[arg(short = 'f')]
    omit_heading: bool,

    /// omit the user's full name in short format
    #[arg(short = 'w')]
    omit_fullname: bool,

    /// omit the user's full name and remote host in short format
    #[arg(short = 'i')]
    omit_fullname_host: bool,

    /// omit the user's full name, remote host and idle time in short format
    #[arg(short = 'q')]
    omit_fullname_host_idle: bool,

    /// users to look up
    users: Vec<String>,
}

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();
    let cli = Cli::parse();

    let mut config = pinky::PinkyConfig::default();

    config.long_format = cli.long_format;
    config.omit_home_shell = cli.omit_home_shell;
    config.omit_project = cli.omit_project;
    config.omit_plan = cli.omit_plan;
    config.omit_heading = cli.omit_heading;
    config.omit_fullname = cli.omit_fullname;
    config.omit_fullname_host = cli.omit_fullname_host;
    config.omit_fullname_host_idle = cli.omit_fullname_host_idle;
    config.users = cli.users;

    if cli.short_format || !cli.long_format {
        config.short_format = true;
    }

    // If long format is explicitly requested, disable short format
    if cli.long_format {
        config.short_format = false;
    }

    let output = pinky::run_pinky(&config);
    if !output.is_empty() {
        println!("{}", output);
    }

    process::exit(0);
}
