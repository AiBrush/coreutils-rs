use super::*;

/// Helper to create args from string slices.
fn args(strs: &[&str]) -> Vec<String> {
    strs.iter().map(|s| s.to_string()).collect()
}

/// Helper to evaluate and return the display string.
fn eval(strs: &[&str]) -> Result<String, ExprError> {
    evaluate_expr(&args(strs)).map(|v| v.to_string())
}

#[test]
fn test_expr_arithmetic() {
    // expr 2 + 3 -> 5
    assert_eq!(eval(&["2", "+", "3"]).unwrap(), "5");
    // expr 10 - 3 -> 7
    assert_eq!(eval(&["10", "-", "3"]).unwrap(), "7");
    // expr 100 + 0 -> 100
    assert_eq!(eval(&["100", "+", "0"]).unwrap(), "100");
    // Negative result
    assert_eq!(eval(&["3", "-", "10"]).unwrap(), "-7");
}

#[test]
fn test_expr_multiply() {
    // expr 3 * 4 -> 12
    assert_eq!(eval(&["3", "*", "4"]).unwrap(), "12");
    // expr 7 / 2 -> 3 (integer division)
    assert_eq!(eval(&["7", "/", "2"]).unwrap(), "3");
    // expr 7 % 3 -> 1
    assert_eq!(eval(&["7", "%", "3"]).unwrap(), "1");
}

#[test]
fn test_expr_comparison() {
    // expr 5 > 3 -> 1
    assert_eq!(eval(&["5", ">", "3"]).unwrap(), "1");
    // expr 3 > 5 -> 0
    assert_eq!(eval(&["3", ">", "5"]).unwrap(), "0");
    // expr 5 = 5 -> 1
    assert_eq!(eval(&["5", "=", "5"]).unwrap(), "1");
    // expr 5 != 3 -> 1
    assert_eq!(eval(&["5", "!=", "3"]).unwrap(), "1");
    // expr 3 <= 5 -> 1
    assert_eq!(eval(&["3", "<=", "5"]).unwrap(), "1");
    // expr 5 >= 5 -> 1
    assert_eq!(eval(&["5", ">=", "5"]).unwrap(), "1");
    // expr 3 < 5 -> 1
    assert_eq!(eval(&["3", "<", "5"]).unwrap(), "1");
    // String comparison
    assert_eq!(eval(&["abc", "<", "def"]).unwrap(), "1");
    assert_eq!(eval(&["def", "<", "abc"]).unwrap(), "0");
}

#[test]
fn test_expr_string_match() {
    // expr abc : 'a\(.\)c' -> b
    assert_eq!(eval(&["abc", ":", "a\\(.\\.\\)c"]).unwrap(), "b.");
    // Actually test: a\(.\)c matches abc, capturing 'b'
    assert_eq!(eval(&["abc", ":", "a\\(.\\)c"]).unwrap(), "b");
    // Without groups: returns length of match
    assert_eq!(eval(&["abc", ":", "abc"]).unwrap(), "3");
    // No match: returns 0
    assert_eq!(eval(&["abc", ":", "xyz"]).unwrap(), "0");
    // No match with groups: returns empty string
    assert_eq!(eval(&["abc", ":", "x\\(.\\)z"]).unwrap(), "");
    // match keyword form
    assert_eq!(eval(&["match", "abc", "a\\(.\\)c"]).unwrap(), "b");
}

#[test]
fn test_expr_length() {
    // expr length hello -> 5
    assert_eq!(eval(&["length", "hello"]).unwrap(), "5");
    // expr length '' -> 0
    assert_eq!(eval(&["length", ""]).unwrap(), "0");
    // expr length 'hello world' -> 11
    assert_eq!(eval(&["length", "hello world"]).unwrap(), "11");
}

#[test]
fn test_expr_substr() {
    // expr substr hello 2 3 -> ell
    assert_eq!(eval(&["substr", "hello", "2", "3"]).unwrap(), "ell");
    // expr substr hello 1 1 -> h
    assert_eq!(eval(&["substr", "hello", "1", "1"]).unwrap(), "h");
    // Out of bounds: returns empty string
    assert_eq!(eval(&["substr", "hello", "10", "3"]).unwrap(), "");
    // Zero position: returns empty string
    assert_eq!(eval(&["substr", "hello", "0", "3"]).unwrap(), "");
    // Negative length: returns empty string
    assert_eq!(eval(&["substr", "hello", "1", "-1"]).unwrap(), "");
}

#[test]
fn test_expr_index() {
    // expr index hello l -> 3
    assert_eq!(eval(&["index", "hello", "l"]).unwrap(), "3");
    // expr index hello x -> 0
    assert_eq!(eval(&["index", "hello", "x"]).unwrap(), "0");
    // expr index hello oe -> 2 (first 'e' at position 2; or 'o' at 5 - 'e' comes first)
    assert_eq!(eval(&["index", "hello", "oe"]).unwrap(), "2");
    // expr index hello h -> 1
    assert_eq!(eval(&["index", "hello", "h"]).unwrap(), "1");
}

#[test]
fn test_expr_or() {
    // expr '' | default -> default
    assert_eq!(eval(&["", "|", "default"]).unwrap(), "default");
    // expr hello | default -> hello
    assert_eq!(eval(&["hello", "|", "default"]).unwrap(), "hello");
    // expr 0 | default -> default
    assert_eq!(eval(&["0", "|", "default"]).unwrap(), "default");
    // expr 1 | default -> 1
    assert_eq!(eval(&["1", "|", "default"]).unwrap(), "1");
}

#[test]
fn test_expr_and() {
    // Both non-null: return first
    assert_eq!(eval(&["hello", "&", "world"]).unwrap(), "hello");
    // First is null: return 0
    assert_eq!(eval(&["", "&", "world"]).unwrap(), "0");
    // Second is null: return 0
    assert_eq!(eval(&["hello", "&", ""]).unwrap(), "0");
    // Both null: return 0
    assert_eq!(eval(&["", "&", ""]).unwrap(), "0");
}

#[test]
fn test_expr_parentheses() {
    // expr ( 2 + 3 ) * 4 -> 20
    assert_eq!(eval(&["(", "2", "+", "3", ")", "*", "4"]).unwrap(), "20");
    // Nested parentheses
    assert_eq!(
        eval(&["(", "(", "1", "+", "2", ")", "*", "(", "3", "+", "4", ")", ")"]).unwrap(),
        "21"
    );
}

#[test]
fn test_expr_exit_codes() {
    // Non-null, non-zero result: exit 0
    let result = evaluate_expr(&args(&["1"]));
    assert!(result.is_ok());
    assert!(!result.unwrap().is_null());

    // Null/zero result: exit 1
    let result = evaluate_expr(&args(&["0"]));
    assert!(result.is_ok());
    assert!(result.unwrap().is_null());

    // Empty string is null
    let result = evaluate_expr(&args(&[""]));
    assert!(result.is_ok());
    assert!(result.unwrap().is_null());

    // Syntax error: exit 2
    let result = evaluate_expr(&args(&["+"]));
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().exit_code(), EXIT_EXPR_ERROR);

    // Division by zero: exit 2
    let result = evaluate_expr(&args(&["1", "/", "0"]));
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().exit_code(), EXIT_EXPR_ERROR);

    // Missing operand: exit 2
    let result = evaluate_expr(&args(&[]));
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().exit_code(), EXIT_EXPR_ERROR);
}

#[test]
fn test_expr_matches_gnu() {
    // Test precedence: * binds tighter than +
    // expr 2 + 3 * 4 -> 14
    assert_eq!(eval(&["2", "+", "3", "*", "4"]).unwrap(), "14");

    // expr 10 - 2 - 3 -> 5 (left-associative)
    assert_eq!(eval(&["10", "-", "2", "-", "3"]).unwrap(), "5");

    // expr 10 / 3 -> 3 (truncates toward zero, like GNU)
    assert_eq!(eval(&["10", "/", "3"]).unwrap(), "3");

    // expr -10 / 3 -> -3 (truncates toward zero)
    assert_eq!(eval(&["-10", "/", "3"]).unwrap(), "-3");

    // expr -10 % 3 -> -1 (sign follows dividend, like C)
    assert_eq!(eval(&["-10", "%", "3"]).unwrap(), "-1");

    // String comparison: both integers compare numerically
    assert_eq!(eval(&["9", ">", "10"]).unwrap(), "0");

    // String comparison: non-integer strings compare lexicographically
    assert_eq!(eval(&["abc", ">", "abd"]).unwrap(), "0");
    assert_eq!(eval(&["abd", ">", "abc"]).unwrap(), "1");

    // Chained OR: returns first non-null
    assert_eq!(eval(&["", "|", "", "|", "found"]).unwrap(), "found");

    // Match returns match length when no groups
    assert_eq!(eval(&["abcdef", ":", "abc"]).unwrap(), "3");

    // Match with .* returns full match length
    assert_eq!(eval(&["abcdef", ":", ".*"]).unwrap(), "6");

    // Complex expression: ( 1 + 2 ) * ( 3 + 4 ) = 21
    assert_eq!(
        eval(&["(", "1", "+", "2", ")", "*", "(", "3", "+", "4", ")"]).unwrap(),
        "21"
    );
}

#[test]
fn test_expr_division_by_zero() {
    let result = evaluate_expr(&args(&["1", "/", "0"]));
    assert!(result.is_err());
    match result.unwrap_err() {
        ExprError::DivisionByZero => {}
        other => panic!("Expected DivisionByZero, got: {:?}", other),
    }

    let result = evaluate_expr(&args(&["1", "%", "0"]));
    assert!(result.is_err());
    match result.unwrap_err() {
        ExprError::DivisionByZero => {}
        other => panic!("Expected DivisionByZero, got: {:?}", other),
    }
}

#[test]
fn test_expr_non_integer_arithmetic() {
    // Using non-integer in arithmetic should error
    let result = evaluate_expr(&args(&["abc", "+", "1"]));
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().exit_code(), EXIT_EXPR_ERROR);
}

#[test]
fn test_expr_single_value() {
    // Single integer
    assert_eq!(eval(&["42"]).unwrap(), "42");
    // Single string
    assert_eq!(eval(&["hello"]).unwrap(), "hello");
    // Single negative integer
    assert_eq!(eval(&["-5"]).unwrap(), "-5");
}

#[test]
fn test_expr_extra_args_error() {
    // Unconsumed tokens after valid expression should error
    let result = evaluate_expr(&args(&["1", "2"]));
    assert!(result.is_err());
}

#[test]
fn test_expr_unmatched_paren() {
    // Missing closing paren
    let result = evaluate_expr(&args(&["(", "1", "+", "2"]));
    assert!(result.is_err());

    // Missing opening paren (bare closing paren as primary is just the string ")")
    // This would actually be parsed as the literal string ")" then error on extra args
    let result = evaluate_expr(&args(&[")", "1"]));
    assert!(result.is_err());
}
