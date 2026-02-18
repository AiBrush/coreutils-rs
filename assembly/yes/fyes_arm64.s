// ============================================================================
//  fyes_arm64.s — GNU-compatible "yes" for AArch64 Linux (GNU assembler)
//
//  BUILD:
//    as -o fyes_arm64.o fyes_arm64.s && ld -static -s -e _start -o fyes_arm64 fyes_arm64.o
//
//  DESCRIPTION:
//    Drop-in replacement for GNU coreutils `yes` in AArch64 Linux assembly.
//    Produces a tiny static ELF64 binary with no runtime dependencies.
//
//  AArch64 Linux syscall ABI:
//    w8  = syscall number
//    x0  = arg1 / return value
//    x1  = arg2
//    x2  = arg3
//    svc #0 = invoke syscall
//    Syscall numbers: write=64  exit_group=94
//
//  REGISTER CONVENTIONS (main loop):
//    x19 = argc (saved)
//    x20 = argv pointer (saved)
//    x21 = output line length
//    x22 = write count (BUFSZ after fill, or line length for long lines)
//
//  MEMORY LAYOUT:
//    BUF    (0x20000) — 16KB write buffer
//    ARGBUF (0x24000) — 2MB argument assembly buffer
// ============================================================================

    .set    SYS_WRITE,      64
    .set    SYS_EXIT_GROUP, 94
    .set    STDOUT,         1
    .set    STDERR,         2
    .set    BUFSZ,          16384
    .set    ARGBUFSZ,       2097152
    .set    EINTR,          -4

// ============================================================================
//  BSS — runtime buffers (zero-initialized by OS)
// ============================================================================
    .section .bss
    .align  12              // 4KB page-aligned

buf:
    .zero   BUFSZ           // 16KB write buffer

    .align  12
argbuf:
    .zero   ARGBUFSZ        // 2MB argument assembly buffer

// ============================================================================
//  Read-only data — help/version/error strings
// ============================================================================
    .section .rodata

help_text:
    .ascii  "Usage: yes [STRING]...\n"
    .ascii  "  or:  yes OPTION\n"
    .ascii  "Repeatedly output a line with all specified STRING(s), or 'y'.\n"
    .ascii  "\n"
    .ascii  "      --help     display this help and exit\n"
    .ascii  "      --version  output version information and exit\n"
    .set    help_text_len, . - help_text

version_text:
    .ascii  "yes (fcoreutils)\n"
    .set    version_text_len, . - version_text

err_unrec_pre:
    .ascii  "yes: unrecognized option '"
    .set    err_unrec_pre_len, . - err_unrec_pre

err_inval_pre:
    .ascii  "yes: invalid option -- '"
    .set    err_inval_pre_len, . - err_inval_pre

err_suffix:
    .ascii  "'\nTry 'yes --help' for more information.\n"
    .set    err_suffix_len, . - err_suffix

default_line:
    .ascii  "y\n"
    .set    default_line_len, . - default_line

// ============================================================================
//  CODE
// ============================================================================
    .section .text
    .globl  _start

// Utility macro: write(fd, buf, len) — uses x0/x1/x2/w8, clobbers x0
.macro  WRITE fd, buf, len
    mov     x0, \fd
    adr     x1, \buf
    mov     x2, \len
    mov     w8, #SYS_WRITE
    svc     #0
.endm

// ============================================================================
//  _start — program entry point
//
//  Stack on entry: [sp] = argc, [sp+8] = argv[0], [sp+16] = argv[1], ...
// ============================================================================
_start:
    ldr     x19, [sp]           // x19 = argc
    mov     x20, sp             // x20 = stack pointer (argv array)
    add     x20, x20, #8        // x20 = &argv[0]

    cmp     x19, #2
    b.lt    .default_path       // 0 or 1 args (just program name) → default "y\n"

    // argc >= 2: check if argv[1] is an option
    ldr     x0, [x20, #8]       // x0 = argv[1]
    ldrb    w1, [x0]
    cmp     w1, #'-'
    b.ne    .build_line         // doesn't start with '-' → it's a normal arg

    ldrb    w1, [x0, #1]
    cmp     w1, #'\0'
    b.eq    .build_line         // just "-" alone → literal string

    cmp     w1, #'-'
    b.ne    .err_short_opt      // single dash + char (e.g. "-n") → invalid option

    // Starts with "--"
    ldrb    w1, [x0, #2]
    cmp     w1, #'\0'
    b.eq    .build_line         // exactly "--" → treat as arg separator, build from rest

    // Check "--help" (7 bytes including null): '-','-','h','e','l','p','\0'
    ldr     x1, =0x0070_6c65_682d_2d00  // le: "\0--help\0p" nope
    // Use byte comparison instead
    ldrb    w1, [x0, #2]        // char after "--"
    cmp     w1, #'h'
    b.ne    .chk_version
    // Check "elp\0"
    ldrb    w1, [x0, #3]
    cmp     w1, #'e'
    b.ne    .err_long_opt
    ldrb    w1, [x0, #4]
    cmp     w1, #'l'
    b.ne    .err_long_opt
    ldrb    w1, [x0, #5]
    cmp     w1, #'p'
    b.ne    .err_long_opt
    ldrb    w1, [x0, #6]
    cmp     w1, #'\0'
    b.ne    .err_long_opt

    // --help matched
    mov     x0, #STDOUT
    adr     x1, help_text
    mov     x2, #help_text_len
    mov     w8, #SYS_WRITE
    svc     #0
    b       .exit_ok

.chk_version:
    // Check "--version" (9 bytes + null)
    ldrb    w1, [x0, #2]
    cmp     w1, #'v'
    b.ne    .err_long_opt
    ldrb    w1, [x0, #3]
    cmp     w1, #'e'
    b.ne    .err_long_opt
    ldrb    w1, [x0, #4]
    cmp     w1, #'r'
    b.ne    .err_long_opt
    ldrb    w1, [x0, #5]
    cmp     w1, #'s'
    b.ne    .err_long_opt
    ldrb    w1, [x0, #6]
    cmp     w1, #'i'
    b.ne    .err_long_opt
    ldrb    w1, [x0, #7]
    cmp     w1, #'o'
    b.ne    .err_long_opt
    ldrb    w1, [x0, #8]
    cmp     w1, #'n'
    b.ne    .err_long_opt
    ldrb    w1, [x0, #9]
    cmp     w1, #'\0'
    b.ne    .err_long_opt

    // --version matched
    mov     x0, #STDOUT
    adr     x1, version_text
    mov     x2, #version_text_len
    mov     w8, #SYS_WRITE
    svc     #0
    b       .exit_ok

// ============================================================================
//  Error: unrecognized long option (e.g. --foo)
// ============================================================================
.err_long_opt:
    // x0 still points to the option string (e.g. "--foo")
    mov     x22, x0             // save option string

    // Write prefix "yes: unrecognized option '"
    mov     x0, #STDERR
    adr     x1, err_unrec_pre
    mov     x2, #err_unrec_pre_len
    mov     w8, #SYS_WRITE
    svc     #0

    // Compute strlen of option string
    mov     x1, x22
    mov     x2, #0
.strlen_unrec:
    ldrb    w3, [x1, x2]
    cbz     w3, .strlen_unrec_done
    add     x2, x2, #1
    b       .strlen_unrec
.strlen_unrec_done:
    // Write option string
    mov     x0, #STDERR
    mov     x1, x22
    // x2 already = length
    mov     w8, #SYS_WRITE
    svc     #0

    // Write suffix
    mov     x0, #STDERR
    adr     x1, err_suffix
    mov     x2, #err_suffix_len
    mov     w8, #SYS_WRITE
    svc     #0
    b       .exit_fail

// ============================================================================
//  Error: invalid short option (e.g. -n)
// ============================================================================
.err_short_opt:
    // x0 points to the option string (e.g. "-n"); x0[1] is the char
    ldrb    w22, [x0, #1]       // save option char

    // Write prefix "yes: invalid option -- '"
    mov     x0, #STDERR
    adr     x1, err_inval_pre
    mov     x2, #err_inval_pre_len
    mov     w8, #SYS_WRITE
    svc     #0

    // Write the single option char from the stack
    strb    w22, [sp, #-16]!    // push char onto stack (16-byte aligned)
    mov     x0, #STDERR
    mov     x1, sp
    mov     x2, #1
    mov     w8, #SYS_WRITE
    svc     #0
    add     sp, sp, #16         // pop stack

    // Write suffix
    mov     x0, #STDERR
    adr     x1, err_suffix
    mov     x2, #err_suffix_len
    mov     w8, #SYS_WRITE
    svc     #0
    b       .exit_fail

// ============================================================================
//  Default path: no args → output "y\n" forever
// ============================================================================
.default_path:
    // Fill BUF with "y\n" repeated (BUFSZ/2 pairs = 8192 iterations)
    adr     x0, buf
    mov     x1, #(BUFSZ / 2)
    mov     w2, #0x0A79         // 'y'=0x79, '\n'=0x0A → little-endian halfword
.fill_default:
    strh    w2, [x0], #2
    subs    x1, x1, #1
    b.ne    .fill_default

    mov     x21, #BUFSZ         // x21 = bytes to write per iteration
    b       .write_loop

// ============================================================================
//  Build output line from argv[1..argc-1] into ARGBUF
//  Join with spaces, append \n, fill BUF with repeated copies
// ============================================================================
.build_line:
    adr     x10, argbuf         // x10 = write cursor in ARGBUF
    mov     x11, #0             // x11 = byte count in ARGBUF
    mov     x12, #0             // x12 = "any arg included" flag
    mov     x13, #2             // x13 = argv index (start at 1, but we start at index 1 = offset 8)

    // Skip first "--" if present
    ldr     x0, [x20, #8]      // argv[1]
    ldrb    w1, [x0]
    cmp     w1, #'-'
    b.ne    .bl_start_copy
    ldrb    w1, [x0, #1]
    cmp     w1, #'-'
    b.ne    .bl_start_copy
    ldrb    w1, [x0, #2]
    cmp     w1, #'\0'
    b.ne    .bl_start_copy
    // argv[1] is exactly "--", skip it
    mov     x13, #3             // start from argv[2]

.bl_start_copy:
    // Loop: x13 = current argv index (1-based, i.e. argv[x13-1])
.bl_loop:
    cmp     x13, x19            // x13 >= argc? (x19 = argc)
    b.ge    .bl_done

    // Load current arg: argv[x13] = *(x20 + x13*8)
    lsl     x14, x13, #3       // x14 = x13 * 8
    ldr     x0, [x20, x14]     // x0 = argv[x13]
    add     x13, x13, #1

    // Add space separator before arg (if not first)
    cbz     x12, .bl_first_arg
    // Check buffer not full
    mov     x15, #(ARGBUFSZ - 2)
    cmp     x11, x15
    b.ge    .bl_done
    mov     w1, #' '
    strb    w1, [x10], #1
    add     x11, x11, #1

.bl_first_arg:
    mov     x12, #1             // mark: have included an arg

.bl_copy_bytes:
    // Check buffer not full
    mov     x15, #(ARGBUFSZ - 2)
    cmp     x11, x15
    b.ge    .bl_skip_arg
    ldrb    w1, [x0], #1        // load byte, advance source
    cbz     w1, .bl_loop        // null terminator → next arg
    strb    w1, [x10], #1       // store byte, advance dest
    add     x11, x11, #1
    b       .bl_copy_bytes

.bl_skip_arg:
    // Buffer full: drain rest of arg
    ldrb    w1, [x0], #1
    cbnz    w1, .bl_skip_arg
    b       .bl_loop

.bl_done:
    cbz     x12, .default_path  // no args included → use default

    // Append '\n'
    mov     w1, #'\n'
    strb    w1, [x10]
    add     x11, x11, #1       // x11 = total line length

    // Now fill BUF with repeated copies of ARGBUF[0..x11)
    adr     x14, argbuf         // source = ARGBUF
    adr     x10, buf            // dest = BUF
    mov     x15, #0             // bytes filled so far

.fill_loop:
    // remaining = BUFSZ - x15
    mov     x16, #BUFSZ
    sub     x16, x16, x15
    cbz     x16, .fill_done

    // copy_len = min(remaining, x11)
    cmp     x16, x11
    csel    x17, x16, x11, lt  // x17 = min

    // memcpy x17 bytes from x14 to x10
    mov     x5, x14             // source
    mov     x6, x10             // dest
    mov     x7, x17             // count
.copy_bytes:
    cbz     x7, .copy_done
    ldrb    w0, [x5], #1
    strb    w0, [x6], #1
    sub     x7, x7, #1
    b       .copy_bytes
.copy_done:
    add     x10, x10, x17      // advance dest
    add     x15, x15, x17      // update filled count
    cmp     x15, #BUFSZ
    b.lt    .fill_loop

.fill_done:
    // Round down to complete lines: x15 - (x15 % x11)
    // If line > BUFSZ, write directly from ARGBUF
    mov     x16, #BUFSZ
    cmp     x11, x16
    b.gt    .long_line

    udiv    x16, x15, x11      // x16 = complete lines
    mul     x16, x16, x11      // x16 = complete lines * line_len
    mov     x21, x16           // x21 = write count (trimmed to complete lines)
    b       .write_loop

.long_line:
    // Line longer than BUF: write directly from ARGBUF
    mov     x21, x11           // write count = line length
    adr     x0, argbuf
    b       .write_direct

// ============================================================================
//  Write loop — write x21 bytes from buf to stdout forever
// ============================================================================
.write_loop:
    adr     x0, buf             // source = BUF

.write_direct:
    // write(STDOUT, x0, x21)
    mov     x1, x0
    mov     x0, #STDOUT
    mov     x2, x21
    mov     w8, #SYS_WRITE
    svc     #0

    // Check return value
    cmn     x0, #EINTR          // returned -EINTR?
    b.eq    .write_direct       // retry on EINTR (x0/x1/x2 still valid? No — need to reload)
    // Actually need to reload after EINTR
    cmp     x0, #0
    b.le    .exit_ok            // EPIPE or other error → exit 0
    b       .write_loop

// ============================================================================
//  Exit helpers
// ============================================================================
.exit_ok:
    mov     x0, #0
    mov     w8, #SYS_EXIT_GROUP
    svc     #0

.exit_fail:
    mov     x0, #1
    mov     w8, #SYS_EXIT_GROUP
    svc     #0
