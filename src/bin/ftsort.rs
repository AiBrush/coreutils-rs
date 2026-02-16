// ftsort -- topological sort
//
// Usage: tsort [OPTION] [FILE]
// Read pairs of strings from FILE (or stdin), representing edges in a
// directed graph, and output a topological ordering.

use std::collections::{HashMap, HashSet};
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

/// Perform topological sort using stack-based Kahn's algorithm.
/// Uses LIFO (stack) instead of FIFO (queue) to match GNU tsort's DFS-like ordering.
/// Returns Ok(sorted) if successful, or Err with cycle members if a cycle exists.
fn topological_sort(
    nodes: &[String],
    edges: &[(String, String)],
) -> Result<Vec<String>, Vec<String>> {
    // Build adjacency list and in-degree map
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    // Deduplicate nodes preserving input order
    let mut unique_nodes: Vec<String> = Vec::new();
    {
        let mut seen = HashSet::new();
        for node in nodes {
            if seen.insert(node.clone()) {
                unique_nodes.push(node.clone());
            }
        }
    }

    // Initialize all nodes
    for node in &unique_nodes {
        adj.entry(node.clone()).or_default();
        in_degree.entry(node.clone()).or_insert(0);
    }

    // Add edges, deduplicating (matches GNU behavior)
    let mut edge_set: HashSet<(String, String)> = HashSet::new();
    for (from, to) in edges {
        if from != to && edge_set.insert((from.clone(), to.clone())) {
            adj.entry(from.clone()).or_default().push(to.clone());
            *in_degree.entry(to.clone()).or_insert(0) += 1;
        }
    }

    let total = unique_nodes.len();

    // Stack-based Kahn's algorithm (LIFO instead of FIFO)
    // This matches GNU tsort's DFS-like output ordering
    let mut stack: Vec<String> = Vec::new();
    let mut result: Vec<String> = Vec::new();

    // Push initial zero-degree nodes in reverse input order
    // so the first-in-input node is on top and gets processed first
    let zero_degree: Vec<String> = unique_nodes
        .iter()
        .filter(|n| in_degree.get(*n).copied().unwrap_or(0) == 0)
        .cloned()
        .collect();
    for node in zero_degree.into_iter().rev() {
        stack.push(node);
    }

    while let Some(node) = stack.pop() {
        result.push(node.clone());
        if let Some(neighbors) = adj.get(&node) {
            // Collect newly freed successors, then push in reverse order
            // so that the first successor in adjacency list is on top of the stack
            // and gets processed first (matching GNU tsort's DFS ordering)
            let mut new_zeros = Vec::new();
            for neighbor in neighbors {
                if let Some(deg) = in_degree.get_mut(neighbor)
                    && *deg > 0
                {
                    *deg -= 1;
                    if *deg == 0 {
                        new_zeros.push(neighbor.clone());
                    }
                }
            }
            for n in new_zeros.into_iter().rev() {
                stack.push(n);
            }
        }
    }

    if result.len() != total {
        // Cycle detected -- find the nodes in the cycle, preserving input order
        let result_set: HashSet<&String> = result.iter().collect();
        let cycle_members: Vec<String> = unique_nodes
            .iter()
            .filter(|n| !result_set.contains(n))
            .cloned()
            .collect();
        Err(cycle_members)
    } else {
        Ok(result)
    }
}

fn run(input: &str, source_name: &str) -> i32 {
    let mut all_nodes: Vec<String> = Vec::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut seen_nodes: HashMap<String, bool> = HashMap::new();

    // Parse tokens from input
    let tokens: Vec<&str> = input.split_whitespace().collect();

    if !tokens.len().is_multiple_of(2) {
        eprintln!("{}: input contains an odd number of tokens", TOOL_NAME);
        return 1;
    }

    for pair in tokens.chunks(2) {
        let from = pair[0].to_string();
        let to = pair[1].to_string();

        if !seen_nodes.contains_key(&from) {
            seen_nodes.insert(from.clone(), true);
            all_nodes.push(from.clone());
        }
        if !seen_nodes.contains_key(&to) {
            seen_nodes.insert(to.clone(), true);
            all_nodes.push(to.clone());
        }

        edges.push((from, to));
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();

    match topological_sort(&all_nodes, &edges) {
        Ok(sorted) => {
            for node in &sorted {
                let _ = writeln!(out, "{node}");
            }
            0
        }
        Err(cycle_members) => {
            // Print what we can, then report cycle
            // GNU tsort outputs nodes as it can, reporting loops inline
            eprintln!("{}: {}: input contains a loop:", TOOL_NAME, source_name);
            for member in &cycle_members {
                eprintln!("{}: {member}", TOOL_NAME);
            }

            // Output partial results using stack-based Kahn's, then cycle members
            let mut adj: HashMap<String, Vec<String>> = HashMap::new();
            let mut in_deg: HashMap<String, usize> = HashMap::new();
            let mut unique: Vec<String> = Vec::new();
            {
                let mut seen = HashSet::new();
                for node in &all_nodes {
                    if seen.insert(node.clone()) {
                        unique.push(node.clone());
                    }
                }
            }
            for node in &unique {
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

            let mut stack: Vec<String> = Vec::new();
            let zero: Vec<String> = unique
                .iter()
                .filter(|n| in_deg.get(*n).copied().unwrap_or(0) == 0)
                .cloned()
                .collect();
            for n in zero.into_iter().rev() {
                stack.push(n);
            }
            let mut resolved: Vec<String> = Vec::new();
            while let Some(node) = stack.pop() {
                resolved.push(node.clone());
                if let Some(neighbors) = adj.get(&node) {
                    let mut new_zeros = Vec::new();
                    for nb in neighbors {
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
                        stack.push(n);
                    }
                }
            }

            for node in &resolved {
                let _ = writeln!(out, "{node}");
            }

            // Output remaining cycle nodes in input order
            let resolved_set: HashSet<&String> = resolved.iter().collect();
            for node in &unique {
                if !resolved_set.contains(node) {
                    let _ = writeln!(out, "{node}");
                }
            }
            1
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
