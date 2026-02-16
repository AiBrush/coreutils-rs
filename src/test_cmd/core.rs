use std::fs;
use std::os::unix::fs::MetadataExt;

/// Evaluate a test expression given as a slice of string arguments.
///
/// Returns `Ok(true)` if the expression is true, `Ok(false)` if false,
/// and `Err(msg)` on syntax/parse errors.
///
/// This implements a recursive descent parser for the POSIX test expression
/// grammar with GNU extensions.
pub fn evaluate(args: &[String]) -> Result<bool, String> {
    if args.is_empty() {
        return Ok(false);
    }

    // Special cases for 1, 2, 3, and 4 arguments for POSIX compliance.
    // These bypass the parser to handle ambiguous cases correctly.
    match args.len() {
        1 => return Ok(!args[0].is_empty()),
        2 => {
            if args[0] == "!" {
                return Ok(args[1].is_empty());
            }
            return eval_unary(&args[0], &args[1]);
        }
        3 => {
            // Try binary operator first
            if let Ok(result) = eval_binary(&args[0], &args[1], &args[2]) {
                return Ok(result);
            }
            // Try ! unary
            if args[0] == "!" {
                return evaluate(&args[1..]).map(|v| !v);
            }
            // Try ( expr )
            if args[0] == "(" && args[2] == ")" {
                return evaluate(&args[1..2]);
            }
            return Err(format!("test: {}: binary operator expected", args[1]));
        }
        4 => {
            // ! expr expr expr (3-arg expression negated)
            if args[0] == "!" {
                return evaluate(&args[1..]).map(|v| !v);
            }
            // ( expr ) with binary
            // Fall through to general parser
        }
        _ => {}
    }

    let mut parser = Parser::new(args);
    let result = parser.parse_expr()?;
    if parser.pos < parser.args.len() {
        return Err(format!(
            "test: {}: unexpected argument",
            parser.args[parser.pos]
        ));
    }
    Ok(result)
}

/// Evaluate a unary operator expression.
fn eval_unary(op: &str, arg: &str) -> Result<bool, String> {
    match op {
        "-e" => Ok(path_exists(arg)),
        "-f" => Ok(is_regular_file(arg)),
        "-d" => Ok(is_directory(arg)),
        "-r" => Ok(is_readable(arg)),
        "-w" => Ok(is_writable(arg)),
        "-x" => Ok(is_executable(arg)),
        "-s" => Ok(has_size(arg)),
        "-L" | "-h" => Ok(is_symlink(arg)),
        "-b" => Ok(is_block_special(arg)),
        "-c" => Ok(is_char_special(arg)),
        "-p" => Ok(is_fifo(arg)),
        "-S" => Ok(is_socket(arg)),
        "-g" => Ok(is_setgid(arg)),
        "-u" => Ok(is_setuid(arg)),
        "-k" => Ok(is_sticky(arg)),
        "-O" => Ok(is_owned_by_euid(arg)),
        "-G" => Ok(is_group_egid(arg)),
        "-N" => Ok(is_modified_since_read(arg)),
        "-z" => Ok(arg.is_empty()),
        "-n" => Ok(!arg.is_empty()),
        "-t" => {
            let fd: i32 = arg.parse().map_err(|_| format!("test: {}: integer expression expected", arg))?;
            Ok(is_terminal(fd))
        }
        _ => Err(format!("test: {}: unary operator expected", op)),
    }
}

/// Evaluate a binary operator expression.
fn eval_binary(left: &str, op: &str, right: &str) -> Result<bool, String> {
    match op {
        "=" | "==" => Ok(left == right),
        "!=" => Ok(left != right),
        "<" => Ok(left < right),
        ">" => Ok(left > right),
        "-eq" => int_cmp(left, right, |a, b| a == b),
        "-ne" => int_cmp(left, right, |a, b| a != b),
        "-lt" => int_cmp(left, right, |a, b| a < b),
        "-le" => int_cmp(left, right, |a, b| a <= b),
        "-gt" => int_cmp(left, right, |a, b| a > b),
        "-ge" => int_cmp(left, right, |a, b| a >= b),
        "-nt" => Ok(file_newer_than(left, right)),
        "-ot" => Ok(file_older_than(left, right)),
        "-ef" => Ok(same_file(left, right)),
        _ => Err(format!("test: {}: unknown binary operator", op)),
    }
}

fn int_cmp(left: &str, right: &str, cmp: impl Fn(i64, i64) -> bool) -> Result<bool, String> {
    let a: i64 = left
        .parse()
        .map_err(|_| format!("test: {}: integer expression expected", left))?;
    let b: i64 = right
        .parse()
        .map_err(|_| format!("test: {}: integer expression expected", right))?;
    Ok(cmp(a, b))
}

// ---- File test primitives ----

fn path_exists(path: &str) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn is_regular_file(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.is_file())
}

fn is_directory(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.is_dir())
}

fn is_readable(path: &str) -> bool {
    unsafe { libc::access(to_cstr(path).as_ptr(), libc::R_OK) == 0 }
}

fn is_writable(path: &str) -> bool {
    unsafe { libc::access(to_cstr(path).as_ptr(), libc::W_OK) == 0 }
}

fn is_executable(path: &str) -> bool {
    unsafe { libc::access(to_cstr(path).as_ptr(), libc::X_OK) == 0 }
}

fn has_size(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.len() > 0)
}

fn is_symlink(path: &str) -> bool {
    fs::symlink_metadata(path).map_or(false, |m| m.file_type().is_symlink())
}

fn is_block_special(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path).map_or(false, |m| m.file_type().is_block_device())
}

fn is_char_special(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path).map_or(false, |m| m.file_type().is_char_device())
}

fn is_fifo(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path).map_or(false, |m| m.file_type().is_fifo())
}

fn is_socket(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path).map_or(false, |m| m.file_type().is_socket())
}

fn is_setgid(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.mode() & 0o2000 != 0)
}

fn is_setuid(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.mode() & 0o4000 != 0)
}

fn is_sticky(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.mode() & 0o1000 != 0)
}

fn is_owned_by_euid(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.uid() == unsafe { libc::geteuid() })
}

fn is_group_egid(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.gid() == unsafe { libc::getegid() })
}

fn is_modified_since_read(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.mtime() > m.atime())
}

fn is_terminal(fd: i32) -> bool {
    unsafe { libc::isatty(fd) == 1 }
}

fn file_newer_than(a: &str, b: &str) -> bool {
    let ma = fs::metadata(a).and_then(|m| m.modified());
    let mb = fs::metadata(b).and_then(|m| m.modified());
    match (ma, mb) {
        (Ok(ta), Ok(tb)) => ta > tb,
        (Ok(_), Err(_)) => true,
        _ => false,
    }
}

fn file_older_than(a: &str, b: &str) -> bool {
    let ma = fs::metadata(a).and_then(|m| m.modified());
    let mb = fs::metadata(b).and_then(|m| m.modified());
    match (ma, mb) {
        (Ok(ta), Ok(tb)) => ta < tb,
        (Err(_), Ok(_)) => true,
        _ => false,
    }
}

fn same_file(a: &str, b: &str) -> bool {
    let ma = fs::metadata(a);
    let mb = fs::metadata(b);
    match (ma, mb) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

fn to_cstr(s: &str) -> std::ffi::CString {
    std::ffi::CString::new(s).unwrap_or_else(|_| std::ffi::CString::new("").unwrap())
}

// ---- Recursive descent parser ----
//
// Grammar (POSIX + GNU extensions):
//   expr     := or_expr
//   or_expr  := and_expr ( '-o' and_expr )*
//   and_expr := not_expr ( '-a' not_expr )*
//   not_expr := '!' not_expr | primary
//   primary  := '(' expr ')' | unary_op OPERAND | OPERAND binary_op OPERAND | OPERAND

struct Parser<'a> {
    args: &'a [String],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(args: &'a [String]) -> Self {
        Self { args, pos: 0 }
    }

    fn peek(&self) -> Option<&str> {
        self.args.get(self.pos).map(|s| s.as_str())
    }

    fn advance(&mut self) -> Result<&str, String> {
        if self.pos >= self.args.len() {
            return Err("test: missing argument".to_string());
        }
        let val = &self.args[self.pos];
        self.pos += 1;
        Ok(val.as_str())
    }

    fn parse_expr(&mut self) -> Result<bool, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<bool, String> {
        let mut result = self.parse_and()?;
        while self.peek() == Some("-o") {
            self.pos += 1;
            let right = self.parse_and()?;
            result = result || right;
        }
        Ok(result)
    }

    fn parse_and(&mut self) -> Result<bool, String> {
        let mut result = self.parse_not()?;
        while self.peek() == Some("-a") {
            self.pos += 1;
            let right = self.parse_not()?;
            result = result && right;
        }
        Ok(result)
    }

    fn parse_not(&mut self) -> Result<bool, String> {
        if self.peek() == Some("!") {
            self.pos += 1;
            let val = self.parse_not()?;
            return Ok(!val);
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<bool, String> {
        let token = self
            .peek()
            .ok_or_else(|| "test: missing argument".to_string())?;

        // Parenthesized expression
        if token == "(" {
            self.pos += 1;
            let result = self.parse_expr()?;
            if self.peek() != Some(")") {
                return Err("test: missing ')'".to_string());
            }
            self.pos += 1;
            return Ok(result);
        }

        // Check for unary operators
        if is_unary_op(token) {
            let op = self.advance()?.to_string();
            let operand = self.advance()?;
            return eval_unary(&op, operand);
        }

        // Otherwise it's an operand, which might be followed by a binary operator
        let left = self.advance()?.to_string();

        // Check if next token is a binary operator
        if let Some(next) = self.peek() {
            if is_binary_op(next) {
                let op = self.advance()?.to_string();
                let right = self.advance()?;
                return eval_binary(&left, &op, right);
            }
        }

        // Bare string: true if non-empty
        Ok(!left.is_empty())
    }
}

fn is_unary_op(s: &str) -> bool {
    matches!(
        s,
        "-e" | "-f"
            | "-d"
            | "-r"
            | "-w"
            | "-x"
            | "-s"
            | "-L"
            | "-h"
            | "-b"
            | "-c"
            | "-p"
            | "-S"
            | "-g"
            | "-u"
            | "-k"
            | "-O"
            | "-G"
            | "-N"
            | "-z"
            | "-n"
            | "-t"
    )
}

fn is_binary_op(s: &str) -> bool {
    matches!(
        s,
        "=" | "==" | "!=" | "<" | ">" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" | "-nt"
            | "-ot"
            | "-ef"
    )
}
