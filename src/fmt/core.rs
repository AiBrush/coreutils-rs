use std::io::{self, BufRead, Write};

/// Configuration for the fmt command.
pub struct FmtConfig {
    /// Maximum line width (default 75).
    pub width: usize,
    /// Goal width for line filling (default 93% of width).
    pub goal: usize,
    /// Only split long lines, do not refill short lines.
    pub split_only: bool,
    /// Crown margin mode: preserve the indentation of the first two lines.
    pub crown_margin: bool,
    /// Tagged paragraph mode: first line indentation differs from subsequent lines.
    pub tagged: bool,
    /// Uniform spacing: one space between words, two after sentence-ending punctuation.
    pub uniform_spacing: bool,
    /// Only reformat lines beginning with this prefix.
    pub prefix: Option<String>,
}

impl Default for FmtConfig {
    fn default() -> Self {
        let width = 75;
        Self {
            width,
            goal: (width * 93) / 100,
            split_only: false,
            crown_margin: false,
            tagged: false,
            uniform_spacing: false,
            prefix: None,
        }
    }
}

/// Reformat text from `input` and write the result to `output`.
///
/// Text is processed paragraph by paragraph (paragraphs are separated by blank lines).
/// Each paragraph's words are reflowed to fit within the configured width using
/// greedy line breaking.
pub fn fmt_file<R: BufRead, W: Write>(
    input: R,
    output: &mut W,
    config: &FmtConfig,
) -> io::Result<()> {
    let mut paragraphs: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();

    for line in input.lines() {
        let line = line?;

        // If a prefix is set, only reformat lines that start with it.
        // Lines without the prefix are emitted verbatim.
        if let Some(ref pfx) = config.prefix {
            if !line.starts_with(pfx.as_str()) {
                // Flush current paragraph before emitting non-prefix line.
                if !current.is_empty() {
                    paragraphs.push(current);
                    current = Vec::new();
                }
                // Emit as a single-line paragraph marked for verbatim output.
                // We use a special sentinel: a paragraph whose sole entry is the
                // raw line prefixed with a NUL byte so we can distinguish it later.
                paragraphs.push(vec![format!("\x00{}", line)]);
                continue;
            }
        }

        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(current);
                current = Vec::new();
            }
            // Represent a blank line as an empty paragraph.
            paragraphs.push(Vec::new());
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }

    for para in &paragraphs {
        // Blank line separator.
        if para.is_empty() {
            output.write_all(b"\n")?;
            continue;
        }

        // Check for verbatim sentinel (non-prefix line).
        if para.len() == 1 && para[0].starts_with('\x00') {
            output.write_all(&para[0].as_bytes()[1..])?;
            output.write_all(b"\n")?;
            continue;
        }

        format_paragraph(para, config, output)?;
    }

    Ok(())
}

/// Determine the leading whitespace (indentation) of a line.
fn leading_indent(line: &str) -> &str {
    let trimmed = line.trim_start();
    &line[..line.len() - trimmed.len()]
}

/// Check if a word ends a sentence (ends with '.', '!', or '?').
fn is_sentence_end(word: &str) -> bool {
    matches!(word.as_bytes().last(), Some(b'.' | b'!' | b'?'))
}

/// Extract words from a line, optionally stripping a prefix first.
fn extract_words<'a>(line: &'a str, prefix: Option<&str>) -> Vec<&'a str> {
    let s = match prefix {
        Some(pfx) => line.strip_prefix(pfx).unwrap_or(line),
        None => line,
    };
    s.split_whitespace().collect()
}

/// Format a single paragraph (a group of non-blank lines) and write it.
fn format_paragraph<W: Write>(
    lines: &[String],
    config: &FmtConfig,
    output: &mut W,
) -> io::Result<()> {
    if lines.is_empty() {
        return Ok(());
    }

    let prefix_str = config.prefix.as_deref();

    // Strip the prefix from lines for indentation analysis.
    let stripped_first = match prefix_str {
        Some(pfx) => lines[0].strip_prefix(pfx).unwrap_or(&lines[0]),
        None => &lines[0],
    };

    let first_indent = leading_indent(stripped_first).to_string();

    // Determine continuation indent.
    let rest_indent = if lines.len() > 1 {
        let stripped = match prefix_str {
            Some(pfx) => lines[1].strip_prefix(pfx).unwrap_or(&lines[1]),
            None => &lines[1],
        };
        leading_indent(stripped).to_string()
    } else {
        first_indent.clone()
    };

    // Choose indentation based on mode.
    let (first_line_indent, cont_indent) = if config.tagged {
        // Tagged paragraph: first line keeps its indent, rest uses second line's indent.
        (first_indent.clone(), rest_indent.clone())
    } else if config.crown_margin {
        // Crown margin: preserve the first two lines' indentation exactly.
        (first_indent.clone(), rest_indent.clone())
    } else {
        // Default: use the first line's indent for all lines.
        (first_indent.clone(), first_indent.clone())
    };

    // In split-only mode, we do not rejoin words across lines.
    if config.split_only {
        for line in lines {
            split_long_line(line, config, prefix_str, output)?;
        }
        return Ok(());
    }

    // Collect all words from the paragraph.
    let mut all_words: Vec<&str> = Vec::new();
    for line in lines {
        all_words.extend(extract_words(line, prefix_str));
    }

    if all_words.is_empty() {
        output.write_all(b"\n")?;
        return Ok(());
    }

    // Build the prefix string to prepend to each output line.
    let pfx = prefix_str.unwrap_or("");

    // Reflow the words.
    let result =
        reflow_paragraph(&all_words, pfx, &first_line_indent, &cont_indent, config);
    output.write_all(result.as_bytes())?;
    Ok(())
}

/// Reflow words into lines that fit within the configured width.
///
/// Uses optimal line breaking with a cost function matching GNU fmt:
///   SHORT_COST(n) = (n * 10)^2  where n = goal - line_length
///   RAGGED_COST(n) = (n * 10)^2 / 2  where n = line_length - next_line_length
///   Last line has zero cost.
fn reflow_paragraph(
    words: &[&str],
    prefix: &str,
    first_indent: &str,
    cont_indent: &str,
    config: &FmtConfig,
) -> String {
    if words.is_empty() {
        return String::new();
    }

    let n = words.len();
    let first_base = prefix.len() + first_indent.len();
    let cont_base = prefix.len() + cont_indent.len();
    let goal = config.goal as i64;
    let width = config.width;

    // Compute separator widths between consecutive words on the same line.
    let sep_widths: Vec<usize> = (0..n)
        .map(|i| {
            if i == 0 {
                0
            } else if config.uniform_spacing && is_sentence_end(words[i - 1]) {
                2
            } else {
                1
            }
        })
        .collect();

    let word_lens: Vec<usize> = words.iter().map(|w| w.len()).collect();

    // DP state:
    // cost[i] = minimum cost to format words[i..n]
    // best[i] = last word index of the optimal line starting at word i
    // first_line_len[i] = length of the first line in the optimal layout of words[i..n]
    // has_more_lines[i] = whether the sub-solution at i has more than one line
    let mut cost = vec![i64::MAX; n + 1];
    let mut best = vec![0usize; n];
    let mut first_line_len = vec![0i64; n + 1];
    let mut has_more_lines = vec![false; n + 1]; // true if sub-solution has >= 2 lines
    cost[n] = 0;

    for i in (0..n).rev() {
        let base = if i == 0 { first_base } else { cont_base };
        let mut len = base + word_lens[i];

        for j in i..n {
            if j > i {
                len += sep_widths[j] + word_lens[j];
            }

            if len > width {
                if j == i {
                    let line_cost = if j == n - 1 {
                        0
                    } else {
                        let short_n = goal - len as i64;
                        let short_cost = short_n * 10 * short_n * 10;
                        // Ragged cost only if the next sub-solution has more than one line
                        // (i.e., next line is not the last line of the paragraph)
                        let ragged_cost = if has_more_lines[j + 1] {
                            let ragged_n = len as i64 - first_line_len[j + 1];
                            ragged_n * 10 * ragged_n * 10 / 2
                        } else {
                            0
                        };
                        short_cost + ragged_cost
                    };
                    if cost[j + 1] != i64::MAX {
                        let total = line_cost + cost[j + 1];
                        if total < cost[i] {
                            cost[i] = total;
                            best[i] = j;
                            first_line_len[i] = len as i64;
                            has_more_lines[i] = j + 1 < n;
                        }
                    }
                }
                break;
            }

            let line_cost = if j == n - 1 {
                0
            } else {
                let short_n = goal - len as i64;
                let short_cost = short_n * 10 * short_n * 10;
                // Ragged cost only when the next line is not the last line
                let ragged_cost = if has_more_lines[j + 1] {
                    let ragged_n = len as i64 - first_line_len[j + 1];
                    ragged_n * 10 * ragged_n * 10 / 2
                } else {
                    0
                };
                short_cost + ragged_cost
            };

            if cost[j + 1] != i64::MAX {
                let total = line_cost + cost[j + 1];
                if total < cost[i] {
                    cost[i] = total;
                    best[i] = j;
                    first_line_len[i] = len as i64;
                    has_more_lines[i] = j + 1 < n;
                }
            }
        }
    }

    // Reconstruct the lines from the DP solution.
    let mut result = String::new();
    let mut i = 0;
    let mut is_first_line = true;

    while i < n {
        let j = best[i];
        let base = if is_first_line {
            format!("{}{}", prefix, first_indent)
        } else {
            format!("{}{}", prefix, cont_indent)
        };

        result.push_str(&base);
        result.push_str(words[i]);
        for k in (i + 1)..=j {
            if config.uniform_spacing && is_sentence_end(words[k - 1]) {
                result.push_str("  ");
            } else {
                result.push(' ');
            }
            result.push_str(words[k]);
        }
        result.push('\n');

        is_first_line = false;
        i = j + 1;
    }

    result
}

/// Split a single long line at the width boundary without reflowing.
/// Used in split-only mode (-s).
fn split_long_line<W: Write>(
    line: &str,
    config: &FmtConfig,
    prefix: Option<&str>,
    output: &mut W,
) -> io::Result<()> {
    let stripped = match prefix {
        Some(pfx) => line.strip_prefix(pfx).unwrap_or(line),
        None => line,
    };
    let indent = leading_indent(stripped).to_string();
    let pfx = prefix.unwrap_or("");

    if line.len() <= config.width {
        output.write_all(line.as_bytes())?;
        output.write_all(b"\n")?;
        return Ok(());
    }

    // Split the line's words, preserving the original structure as much as possible.
    let words: Vec<&str> = extract_words(line, prefix);
    if words.is_empty() {
        output.write_all(line.as_bytes())?;
        output.write_all(b"\n")?;
        return Ok(());
    }

    let mut cur_line = format!("{}{}", pfx, indent);
    for (i, word) in words.iter().enumerate() {
        let sep = if cur_line.len() == pfx.len() + indent.len() {
            ""
        } else if config.uniform_spacing && i > 0 && is_sentence_end(words[i - 1]) {
            "  "
        } else {
            " "
        };

        if cur_line.len() + sep.len() + word.len() > config.width
            && cur_line.len() > pfx.len() + indent.len()
        {
            output.write_all(cur_line.as_bytes())?;
            output.write_all(b"\n")?;
            cur_line = format!("{}{}", pfx, indent);
        }

        let sep = if cur_line.len() == pfx.len() + indent.len() {
            ""
        } else if config.uniform_spacing && i > 0 && is_sentence_end(words[i - 1]) {
            "  "
        } else {
            " "
        };
        cur_line.push_str(sep);
        cur_line.push_str(word);
    }

    if cur_line.len() > pfx.len() + indent.len() {
        output.write_all(cur_line.as_bytes())?;
        output.write_all(b"\n")?;
    }

    Ok(())
}
