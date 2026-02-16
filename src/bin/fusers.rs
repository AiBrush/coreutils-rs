#[cfg(not(unix))]
fn main() {
    eprintln!("users: only available on Unix");
    std::process::exit(1);
}

// fusers -- print the user names of users currently logged in
//
// Usage: users [FILE]
//
// Prints a space-separated sorted list of login names from utmpx.

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    for arg in &args {
        match arg.as_str() {
            "--help" => {
                println!("Usage: users [OPTION]... [FILE]");
                println!("Output who is currently logged in according to FILE.");
                println!("If FILE is not specified, use /var/run/utmp.");
                println!();
                println!("      --help     display this help and exit");
                println!("      --version  output version information and exit");
                return;
            }
            "--version" => {
                println!("users (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => {}
        }
    }

    let users = coreutils_rs::users::get_users();
    let output = coreutils_rs::users::format_users(&users);
    if !output.is_empty() {
        println!("{}", output);
    }
}
