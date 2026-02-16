// fchcon -- change file SELinux security context
//
// Usage: chcon [OPTION]... CONTEXT FILE...
//        chcon [OPTION]... [-u USER] [-r ROLE] [-l RANGE] [-t TYPE] FILE...
//        chcon [OPTION]... --reference=RFILE FILE...
//
// This is a stub implementation. SELinux is not supported in this build.

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--help") {
        println!("Usage: chcon [OPTION]... CONTEXT FILE...");
        println!("  or:  chcon [OPTION]... [-u USER] [-r ROLE] [-l RANGE] [-t TYPE] FILE...");
        println!("  or:  chcon [OPTION]... --reference=RFILE FILE...");
        println!("Change the SELinux security context of each FILE to CONTEXT.");
        println!();
        println!("  -h, --no-dereference   affect symbolic links instead of referenced files");
        println!("      --reference=RFILE  use RFILE's security context");
        println!("  -R, --recursive        operate on files and directories recursively");
        println!("  -u, --user=USER        set user USER in the target security context");
        println!("  -r, --role=ROLE        set role ROLE in the target security context");
        println!("  -t, --type=TYPE        set type TYPE in the target security context");
        println!("  -l, --range=RANGE      set range RANGE in the target security context");
        println!("  -v, --verbose          output a diagnostic for every file processed");
        println!("      --help             display this help and exit");
        println!("      --version          output version information and exit");
        println!();
        println!("SELinux is not supported in this build.");
        return;
    }

    if args.iter().any(|a| a == "--version") {
        println!("chcon (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    eprintln!("chcon: SELinux is not enabled on this system");
    std::process::exit(1);
}
