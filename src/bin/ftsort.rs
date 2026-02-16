// ftsort -- topological sort
//
// Usage: tsort [OPTION] [FILE]
// Read pairs of strings from FILE (or stdin), representing edges in a
// directed graph, and output a topological ordering.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, BufRead, Write};
use std::process;

const TOOL_NAME: &str = "tsort";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("Usage: {} [OPTION] [FILE]", TOOL_NAME);
    println!("Write totally ordered list consistent with the partial ordering in FILE.");
    println!("With no FILE, or when FILE is -, read standard input.");
    println!();
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
}

fn print_version() {
    println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
}

fn run(input: &str, source_name: &str) -> i32 {
    let mut all_nodes: Vec<String> = Vec::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut seen_nodes: HashSet<String> = HashSet::new();

    // Parse tokens from input
    let tokens: Vec<&str> = input.split_whitespace().collect();

    if !tokens.len().is_multiple_of(2) {
        eprintln!(
            "{}: {}: input contains an odd number of tokens",
            TOOL_NAME, source_name
        );
        return 1;
    }

    for pair in tokens.chunks(2) {
        let from = pair[0].to_string();
        let to = pair[1].to_string();

        if seen_nodes.insert(from.clone()) {
            all_nodes.push(from.clone());
        }
        if seen_nodes.insert(to.clone()) {
            all_nodes.push(to.clone());
        }

        edges.push((from, to));
    }

    // Build graph (all_nodes is already deduplicated via seen_nodes)
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_deg: HashMap<String, usize> = HashMap::new();

    for node in &all_nodes {
        adj.entry(node.clone()).or_default();
        in_deg.entry(node.clone()).or_insert(0);
    }

    let mut edge_set: HashSet<(String, String)> = HashSet::new();
    for (from, to) in &edges {
        if from != to && edge_set.insert((from.clone(), to.clone())) {
            adj.entry(from.clone()).or_default().push(to.clone());
            *in_deg.entry(to.clone()).or_insert(0) += 1;
        }
    }

    let total = all_nodes.len();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    // Incremental Kahn's algorithm with cycle breaking (matches GNU behavior)
    let mut queue: VecDeque<String> = VecDeque::new();
    let mut processed = 0usize;
    let mut has_cycle = false;
    let mut removed: HashSet<String> = HashSet::new();

    // Seed queue with initial zero-degree nodes
    for node in &all_nodes {
        if in_deg.get(node).copied().unwrap_or(0) == 0 {
            queue.push_back(node.clone());
        }
    }

    loop {
        // Phase 1: process all zero-degree nodes
        while let Some(node) = queue.pop_front() {
            processed += 1;
            removed.insert(node.clone());
            let _ = writeln!(out, "{node}");
            if let Some(neighbors) = adj.get(&node) {
                let mut new_zeros = Vec::new();
                for nb in neighbors {
                    if removed.contains(nb) {
                        continue;
                    }
                    if let Some(d) = in_deg.get_mut(nb)
                        && *d > 0
                    {
                        *d -= 1;
                        if *d == 0 {
                            new_zeros.push(nb.clone());
                        }
                    }
                }
                for n in new_zeros.into_iter().rev() {
                    queue.push_back(n);
                }
            }
        }

        if processed >= total {
            break;
        }

        // Phase 2: cycle detected — find and report one cycle, then break it
        has_cycle = true;

        // Find the first unprocessed node in input order
        let start = all_nodes
            .iter()
            .find(|n| !removed.contains(*n))
            .unwrap()
            .clone();

        // Find the cycle by following edges from start using DFS
        let cycle = find_cycle(&start, &adj, &removed);

        eprintln!("{}: {}: input contains a loop:", TOOL_NAME, source_name);
        for member in &cycle {
            eprintln!("{}: {member}", TOOL_NAME);
        }

        // Break the cycle: force the first cycle member's in-degree to 0
        if let Some(d) = in_deg.get_mut(&cycle[0]) {
            *d = 0;
        }
        queue.push_back(cycle[0].clone());
    }

    if has_cycle { 1 } else { 0 }
}

/// Find a cycle starting from `start` by following edges via DFS.
/// Returns the cycle members in order.
fn find_cycle(
    start: &str,
    adj: &HashMap<String, Vec<String>>,
    removed: &HashSet<String>,
) -> Vec<String> {
    // Follow edges from start to find a cycle
    let mut visited: HashMap<String, usize> = HashMap::new();
    let mut path: Vec<String> = Vec::new();

    let mut current = start.to_string();
    loop {
        if let Some(&idx) = visited.get(&current) {
            // Found the cycle: extract from idx to end of path
            return path[idx..].to_vec();
        }
        visited.insert(current.clone(), path.len());
        path.push(current.clone());

        // Follow the first non-removed successor
        let next = adj
            .get(&current)
            .and_then(|neighbors| neighbors.iter().find(|n| !removed.contains(*n)));
        match next {
            Some(n) => current = n.clone(),
            None => {
                // No successor — shouldn't happen in a cycle, return just the node
                return vec![start.to_string()];
            }
        }
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut filename: Option<String> = None;

    for arg in &args {
        match arg.as_str() {
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                print_version();
                return;
            }
            "-" => {
                filename = None;
            }
            _ => {
                if arg.starts_with('-') {
                    eprintln!("{}: unrecognized option '{}'", TOOL_NAME, arg);
                    process::exit(1);
                }
                filename = Some(arg.clone());
            }
        }
    }

    let (input, source_name) = if let Some(ref file) = filename {
        match std::fs::read_to_string(file) {
            Ok(contents) => (contents, file.clone()),
            Err(e) => {
                eprintln!(
                    "{}: {}: {}",
                    TOOL_NAME,
                    file,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        }
    } else {
        let stdin = io::stdin();
        let mut input = String::new();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    input.push_str(&l);
                    input.push('\n');
                }
                Err(e) => {
                    eprintln!("{}: read error: {}", TOOL_NAME, e);
                    process::exit(1);
                }
            }
        }
        (input, "-".to_string())
    };

    let exit_code = run(&input, &source_name);
    process::exit(exit_code);
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("ftsort");
        Command::new(path)
    }

    #[test]
    fn test_basic_sort() {
        let output = cmd()
            .args(["-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                let stdin = child.stdin.as_mut().unwrap();
                stdin.write_all(b"a b\nb c\n").unwrap();
                drop(child.stdin.take());
                child.wait_with_output()
            })
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert_eq!(lines.len(), 3);
        // a must come before b, b must come before c
        let pos_a = lines.iter().position(|&x| x == "a").unwrap();
        let pos_b = lines.iter().position(|&x| x == "b").unwrap();
        let pos_c = lines.iter().position(|&x| x == "c").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn test_stdin_input() {
        use std::io::Write;
        let mut child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(b"x y\ny z\n").unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        let pos_x = lines.iter().position(|&x| x == "x").unwrap();
        let pos_y = lines.iter().position(|&x| x == "y").unwrap();
        let pos_z = lines.iter().position(|&x| x == "z").unwrap();
        assert!(pos_x < pos_y);
        assert!(pos_y < pos_z);
    }

    #[test]
    fn test_cycle_detection() {
        use std::io::Write;
        let mut child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(b"a b\nb c\nc a\n").unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("loop"));
    }

    #[test]
    fn test_single_element() {
        use std::io::Write;
        let mut child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(b"a a\n").unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "a");
    }

    #[test]
    fn test_file_input() {
        use std::io::Write;
        let dir = std::env::temp_dir();
        let file_path = dir.join("tsort_test_input.txt");
        {
            let mut f = std::fs::File::create(&file_path).unwrap();
            f.write_all(b"1 2\n2 3\n3 4\n").unwrap();
        }
        let output = cmd().arg(file_path.to_str().unwrap()).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert_eq!(lines, vec!["1", "2", "3", "4"]);
        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("tsort"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("tsort"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_match_gnu_basic() {
        use std::io::Write;

        let gnu = Command::new("tsort")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                let stdin = child.stdin.as_mut().unwrap();
                stdin.write_all(b"a b\nb c\nc d\n").unwrap();
                drop(child.stdin.take());
                child.wait_with_output()
            });

        if let Ok(gnu) = gnu {
            let mut child = cmd()
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            {
                let stdin = child.stdin.as_mut().unwrap();
                stdin.write_all(b"a b\nb c\nc d\n").unwrap();
            }
            let ours = child.wait_with_output().unwrap();
            assert_eq!(
                String::from_utf8_lossy(&ours.stdout),
                String::from_utf8_lossy(&gnu.stdout),
                "Output mismatch with GNU tsort"
            );
        }
    }

    #[test]
    fn test_diamond_dependency() {
        use std::io::Write;
        let mut child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(b"a b\na c\nb d\nc d\n").unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        let pos_a = lines.iter().position(|&x| x == "a").unwrap();
        let pos_b = lines.iter().position(|&x| x == "b").unwrap();
        let pos_c = lines.iter().position(|&x| x == "c").unwrap();
        let pos_d = lines.iter().position(|&x| x == "d").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
        assert!(pos_b < pos_d);
        assert!(pos_c < pos_d);
    }
}
