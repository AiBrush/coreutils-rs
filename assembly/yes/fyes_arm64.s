// ============================================================================
//  fyes_arm64.s — GNU-compatible "yes" in AArch64 Linux assembly
//
//  Drop-in replacement for GNU coreutils `yes` for ARM64 Linux.
//  Produces a small static ELF binary with no runtime dependencies.
//
//  BUILD:
//    as -o fyes_arm64.o fyes_arm64.s
//    ld -static -s -e _start -o fyes_arm64 fyes_arm64.o
//
//  COMPATIBILITY:
//    - --help / --version recognized in argv[1]
//    - "--" in argv[1] stripped (subsequent "--" included in output)
//    - Unrecognized long options (--foo): error to stderr, exit 1
//    - Invalid short options (-x): error to stderr, exit 1
//    - Bare "-" is a literal string, not an option
//    - SIGPIPE/EPIPE: clean exit 0
//    - EINTR on write: automatic retry
//
//  SYSCALLS (only 2):
//    write (64): output to stdout/stderr
//    exit_group (94): terminate process
//
//  REGISTER CONVENTIONS (main execution):
//    x19 = write buffer pointer (saved for write loop)
//    x20 = argc
//    x21 = &argv[0] pointer
//    x22 = write byte count (for write loop)
//    x23 = scratch / option string save
//    x24 = argbuf base pointer
//    x25 = line length (bytes in argbuf including \n)
//    x26 = argv scan pointer (build_line)
//    x27 = "any arg included" flag
//    x28 = bytes filled in buf
// ============================================================================

    .set    SYS_WRITE,      64
    .set    SYS_EXIT_GROUP, 94
    .set    STDOUT,         1
    .set    STDERR,         2
    .set    BUFSZ,          16384
    .set    ARGBUFSZ,       2097152

// ======================== BSS — zero-initialized buffers =====================
    .section .bss
    .balign 4096
buf:
    .zero   BUFSZ               // 16KB write buffer
    .balign 4096
argbuf:
    .zero   ARGBUFSZ            // 2MB argument assembly buffer

// ======================== Read-only data =====================================
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

// ======================== Code ===============================================
    .section .text
    .globl  _start

_start:
    // Stack at entry: [sp]=argc, [sp+8]=argv[0], [sp+16]=argv[1], ...
    ldr     x20, [sp]              // x20 = argc
    add     x21, sp, #8            // x21 = &argv[0]

    // Block SIGPIPE so write() returns -EPIPE instead of killing us.
    // Without this, `yes | head` under bash's pipefail exits with code 141.
    // rt_sigprocmask(SIG_BLOCK=0, &sigset, NULL, 8)
    // SIGPIPE=13; sigset bit = 1<<(13-1) = 1<<12 = 0x1000
    sub     sp, sp, #16
    mov     x0, #0x1000            // sigset: bit 12 = SIGPIPE
    str     x0, [sp]
    mov     x0, #0                 // SIG_BLOCK
    mov     x1, sp                 // &new_set
    mov     x2, #0                 // NULL (old_set)
    mov     x3, #8                 // sigsetsize
    mov     w8, #135               // SYS_RT_SIGPROCMASK
    svc     #0
    add     sp, sp, #16

    cmp     x20, #2
    b.lt    .default_path           // argc < 2: no args, output "y\n"

    // ---- Check argv[1] for options ----
    ldr     x0, [x21, #8]          // x0 = argv[1]
    ldrb    w1, [x0]
    cmp     w1, #'-'
    b.ne    .build_line             // doesn't start with '-': normal arg

    ldrb    w1, [x0, #1]
    cbz     w1, .build_line         // just "-" alone: literal string

    cmp     w1, #'-'
    b.ne    .err_short_opt          // single dash + char (e.g. "-n"): invalid

    // Starts with "--"
    ldrb    w1, [x0, #2]
    cbz     w1, .build_line         // exactly "--": separator, build from rest

    // ---- Check "--help" byte-by-byte ----
    cmp     w1, #'h'
    b.ne    .chk_version
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
    cbz     w1, .do_help
    b       .err_long_opt

.chk_version:
    // ---- Check "--version" byte-by-byte ----
    cmp     w1, #'v'               // w1 still has [x0, #2]
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
    cbz     w1, .do_version
    b       .err_long_opt

// ======================== --help =============================================
.do_help:
    mov     x0, #STDOUT
    adr     x1, help_text
    mov     x2, #help_text_len
    mov     w8, #SYS_WRITE
    svc     #0
    b       .exit_ok

// ======================== --version ==========================================
.do_version:
    mov     x0, #STDOUT
    adr     x1, version_text
    mov     x2, #version_text_len
    mov     w8, #SYS_WRITE
    svc     #0
    b       .exit_ok

// ======================== Error: unrecognized long option (--foo) =============
.err_long_opt:
    ldr     x23, [x21, #8]         // x23 = save argv[1] pointer

    mov     x0, #STDERR
    adr     x1, err_unrec_pre
    mov     x2, #err_unrec_pre_len
    mov     w8, #SYS_WRITE
    svc     #0

    // Compute strlen of option string
    mov     x1, x23
    mov     x2, #0
.strlen_long:
    ldrb    w3, [x1, x2]
    cbz     w3, .strlen_long_done
    add     x2, x2, #1
    b       .strlen_long
.strlen_long_done:
    mov     x0, #STDERR
    mov     x1, x23
    mov     w8, #SYS_WRITE
    svc     #0

    mov     x0, #STDERR
    adr     x1, err_suffix
    mov     x2, #err_suffix_len
    mov     w8, #SYS_WRITE
    svc     #0
    b       .exit_fail

// ======================== Error: invalid short option (-x) ===================
.err_short_opt:
    ldr     x0, [x21, #8]          // argv[1]
    ldrb    w23, [x0, #1]          // save option char

    mov     x0, #STDERR
    adr     x1, err_inval_pre
    mov     x2, #err_inval_pre_len
    mov     w8, #SYS_WRITE
    svc     #0

    // Write the single option char from the stack
    strb    w23, [sp, #-16]!       // push char (16-byte aligned)
    mov     x0, #STDERR
    mov     x1, sp
    mov     x2, #1
    mov     w8, #SYS_WRITE
    svc     #0
    add     sp, sp, #16            // pop

    mov     x0, #STDERR
    adr     x1, err_suffix
    mov     x2, #err_suffix_len
    mov     w8, #SYS_WRITE
    svc     #0
    b       .exit_fail

// ======================== Default "y\n" fast path ============================
.default_path:
    adr     x0, buf
    mov     x1, #(BUFSZ / 2)       // 8192 halfwords
    mov     w2, #0x0A79             // "y\n" as little-endian halfword
.fill_default:
    strh    w2, [x0], #2
    subs    x1, x1, #1
    b.ne    .fill_default

    adr     x19, buf                // x19 = write source
    mov     x22, #BUFSZ             // x22 = bytes per write
    b       .write_loop

// ======================== Build output line from argv =========================
//
// Join argv[1..] with spaces into argbuf, append \n.
// Skip first "--" if argv[1] is exactly "--".
// Fill buf with repeated copies, then enter write loop.
.build_line:
    adr     x24, argbuf             // x24 = argbuf base
    mov     x0, x24                 // x0 = write cursor
    mov     x25, #0                 // x25 = bytes written
    mov     w27, #0                 // w27 = "any arg included" flag

    // x26 = pointer to current argv slot (start at argv[1])
    add     x26, x21, #8           // x26 = &argv[1]

    // Check if argv[1] is exactly "--" and skip it
    ldr     x1, [x26]
    ldrb    w2, [x1]
    cmp     w2, #'-'
    b.ne    .bl_loop
    ldrb    w2, [x1, #1]
    cmp     w2, #'-'
    b.ne    .bl_loop
    ldrb    w2, [x1, #2]
    cbnz    w2, .bl_loop
    // argv[1] is exactly "--", skip it
    add     x26, x26, #8          // advance to argv[2]

.bl_loop:
    ldr     x1, [x26], #8         // x1 = current arg, advance ptr
    cbz     x1, .bl_done          // NULL = end of argv

    // Space separator before arg (unless first)
    cbz     w27, .bl_first

    // Check buffer space
    mov     x9, #(ARGBUFSZ - 2)
    cmp     x25, x9
    b.ge    .bl_done
    mov     w2, #' '
    strb    w2, [x0], #1
    add     x25, x25, #1
    b       .bl_copy

.bl_first:
    mov     w27, #1                 // mark: started including args

.bl_copy:
    mov     x9, #(ARGBUFSZ - 2)
    cmp     x25, x9
    b.ge    .bl_skip_rest
    ldrb    w2, [x1], #1           // load byte from arg
    cbz     w2, .bl_loop           // null -> next arg
    strb    w2, [x0], #1           // store in argbuf
    add     x25, x25, #1
    b       .bl_copy

.bl_skip_rest:
    ldrb    w2, [x1], #1
    cbnz    w2, .bl_skip_rest
    b       .bl_loop

.bl_done:
    cbz     w27, .default_path     // no args included -> default

    // Append newline
    mov     w2, #'\n'
    strb    w2, [x0]
    add     x25, x25, #1          // x25 = total line length

    // ---- Fill buf with repeated copies of the line ----
    adr     x19, buf                // x19 = buf base (write source)
    mov     x0, x19                 // x0 = destination cursor
    mov     x28, #0                 // x28 = bytes filled

.fill_loop:
    mov     x2, #BUFSZ
    sub     x2, x2, x28           // remaining space
    cmp     x2, x25               // room for another complete line?
    b.lt    .fill_done

    // Copy one line from argbuf to buf
    mov     x3, x25               // bytes to copy
    mov     x4, x24               // source = argbuf base
.fill_copy:
    cbz     x3, .fill_next
    ldrb    w5, [x4], #1
    strb    w5, [x0], #1
    sub     x3, x3, #1
    b       .fill_copy

.fill_next:
    add     x28, x28, x25
    b       .fill_loop

.fill_done:
    // If line > BUFSZ (buffer empty), write directly from argbuf
    cbz     x28, .long_line

    // Round down to complete lines
    udiv    x2, x28, x25          // complete lines
    mul     x22, x2, x25          // x22 = complete lines * line_len
    // x19 already = buf base
    b       .write_loop

.long_line:
    mov     x19, x24               // write from argbuf
    mov     x22, x25               // write count = line length

// ======================== Write loop =========================================
//
// Hot loop: write x22 bytes from x19 to stdout forever.
// x19 = buffer pointer (callee-saved, survives syscalls)
// x22 = byte count (callee-saved, survives syscalls)
.write_loop:
    mov     x0, #STDOUT
    mov     x1, x19               // buffer
    mov     x2, x22               // count
    mov     w8, #SYS_WRITE
    svc     #0

    // Check for -EINTR: x0 == -4 means cmn(x0, 4) sets Z
    cmn     x0, #4
    b.eq    .write_loop            // retry on EINTR

    cmp     x0, #0
    b.gt    .write_loop            // positive: bytes written, keep going

    // Zero or negative (EPIPE, error) -> exit 0

// ======================== Exit helpers =======================================
.exit_ok:
    mov     x0, #0
    mov     w8, #SYS_EXIT_GROUP
    svc     #0

.exit_fail:
    mov     x0, #1
    mov     w8, #SYS_EXIT_GROUP
    svc     #0
