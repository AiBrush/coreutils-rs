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
    // Parse tokens
    let tokens: Vec<&str> = input.split_whitespace().collect();

    if !tokens.len().is_multiple_of(2) {
        eprintln!(
            "{}: {}: input contains an odd number of tokens",
            TOOL_NAME, source_name
        );
        return 1;
    }

    // Use integer node IDs to avoid string cloning throughout the algorithm
    let mut node_names: Vec<String> = Vec::new();
    let mut name_to_id: HashMap<String, usize> = HashMap::new();

    let get_id = |name: &str, names: &mut Vec<String>, map: &mut HashMap<String, usize>| -> usize {
        use std::collections::hash_map::Entry;
        match map.entry(name.to_string()) {
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => {
                let id = names.len();
                names.push(e.key().clone());
                *e.insert(id)
            }
        }
    };

    let mut edge_pairs: Vec<(usize, usize)> = Vec::with_capacity(tokens.len() / 2);
    for pair in tokens.chunks(2) {
        let from = get_id(pair[0], &mut node_names, &mut name_to_id);
        let to = get_id(pair[1], &mut node_names, &mut name_to_id);
        edge_pairs.push((from, to));
    }

    let total = node_names.len();

    // Build graph using integer IDs — Vec-based adjacency for cache locality
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); total];
    let mut in_deg: Vec<usize> = vec![0; total];
    let mut edge_set: HashSet<(usize, usize)> = HashSet::with_capacity(edge_pairs.len());

    for &(from, to) in &edge_pairs {
        if from != to && edge_set.insert((from, to)) {
            adj[from].push(to);
            in_deg[to] += 1;
        }
    }

    let stdout = io::stdout();
    let mut out = std::io::BufWriter::with_capacity(256 * 1024, stdout.lock());

    // Incremental Kahn's algorithm with cycle breaking (matches GNU behavior)
    let mut queue: VecDeque<usize> = VecDeque::new();
    let mut processed = 0usize;
    let mut has_cycle = false;
    let mut removed = vec![false; total];

    // Seed queue with initial zero-degree nodes
    for (id, &deg) in in_deg.iter().enumerate().take(total) {
        if deg == 0 {
            queue.push_back(id);
        }
    }

    loop {
        // Phase 1: process all zero-degree nodes
        while let Some(node) = queue.pop_front() {
            processed += 1;
            removed[node] = true;
            let _ = writeln!(out, "{}", node_names[node]);
            let mut new_zeros = Vec::new();
            for &nb in &adj[node] {
                if removed[nb] {
                    continue;
                }
                if in_deg[nb] > 0 {
                    in_deg[nb] -= 1;
                    if in_deg[nb] == 0 {
                        new_zeros.push(nb);
                    }
                }
            }
            for n in new_zeros.into_iter().rev() {
                queue.push_back(n);
            }
        }

        if processed >= total {
            break;
        }

        // Phase 2: cycle detected — find and report one cycle, then break it
        has_cycle = true;

        let start = (0..total).find(|&n| !removed[n]).unwrap();
        let cycle = find_cycle(start, &adj, &removed);

        let _ = out.flush();
        eprintln!("{}: {}: input contains a loop:", TOOL_NAME, source_name);
        for &member in &cycle {
            eprintln!("{}: {}", TOOL_NAME, node_names[member]);
        }

        in_deg[cycle[0]] = 0;
        queue.push_back(cycle[0]);
    }

    let _ = out.flush();
    if has_cycle { 1 } else { 0 }
}

/// Find a cycle starting from `start` by following edges via DFS.
/// Uses integer node IDs for efficiency.
fn find_cycle(start: usize, adj: &[Vec<usize>], removed: &[bool]) -> Vec<usize> {
    let mut visited: HashMap<usize, usize> = HashMap::new();
    let mut path: Vec<usize> = Vec::new();

    let mut current = start;
    loop {
        if let Some(&idx) = visited.get(&current) {
            return path[idx..].to_vec();
        }
        visited.insert(current, path.len());
        path.push(current);

        let next = adj[current].iter().find(|&&n| !removed[n]);
        match next {
            Some(&n) => current = n,
            None => return vec![start],
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
