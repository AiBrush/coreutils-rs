use std::collections::HashSet;
use std::io::{self, BufRead, Write};

/// Output format for ptx.
#[derive(Clone, Debug, PartialEq)]
pub enum OutputFormat {
    /// Default GNU ptx output format (roff-like).
    Roff,
    /// TeX output format.
    Tex,
    /// Dumb terminal / plain text format.
    Plain,
}

/// Configuration for the ptx command.
#[derive(Clone, Debug)]
pub struct PtxConfig {
    pub width: usize,
    pub ignore_case: bool,
    pub auto_reference: bool,
    pub traditional: bool,
    pub format: OutputFormat,
    pub ignore_words: HashSet<String>,
    pub only_words: Option<HashSet<String>>,
    pub references: bool,
    pub gap_size: usize,
    pub right_reference: bool,
    pub sentence_regexp: Option<String>,
    pub word_regexp: Option<String>,
}

impl Default for PtxConfig {
    fn default() -> Self {
        Self {
            width: 72,
            ignore_case: false,
            auto_reference: false,
            traditional: false,
            format: OutputFormat::Plain,
            ignore_words: HashSet::new(),
            only_words: None,
            references: false,
            gap_size: 3,
            right_reference: false,
            sentence_regexp: None,
            word_regexp: None,
        }
    }
}

/// A single KWIC (Key Word In Context) entry.
#[derive(Clone, Debug)]
struct KwicEntry {
    /// Reference (filename:line or line number).
    reference: String,
    /// Text before the keyword (left context).
    left_context: String,
    /// The keyword itself.
    keyword: String,
    /// Text after the keyword (right context).
    right_context: String,
    /// Sort key (lowercase keyword for case-insensitive sorting).
    sort_key: String,
}

/// Extract words from a line of text.
fn extract_words(line: &str) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let mut start = None;

    for (i, ch) in line.char_indices() {
        if ch.is_alphanumeric() || ch == '_' {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start {
            words.push((s, &line[s..i]));
            start = None;
        }
    }

    if let Some(s) = start {
        words.push((s, &line[s..]));
    }

    words
}

/// Check if a word should be indexed.
fn should_index(word: &str, config: &PtxConfig) -> bool {
    let check_word = if config.ignore_case {
        word.to_lowercase()
    } else {
        word.to_string()
    };

    // If only_words is set, the word must be in that set
    if let Some(ref only) = config.only_words {
        if config.ignore_case {
            return only.iter().any(|w| w.to_lowercase() == check_word);
        }
        return only.contains(&check_word);
    }

    // Otherwise, word must not be in ignore list
    if config.ignore_case {
        !config
            .ignore_words
            .iter()
            .any(|w| w.to_lowercase() == check_word)
    } else {
        !config.ignore_words.contains(&check_word)
    }
}

/// Generate KWIC entries from input lines.
fn generate_entries(
    lines: &[(String, String)],
    config: &PtxConfig,
) -> Vec<KwicEntry> {
    let mut entries = Vec::new();

    for (reference, line) in lines {
        let words = extract_words(line);

        for &(word_start, word) in &words {
            if !should_index(word, config) {
                continue;
            }

            let word_end = word_start + word.len();

            // Left context: text before the keyword
            let left = line[..word_start].trim_end();

            // Right context: text after the keyword
            let right = line[word_end..].trim_start();

            let sort_key = if config.ignore_case {
                word.to_lowercase()
            } else {
                word.to_string()
            };

            entries.push(KwicEntry {
                reference: reference.clone(),
                left_context: left.to_string(),
                keyword: word.to_string(),
                right_context: right.to_string(),
                sort_key,
            });
        }
    }

    // Sort by keyword (case-insensitive if requested), then by reference
    entries.sort_by(|a, b| {
        a.sort_key
            .cmp(&b.sort_key)
            .then_with(|| a.reference.cmp(&b.reference))
    });

    entries
}

/// Truncate a string from the left to fit within max_len characters.
fn truncate_left(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let skip = s.len() - max_len;
    // Find a valid char boundary
    let mut idx = skip;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    &s[idx..]
}

/// Truncate a string from the right to fit within max_len characters.
fn truncate_right(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let mut idx = max_len;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    &s[..idx]
}

/// Format a KWIC entry for plain text output.
fn format_plain(entry: &KwicEntry, config: &PtxConfig) -> String {
    let ref_str = if config.auto_reference || config.references {
        &entry.reference
    } else {
        ""
    };

    let total_width = config.width;
    let gap = config.gap_size;

    // Calculate available space
    let ref_width = if ref_str.is_empty() {
        0
    } else {
        ref_str.len() + gap
    };

    let available = if total_width > ref_width {
        total_width - ref_width
    } else {
        total_width
    };

    // Split available space: left context, keyword+right context
    // Allocate roughly half for left, half for keyword+right
    let right_half = available / 2;
    let left_half = available - right_half;

    // Left context (truncated from the left to fit)
    let left = truncate_left(&entry.left_context, if left_half > gap { left_half - gap } else { 0 });

    // Right side: keyword + right context
    let right_text = if entry.right_context.is_empty() {
        entry.keyword.clone()
    } else {
        format!("{} {}", entry.keyword, entry.right_context)
    };
    let right = truncate_right(&right_text, right_half);

    if ref_str.is_empty() {
        format!("{:>left_w$}{}{}", left, " ".repeat(gap), right, left_w = left_half - gap)
    } else if config.right_reference {
        format!(
            "{:>left_w$}{}{}{}{}",
            left,
            " ".repeat(gap),
            right,
            " ".repeat(gap),
            ref_str,
            left_w = left_half - gap,
        )
    } else {
        format!(
            "{}{}{:>left_w$}{}{}",
            ref_str,
            " ".repeat(gap),
            left,
            " ".repeat(gap),
            right,
            left_w = left_half - gap,
        )
    }
}

/// Format a KWIC entry for roff output.
fn format_roff(entry: &KwicEntry, config: &PtxConfig) -> String {
    let ref_str = if config.auto_reference || config.references {
        &entry.reference
    } else {
        ""
    };

    // Escape backslashes and quotes for roff
    let left = entry.left_context.replace('\\', "\\\\").replace('"', "\\\"");
    let keyword = entry.keyword.replace('\\', "\\\\").replace('"', "\\\"");
    let right = entry
        .right_context
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let reference = ref_str.replace('\\', "\\\\").replace('"', "\\\"");

    format!(
        ".xx \"{}\" \"{}\" \"{}\" \"{}\"",
        left, keyword, right, reference
    )
}

/// Format a KWIC entry for TeX output.
fn format_tex(entry: &KwicEntry, config: &PtxConfig) -> String {
    let ref_str = if config.auto_reference || config.references {
        &entry.reference
    } else {
        ""
    };

    // Escape TeX special characters
    fn escape_tex(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for ch in s.chars() {
            match ch {
                '\\' => result.push_str("\\backslash "),
                '{' => result.push_str("\\{"),
                '}' => result.push_str("\\}"),
                '$' => result.push_str("\\$"),
                '&' => result.push_str("\\&"),
                '#' => result.push_str("\\#"),
                '_' => result.push_str("\\_"),
                '^' => result.push_str("\\^{}"),
                '~' => result.push_str("\\~{}"),
                '%' => result.push_str("\\%"),
                _ => result.push(ch),
            }
        }
        result
    }

    format!(
        "\\xx {{{}}}{{{}}}{{{}}}{{{}}}",
        escape_tex(&entry.left_context),
        escape_tex(&entry.keyword),
        escape_tex(&entry.right_context),
        escape_tex(ref_str),
    )
}

/// Generate a permuted index from input.
///
/// Reads lines from `input`, generates KWIC entries for each indexable word,
/// sorts them, and writes the formatted output to `output`.
pub fn generate_ptx<R: BufRead, W: Write>(
    input: R,
    output: &mut W,
    config: &PtxConfig,
) -> io::Result<()> {
    // Read all lines with references
    let mut lines: Vec<(String, String)> = Vec::new();
    let mut line_num = 0usize;

    for line_result in input.lines() {
        let line = line_result?;
        line_num += 1;

        let reference = if config.auto_reference {
            format!("{}", line_num)
        } else {
            String::new()
        };

        lines.push((reference, line));
    }

    // Generate KWIC entries
    let entries = generate_entries(&lines, config);

    // Format and output
    for entry in &entries {
        let formatted = match config.format {
            OutputFormat::Plain => format_plain(entry, config),
            OutputFormat::Roff => format_roff(entry, config),
            OutputFormat::Tex => format_tex(entry, config),
        };
        writeln!(output, "{}", formatted)?;
    }

    Ok(())
}

/// Read a word list file (one word per line) into a HashSet.
pub fn read_word_file(path: &str) -> io::Result<HashSet<String>> {
    let content = std::fs::read_to_string(path)?;
    Ok(content.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}
