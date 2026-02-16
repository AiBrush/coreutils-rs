use super::core::process_format_string;

#[test]
fn test_printf_string() {
    let result = process_format_string("%s\\n", &["hello"]);
    assert_eq!(result, b"hello\n");
}

#[test]
fn test_printf_integer() {
    let result = process_format_string("%d", &["42"]);
    assert_eq!(result, b"42");
}

#[test]
fn test_printf_float() {
    let result = process_format_string("%.2f", &["3.14159"]);
    assert_eq!(result, b"3.14");
}

#[test]
fn test_printf_hex() {
    let result = process_format_string("%x", &["255"]);
    assert_eq!(result, b"ff");
}

#[test]
fn test_printf_octal() {
    let result = process_format_string("%o", &["8"]);
    assert_eq!(result, b"10");
}

#[test]
fn test_printf_width_precision() {
    // Right-aligned string with width
    let result = process_format_string("%10s", &["hello"]);
    assert_eq!(result, b"     hello");

    // Left-aligned string with width
    let result = process_format_string("%-10s", &["hello"]);
    assert_eq!(result, b"hello     ");

    // Zero-padded integer
    let result = process_format_string("%05d", &["42"]);
    assert_eq!(result, b"00042");

    // Width and precision on float
    let result = process_format_string("%10.2f", &["3.14159"]);
    assert_eq!(result, b"      3.14");

    // String precision truncates
    let result = process_format_string("%.3s", &["hello"]);
    assert_eq!(result, b"hel");

    // Width + precision on string
    let result = process_format_string("%10.3s", &["hello"]);
    assert_eq!(result, b"       hel");
}

#[test]
fn test_printf_escape_sequences() {
    // Tab
    let result = process_format_string("\\t", &[]);
    assert_eq!(result, b"\t");

    // Newline
    let result = process_format_string("\\n", &[]);
    assert_eq!(result, b"\n");

    // Backslash
    let result = process_format_string("\\\\", &[]);
    assert_eq!(result, b"\\");

    // Bell
    let result = process_format_string("\\a", &[]);
    assert_eq!(result, &[0x07]);

    // Backspace
    let result = process_format_string("\\b", &[]);
    assert_eq!(result, &[0x08]);

    // Escape
    let result = process_format_string("\\e", &[]);
    assert_eq!(result, &[0x1B]);

    // Form feed
    let result = process_format_string("\\f", &[]);
    assert_eq!(result, &[0x0C]);

    // Carriage return
    let result = process_format_string("\\r", &[]);
    assert_eq!(result, b"\r");

    // Vertical tab
    let result = process_format_string("\\v", &[]);
    assert_eq!(result, &[0x0B]);

    // Octal
    let result = process_format_string("\\101", &[]);
    assert_eq!(result, b"A");

    // Hex
    let result = process_format_string("\\x41", &[]);
    assert_eq!(result, b"A");

    // \c stops output
    let result = process_format_string("hello\\cworld", &[]);
    assert_eq!(result, b"hello");

    // Percent literal
    let result = process_format_string("%%", &[]);
    assert_eq!(result, b"%");

    // Double quote
    let result = process_format_string("\\\"", &[]);
    assert_eq!(result, b"\"");
}

#[test]
fn test_printf_repeat_format() {
    let result = process_format_string("%s ", &["a", "b", "c"]);
    assert_eq!(result, b"a b c ");
}

#[test]
fn test_printf_b_specifier() {
    // Basic backslash escapes in %b argument
    let result = process_format_string("%b", &["hello\\nworld"]);
    assert_eq!(result, b"hello\nworld");

    // Tab in %b
    let result = process_format_string("%b", &["col1\\tcol2"]);
    assert_eq!(result, b"col1\tcol2");

    // \c in %b stops all output
    let result = process_format_string("%b%b", &["hello\\c", "world"]);
    assert_eq!(result, b"hello");

    // Octal in %b
    let result = process_format_string("%b", &["\\101"]);
    assert_eq!(result, b"A");
}

#[test]
fn test_printf_unicode() {
    // \u escape for Unicode
    let result = process_format_string("\\u0041", &[]);
    assert_eq!(result, b"A");

    // Multi-byte Unicode
    let result = process_format_string("\\u00e9", &[]);
    assert_eq!(result, "\u{00e9}".as_bytes());

    // \U escape for 8-digit Unicode
    let result = process_format_string("\\U00000041", &[]);
    assert_eq!(result, b"A");
}

#[test]
fn test_printf_matches_gnu() {
    // Test various format behaviors that should match GNU printf

    // %d with no args produces "0"
    let result = process_format_string("%d", &[]);
    assert_eq!(result, b"0");

    // %s with no args produces ""
    let result = process_format_string("%s", &[]);
    assert_eq!(result, b"");

    // Hex uppercase
    let result = process_format_string("%X", &["255"]);
    assert_eq!(result, b"FF");

    // Multiple conversions in one format
    let result = process_format_string("%s is %d", &["age", "42"]);
    assert_eq!(result, b"age is 42");

    // Repeating format consumes all arguments
    let result = process_format_string("[%d]", &["1", "2", "3"]);
    assert_eq!(result, b"[1][2][3]");

    // Character specifier
    let result = process_format_string("%c", &["A"]);
    assert_eq!(result, b"A");

    // Octal output
    let result = process_format_string("%o", &["255"]);
    assert_eq!(result, b"377");

    // Unsigned
    let result = process_format_string("%u", &["42"]);
    assert_eq!(result, b"42");

    // Hex with 0x prefix
    let result = process_format_string("%d", &["0xff"]);
    assert_eq!(result, b"255");

    // Octal input
    let result = process_format_string("%d", &["010"]);
    assert_eq!(result, b"8");

    // Character constant input
    let result = process_format_string("%d", &["'A"]);
    assert_eq!(result, b"65");

    // Negative integer
    let result = process_format_string("%d", &["-5"]);
    assert_eq!(result, b"-5");

    // Plus flag
    let result = process_format_string("%+d", &["5"]);
    assert_eq!(result, b"+5");

    // Space flag
    let result = process_format_string("% d", &["5"]);
    assert_eq!(result, b" 5");

    // Float default precision
    let result = process_format_string("%f", &["3.14"]);
    assert_eq!(result, b"3.140000");

    // Scientific notation
    let result = process_format_string("%e", &["100000"]);
    assert_eq!(result, b"1.000000e+05");

    // %g format
    let result = process_format_string("%g", &["100000"]);
    assert_eq!(result, b"100000");

    let result = process_format_string("%g", &["1000000"]);
    assert_eq!(result, b"1e+06");
}

#[test]
fn test_printf_integer_formats() {
    // %i is same as %d
    let result = process_format_string("%i", &["42"]);
    assert_eq!(result, b"42");

    // Zero-padded with width
    let result = process_format_string("%08x", &["255"]);
    assert_eq!(result, b"000000ff");

    // Left-aligned integer
    let result = process_format_string("%-5d|", &["42"]);
    assert_eq!(result, b"42   |");
}

#[test]
fn test_printf_empty_format() {
    let result = process_format_string("", &["ignored"]);
    assert_eq!(result, b"");
}

#[test]
fn test_printf_literal_text() {
    let result = process_format_string("hello world", &[]);
    assert_eq!(result, b"hello world");
}

#[test]
fn test_printf_mixed_format_and_text() {
    let result = process_format_string("Name: %s, Age: %d\\n", &["Alice", "30"]);
    assert_eq!(result, b"Name: Alice, Age: 30\n");
}

#[test]
fn test_printf_octal_escape_with_leading_zero() {
    // \0NNN format
    let result = process_format_string("\\0101", &[]);
    assert_eq!(result, b"A");
}

#[test]
fn test_printf_repeat_with_multiple_specifiers() {
    // Format has 2 specifiers, 4 arguments -> 2 passes
    let result = process_format_string("%s=%d\\n", &["x", "1", "y", "2"]);
    assert_eq!(result, b"x=1\ny=2\n");
}

#[test]
fn test_printf_extra_args_default() {
    // Extra arguments beyond what format consumes get "" or 0
    // With %d and three args: repeats format
    let result = process_format_string("%d\\n", &["1", "2", "3"]);
    assert_eq!(result, b"1\n2\n3\n");
}
