use std::fmt;

use regex::Regex;

/// Exit code: expression is non-null and non-zero.
pub const EXIT_SUCCESS: i32 = 0;
/// Exit code: expression is null or zero.
pub const EXIT_FAILURE: i32 = 1;
/// Exit code: expression is syntactically invalid.
pub const EXIT_EXPR_ERROR: i32 = 2;
/// Exit code: regex error.
pub const EXIT_REGEX_ERROR: i32 = 3;

/// A value produced by evaluating an expr expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprValue {
    Integer(i64),
    Str(String),
}

impl fmt::Display for ExprValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprValue::Integer(n) => write!(f, "{}", n),
            ExprValue::Str(s) => write!(f, "{}", s),
        }
    }
}

impl ExprValue {
    /// Returns true if this value is considered "null" (empty string or integer 0).
    pub fn is_null(&self) -> bool {
        match self {
            ExprValue::Integer(n) => *n == 0,
            ExprValue::Str(s) => s.is_empty() || s == "0",
        }
    }

    /// Try to interpret this value as an integer.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            ExprValue::Integer(n) => Some(*n),
            ExprValue::Str(s) => parse_integer(s),
        }
    }
}

/// Parse an integer from a string, accepting optional leading sign and digits only.
fn parse_integer(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (sign, digits) = if let Some(rest) = s.strip_prefix('-') {
        (-1i64, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (1i64, rest)
    } else {
        (1i64, s)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok().map(|v| sign * v)
}

/// Errors that can occur during expression evaluation.
#[derive(Debug, Clone)]
pub enum ExprError {
    /// Syntax error in the expression.
    Syntax(String),
    /// Division by zero.
    DivisionByZero,
    /// Invalid regex pattern.
    RegexError(String),
    /// Non-integer argument where integer was required.
    NonIntegerArgument,
    /// Missing operand.
    MissingOperand,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprError::Syntax(msg) => write!(f, "syntax error: {}", msg),
            ExprError::DivisionByZero => write!(f, "division by zero"),
            ExprError::RegexError(msg) => write!(f, "{}", msg),
            ExprError::NonIntegerArgument => write!(f, "non-integer argument"),
            ExprError::MissingOperand => write!(f, "missing operand"),
        }
    }
}

impl ExprError {
    /// Returns the exit code for this error type.
    pub fn exit_code(&self) -> i32 {
        match self {
            ExprError::RegexError(_) => EXIT_REGEX_ERROR,
            _ => EXIT_EXPR_ERROR,
        }
    }
}

/// Recursive descent parser for expr expressions.
struct ExprParser<'a> {
    args: &'a [String],
    pos: usize,
}

impl<'a> ExprParser<'a> {
    fn new(args: &'a [String]) -> Self {
        ExprParser { args, pos: 0 }
    }

    /// Peek at the current token without consuming it.
    fn peek(&self) -> Option<&str> {
        if self.pos < self.args.len() {
            Some(self.args[self.pos].as_str())
        } else {
            None
        }
    }

    /// Consume the current token and advance.
    fn consume(&mut self) -> Option<&str> {
        if self.pos < self.args.len() {
            let tok = self.args[self.pos].as_str();
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    /// Expect a specific token, returning an error if not found.
    fn expect(&mut self, expected: &str) -> Result<(), ExprError> {
        match self.consume() {
            Some(tok) if tok == expected => Ok(()),
            Some(tok) => Err(ExprError::Syntax(format!(
                "expected '{}', found '{}'",
                expected, tok
            ))),
            None => Err(ExprError::Syntax(format!("expected '{}'", expected))),
        }
    }

    /// Parse the top-level: OR expression.
    /// OR: AND ( '|' AND )*
    fn parse_or(&mut self) -> Result<ExprValue, ExprError> {
        let mut left = self.parse_and()?;
        while self.peek() == Some("|") {
            self.consume();
            let right = self.parse_and()?;
            left = if !left.is_null() { left } else { right };
        }
        Ok(left)
    }

    /// Parse AND expression.
    /// AND: COMPARISON ( '&' COMPARISON )*
    fn parse_and(&mut self) -> Result<ExprValue, ExprError> {
        let mut left = self.parse_comparison()?;
        while self.peek() == Some("&") {
            self.consume();
            let right = self.parse_comparison()?;
            left = if !left.is_null() && !right.is_null() {
                left
            } else {
                ExprValue::Integer(0)
            };
        }
        Ok(left)
    }

    /// Parse comparison expression.
    /// COMPARISON: ADDITION ( ('<'|'<='|'='|'!='|'>='|'>') ADDITION )*
    fn parse_comparison(&mut self) -> Result<ExprValue, ExprError> {
        let mut left = self.parse_addition()?;
        while matches!(self.peek(), Some("<") | Some("<=") | Some("=") | Some("!=") | Some(">=") | Some(">"))
        {
            let op = self.consume().unwrap().to_string();
            let right = self.parse_addition()?;
            let result = compare_values(&left, &right, &op);
            left = ExprValue::Integer(if result { 1 } else { 0 });
        }
        Ok(left)
    }

    /// Parse addition/subtraction.
    /// ADDITION: MULTIPLICATION ( ('+'|'-') MULTIPLICATION )*
    fn parse_addition(&mut self) -> Result<ExprValue, ExprError> {
        let mut left = self.parse_multiplication()?;
        while matches!(self.peek(), Some("+") | Some("-")) {
            let op = self.consume().unwrap().to_string();
            let right = self.parse_multiplication()?;
            let lv = left
                .as_integer()
                .ok_or(ExprError::NonIntegerArgument)?;
            let rv = right
                .as_integer()
                .ok_or(ExprError::NonIntegerArgument)?;
            left = match op.as_str() {
                "+" => ExprValue::Integer(lv.wrapping_add(rv)),
                "-" => ExprValue::Integer(lv.wrapping_sub(rv)),
                _ => unreachable!(),
            };
        }
        Ok(left)
    }

    /// Parse multiplication/division/modulo.
    /// MULTIPLICATION: MATCH ( ('*'|'/'|'%') MATCH )*
    fn parse_multiplication(&mut self) -> Result<ExprValue, ExprError> {
        let mut left = self.parse_match()?;
        while matches!(self.peek(), Some("*") | Some("/") | Some("%")) {
            let op = self.consume().unwrap().to_string();
            let right = self.parse_match()?;
            let lv = left
                .as_integer()
                .ok_or(ExprError::NonIntegerArgument)?;
            let rv = right
                .as_integer()
                .ok_or(ExprError::NonIntegerArgument)?;
            left = match op.as_str() {
                "*" => ExprValue::Integer(lv.wrapping_mul(rv)),
                "/" => {
                    if rv == 0 {
                        return Err(ExprError::DivisionByZero);
                    }
                    ExprValue::Integer(lv.wrapping_div(rv))
                }
                "%" => {
                    if rv == 0 {
                        return Err(ExprError::DivisionByZero);
                    }
                    ExprValue::Integer(lv.wrapping_rem(rv))
                }
                _ => unreachable!(),
            };
        }
        Ok(left)
    }

    /// Parse match/colon expression.
    /// MATCH: PRIMARY ( ':' PRIMARY )?
    fn parse_match(&mut self) -> Result<ExprValue, ExprError> {
        let left = self.parse_primary()?;
        if self.peek() == Some(":") {
            self.consume();
            let right = self.parse_primary()?;
            let pattern_str = match &right {
                ExprValue::Str(s) => s.clone(),
                ExprValue::Integer(n) => n.to_string(),
            };
            let string = match &left {
                ExprValue::Str(s) => s.clone(),
                ExprValue::Integer(n) => n.to_string(),
            };
            return do_match(&string, &pattern_str);
        }
        Ok(left)
    }

    /// Parse primary expression: keyword functions, parenthesized expressions, or atoms.
    fn parse_primary(&mut self) -> Result<ExprValue, ExprError> {
        match self.peek() {
            None => Err(ExprError::MissingOperand),
            Some("(") => {
                self.consume();
                let val = self.parse_or()?;
                self.expect(")")?;
                Ok(val)
            }
            Some("match") => {
                self.consume();
                let string_val = self.parse_primary()?;
                let pattern_val = self.parse_primary()?;
                let string = match &string_val {
                    ExprValue::Str(s) => s.clone(),
                    ExprValue::Integer(n) => n.to_string(),
                };
                let pattern = match &pattern_val {
                    ExprValue::Str(s) => s.clone(),
                    ExprValue::Integer(n) => n.to_string(),
                };
                do_match(&string, &pattern)
            }
            Some("substr") => {
                self.consume();
                let string_val = self.parse_primary()?;
                let pos_val = self.parse_primary()?;
                let len_val = self.parse_primary()?;
                let string = match &string_val {
                    ExprValue::Str(s) => s.clone(),
                    ExprValue::Integer(n) => n.to_string(),
                };
                let pos = pos_val
                    .as_integer()
                    .ok_or(ExprError::NonIntegerArgument)?;
                let len = len_val
                    .as_integer()
                    .ok_or(ExprError::NonIntegerArgument)?;
                Ok(do_substr(&string, pos, len))
            }
            Some("index") => {
                self.consume();
                let string_val = self.parse_primary()?;
                let chars_val = self.parse_primary()?;
                let string = match &string_val {
                    ExprValue::Str(s) => s.clone(),
                    ExprValue::Integer(n) => n.to_string(),
                };
                let chars = match &chars_val {
                    ExprValue::Str(s) => s.clone(),
                    ExprValue::Integer(n) => n.to_string(),
                };
                Ok(do_index(&string, &chars))
            }
            Some("length") => {
                self.consume();
                let val = self.parse_primary()?;
                let s = match &val {
                    ExprValue::Str(s) => s.clone(),
                    ExprValue::Integer(n) => n.to_string(),
                };
                Ok(ExprValue::Integer(s.len() as i64))
            }
            _ => {
                // Atom: a literal string or number.
                let tok = self.consume().unwrap().to_string();
                if let Some(n) = parse_integer(&tok) {
                    Ok(ExprValue::Integer(n))
                } else {
                    Ok(ExprValue::Str(tok))
                }
            }
        }
    }
}

/// Compare two ExprValues. If both are integers, compare numerically;
/// otherwise compare as strings lexicographically.
fn compare_values(left: &ExprValue, right: &ExprValue, op: &str) -> bool {
    let left_int = left.as_integer();
    let right_int = right.as_integer();

    if let (Some(lv), Some(rv)) = (left_int, right_int) {
        match op {
            "<" => lv < rv,
            "<=" => lv <= rv,
            "=" => lv == rv,
            "!=" => lv != rv,
            ">=" => lv >= rv,
            ">" => lv > rv,
            _ => false,
        }
    } else {
        let ls = left.to_string();
        let rs = right.to_string();
        match op {
            "<" => ls < rs,
            "<=" => ls <= rs,
            "=" => ls == rs,
            "!=" => ls != rs,
            ">=" => ls >= rs,
            ">" => ls > rs,
            _ => false,
        }
    }
}

/// Convert a POSIX BRE (Basic Regular Expression) pattern to a Rust regex pattern.
/// BRE differences from ERE:
/// - `\(` and `\)` are group delimiters (not `(` and `)`)
/// - `\{` and `\}` are interval delimiters
/// - `(` and `)` are literal in BRE
/// - `{` and `}` are literal in BRE
/// - `\+`, `\?` are special in BRE (some implementations)
/// - `+`, `?` are literal in BRE
/// - The match is always anchored at the beginning (as if `^` is prepended).
fn bre_to_rust_regex(pattern: &str) -> String {
    let mut result = String::with_capacity(pattern.len() + 2);
    // BRE patterns in expr are implicitly anchored at the start
    result.push('^');

    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'(' => {
                    result.push('(');
                    i += 2;
                }
                b')' => {
                    result.push(')');
                    i += 2;
                }
                b'{' => {
                    result.push('{');
                    i += 2;
                }
                b'}' => {
                    result.push('}');
                    i += 2;
                }
                b'+' => {
                    result.push('+');
                    i += 2;
                }
                b'?' => {
                    result.push('?');
                    i += 2;
                }
                b'1'..=b'9' => {
                    // Backreference: \1 through \9
                    result.push('\\');
                    result.push(bytes[i + 1] as char);
                    i += 2;
                }
                b'n' => {
                    result.push_str("\\n");
                    i += 2;
                }
                b't' => {
                    result.push_str("\\t");
                    i += 2;
                }
                b'.' | b'*' | b'\\' | b'[' | b']' | b'^' | b'$' | b'|' => {
                    result.push('\\');
                    result.push(bytes[i + 1] as char);
                    i += 2;
                }
                _ => {
                    // Unknown escape: pass through literally
                    result.push('\\');
                    result.push(bytes[i + 1] as char);
                    i += 2;
                }
            }
        } else {
            match bytes[i] {
                b'(' => {
                    // Literal in BRE
                    result.push_str("\\(");
                    i += 1;
                }
                b')' => {
                    // Literal in BRE
                    result.push_str("\\)");
                    i += 1;
                }
                b'{' => {
                    // Literal in BRE
                    result.push_str("\\{");
                    i += 1;
                }
                b'}' => {
                    // Literal in BRE
                    result.push_str("\\}");
                    i += 1;
                }
                b'+' => {
                    // Literal in BRE (not a quantifier)
                    result.push_str("\\+");
                    i += 1;
                }
                b'?' => {
                    // Literal in BRE (not a quantifier)
                    result.push_str("\\?");
                    i += 1;
                }
                b'|' => {
                    // Literal in BRE (not alternation)
                    result.push_str("\\|");
                    i += 1;
                }
                _ => {
                    result.push(bytes[i] as char);
                    i += 1;
                }
            }
        }
    }
    result
}

/// Check whether a BRE pattern contains `\(` ... `\)` groups.
fn bre_has_groups(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            return true;
        }
        i += 1;
    }
    false
}

/// Perform regex match operation.
/// If the pattern has `\(` ... `\)` groups, returns the first captured group (or empty string).
/// Otherwise returns the number of matched characters (or 0).
fn do_match(string: &str, pattern: &str) -> Result<ExprValue, ExprError> {
    let has_groups = bre_has_groups(pattern);
    let rust_pattern = bre_to_rust_regex(pattern);

    let re = Regex::new(&rust_pattern).map_err(|e| {
        ExprError::RegexError(format!("Invalid regular expression: {}", e))
    })?;

    match re.captures(string) {
        Some(caps) => {
            if has_groups {
                // Return the first captured group
                match caps.get(1) {
                    Some(m) => Ok(ExprValue::Str(m.as_str().to_string())),
                    None => Ok(ExprValue::Str(String::new())),
                }
            } else {
                // Return the number of matched characters
                let m = caps.get(0).unwrap();
                Ok(ExprValue::Integer(m.as_str().len() as i64))
            }
        }
        None => {
            if has_groups {
                Ok(ExprValue::Str(String::new()))
            } else {
                Ok(ExprValue::Integer(0))
            }
        }
    }
}

/// Perform the substr operation: extract a substring.
/// Position is 1-based. If pos or len <= 0 or pos > length, returns empty string.
fn do_substr(string: &str, pos: i64, len: i64) -> ExprValue {
    if pos <= 0 || len <= 0 {
        return ExprValue::Str(String::new());
    }
    let start = (pos - 1) as usize;
    let slen = string.len();
    if start >= slen {
        return ExprValue::Str(String::new());
    }
    let end = (start + len as usize).min(slen);
    ExprValue::Str(string[start..end].to_string())
}

/// Perform the index operation: find the position of the first character in CHARS
/// that appears in STRING. Returns 0 if not found. Position is 1-based.
fn do_index(string: &str, chars: &str) -> ExprValue {
    for (i, c) in string.chars().enumerate() {
        if chars.contains(c) {
            return ExprValue::Integer((i + 1) as i64);
        }
    }
    ExprValue::Integer(0)
}

/// Evaluate an expr expression from command-line arguments.
pub fn evaluate_expr(args: &[String]) -> Result<ExprValue, ExprError> {
    if args.is_empty() {
        return Err(ExprError::MissingOperand);
    }
    let mut parser = ExprParser::new(args);
    let result = parser.parse_or()?;
    if parser.pos < parser.args.len() {
        return Err(ExprError::Syntax(format!(
            "unexpected argument '{}'",
            parser.args[parser.pos]
        )));
    }
    Ok(result)
}
