#[cfg(not(unix))]
fn main() {
    eprintln!("who: only available on Unix");
    std::process::exit(1);
}

// fwho -- show who is logged on
//
// Usage: who [OPTION]... [ FILE | ARG1 ARG2 ]
//
// Reads utmpx records to display login information.

#[cfg(unix)]
use std::process;

#[cfg(unix)]
use clap::Parser;

#[cfg(unix)]
use coreutils_rs::who;

#[cfg(unix)]
#[derive(Parser)]
#[command(
    name = "who",
    about = "Show who is logged on",
    after_help = "If ARG1 ARG2 given (e.g. 'who am i'), print only the entry for the current terminal."
)]
struct Cli {
    /// same as -b -d --login -p -r -t -T -u
    #[arg(short = 'a', long = "all")]
    all: bool,

    /// time of last system boot
    #[arg(short = 'b', long = "boot")]
    boot: bool,

    /// print dead processes
    #[arg(short = 'd', long = "dead")]
    dead: bool,

    /// print line of column headings
    #[arg(short = 'H', long = "heading")]
    heading: bool,

    /// print system login processes
    #[arg(short = 'l', long = "login")]
    login: bool,

    /// only hostname and user associated with stdin
    #[arg(short = 'm')]
    only_current: bool,

    /// print active processes spawned by init
    #[arg(short = 'p', long = "process")]
    init_process: bool,

    /// all login names and number of users logged on
    #[arg(short = 'q', long = "count")]
    count: bool,

    /// print current runlevel
    #[arg(short = 'r', long = "runlevel")]
    runlevel: bool,

    /// print only name, line, and time (default)
    #[arg(short = 's', long = "short")]
    short: bool,

    /// print last system clock change
    #[arg(short = 't', long = "time")]
    time: bool,

    /// add user's message status as +, - or ?
    #[arg(short = 'T', short_alias = 'w', long = "mesg")]
    mesg: bool,

    /// list users logged in
    #[arg(short = 'u', long = "users")]
    users: bool,

    /// print ips instead of hostnames
    #[arg(long = "ips")]
    ips: bool,

    /// attempt to canonicalize hostnames via DNS
    #[arg(long = "lookup")]
    lookup: bool,

    /// optional positional arguments (FILE or "am i")
    #[arg(trailing_var_arg = true)]
    args: Vec<String>,
}

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();
    let cli = Cli::parse();

    let mut config = who::WhoConfig::default();

    // Check for "who am i" / "who am I" pattern (exactly 2 extra args)
    if cli.args.len() == 2 {
        let a = cli.args[0].to_lowercase();
        let b = cli.args[1].to_lowercase();
        if a == "am" && (b == "i" || b == "I") {
            config.am_i = true;
        }
    }

    config.show_boot = cli.boot;
    config.show_dead = cli.dead;
    config.show_heading = cli.heading;
    config.show_login = cli.login;
    config.only_current = cli.only_current;
    config.show_init_spawn = cli.init_process;
    config.show_count = cli.count;
    config.show_runlevel = cli.runlevel;
    config.short_format = cli.short;
    config.show_clock_change = cli.time;
    config.show_mesg = cli.mesg;
    config.show_users = cli.users;
    config.show_ips = cli.ips;
    config.show_lookup = cli.lookup;

    if cli.all {
        config.show_all = true;
        config.apply_all();
    }

    let output = who::run_who(&config);
    if !output.is_empty() {
        println!("{}", output);
    }

    process::exit(0);
}
