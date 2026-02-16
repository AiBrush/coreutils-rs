// fruncon -- run command with specified SELinux security context
//
// Usage: runcon CONTEXT COMMAND [ARG]...
//        runcon [-c] [-u USER] [-r ROLE] [-t TYPE] [-l RANGE] COMMAND [ARG]...
//
// This is a stub implementation. SELinux is not supported in this build.

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--help") {
        println!("Usage: runcon CONTEXT COMMAND [ARG]...");
        println!("  or:  runcon [-c] [-u USER] [-r ROLE] [-t TYPE] [-l RANGE] COMMAND [ARG]...");
        println!("Run a program in a different SELinux security context.");
        println!("With neither CONTEXT nor COMMAND, print the current security context.");
        println!();
        println!("  -c, --compute          compute process transition context before modifying");
        println!("  -u, --user=USER        set user USER in the target security context");
        println!("  -r, --role=ROLE        set role ROLE in the target security context");
        println!("  -t, --type=TYPE        set type TYPE in the target security context");
        println!("  -l, --range=RANGE      set range RANGE in the target security context");
        println!("      --help             display this help and exit");
        println!("      --version          output version information and exit");
        println!();
        println!("SELinux is not supported in this build.");
        return;
    }

    if args.iter().any(|a| a == "--version") {
        println!("runcon (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    eprintln!("runcon: SELinux is not enabled on this system");
    std::process::exit(1);
}
