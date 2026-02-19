// ============================================================================
//  fyes_arm64.s -- GNU-compatible "yes" in AArch64 Linux assembly
//
//  Drop-in replacement for GNU coreutils `yes` for ARM64 Linux.
//  Produces a small static ELF binary with no runtime dependencies.
//
//  BUILD (manual):
//    as -o fyes_arm64.o fyes_arm64.s
//    ld -static -s -e _start -o fyes_arm64 fyes_arm64.o
//
//  BUILD (recommended -- auto-detects your system's yes output):
//    python3 build.py --target linux-arm64
//
//  COMPATIBILITY:
//    - --help / --version recognized in ALL argv entries (GNU permutation)
//    - "--" terminates option processing; first "--" stripped from output
//    - Unrecognized long options (--foo): error to stderr, exit 1
//    - Invalid short options (-x): error to stderr, exit 1
//    - Bare "-" is a literal string, not an option
//    - SIGPIPE/EPIPE: clean exit 0
//    - EINTR on write: automatic retry
//    - Partial writes: tracked and continued (not restarted from buffer top)
//
//  SYSCALLS (only 3):
//    write (64): output to stdout/stderr
//    exit_group (94): terminate process
//    rt_sigprocmask (135): block SIGPIPE
//
//  REGISTER CONVENTIONS (main execution):
//    x19 = write buffer base pointer
//    x20 = argc
//    x21 = &argv[0] pointer
//    x22 = write byte count per iteration
//    x23 = scratch / saved option string for error messages
//    x24 = argbuf base pointer
//    x25 = line length (bytes in argbuf including \n)
//    x26 = current write pointer (write loop) / argv scan (build_line)
//    x27 = remaining write bytes (write loop) / "any arg" flag (build_line)
//    x28 = bytes filled in buf
// ============================================================================

    .set    SYS_WRITE,           64
    .set    SYS_EXIT_GROUP,      94
    .set    SYS_RT_SIGPROCMASK,  135
    .set    STDOUT,              1
    .set    STDERR,              2
    .set    BUFSZ,               16384
    .set    ARGBUFSZ,            2097152

// ======================== BSS -- zero-initialized buffers ====================
    .section .bss
    .balign 4096
buf:
    .zero   BUFSZ               // 16KB write buffer
    .balign 4096
argbuf:
    .zero   ARGBUFSZ            // 2MB argument assembly buffer

// ======================== Read-only data =====================================
    .section .rodata

// @@DATA_START@@
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
// @@DATA_END@@

// ======================== Code ===============================================
    .section .text
    .globl  _start

_start:
    // Stack at entry: [sp]=argc, [sp+8]=argv[0], [sp+16]=argv[1], ...
    ldr     x20, [sp]              // x20 = argc
    add     x21, sp, #8            // x21 = &argv[0]

    // Block SIGPIPE so write() returns -EPIPE instead of killing us.
    // rt_sigprocmask(SIG_BLOCK=0, &sigset, NULL, 8)
    // SIGPIPE=13; sigset bit = 1<<(13-1) = 1<<12 = 0x1000
    sub     sp, sp, #16
    mov     x0, #0x1000            // sigset: bit 12 = SIGPIPE
    str     x0, [sp]
    mov     x0, #0                 // SIG_BLOCK (Linux = 0)
    mov     x1, sp                 // &new_set
    mov     x2, #0                 // NULL (old_set)
    mov     x3, #8                 // sigsetsize
    mov     w8, #SYS_RT_SIGPROCMASK
    svc     #0
    add     sp, sp, #16

    cmp     x20, #2
    b.lt    .default_path          // argc < 2: no args, output "y\n"

    // ================================================================
    //  PASS 1: Option Validation (GNU permutation)
    //
    //  GNU yes uses parse_gnu_standard_options_only(), which checks
    //  EVERY argv entry for --help/--version, even after non-option
    //  arguments.  "--" terminates option checking.
    //
    //  Register usage in this section:
    //    x9  = "past --" flag (0 = still checking options)
    //    x10 = pointer to current argv entry
    //    x0  = current argument string pointer
    //    w1  = byte comparison temporary
    // ================================================================

    mov     w9, #0                 // x9 = 0: not past "--" yet
    add     x10, x21, #8          // x10 = &argv[1] (skip program name)

.opt_loop:
    ldr     x0, [x10]             // x0 = current argv string pointer
    cbz     x0, .opt_done         // NULL pointer? end of argv

    tbnz    w9, #0, .opt_next     // already past "--"? skip checking

    // --- Check if this arg starts with '-' ---
    ldrb    w1, [x0]
    cmp     w1, #'-'
    b.ne    .opt_next             // doesn't start with '-': not an option

    ldrb    w1, [x0, #1]
    cbz     w1, .opt_next         // just "-" alone: literal string, not option

    // --- Starts with '-'. Is it a long option (--xxx)? ---
    cmp     w1, #'-'
    b.ne    .err_short_opt        // single '-' + char (e.g. "-n"): invalid option

    // Starts with "--". Is it exactly "--" (end-of-options marker)?
    ldrb    w1, [x0, #2]
    cbz     w1, .opt_set_past     // exactly "--": set flag, stop checking

    // ---- Check "--help" byte-by-byte ----
    cmp     w1, #'h'
    b.ne    .p1_chk_version
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

.p1_chk_version:
    // ---- Check "--version" byte-by-byte ----
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
    cbz     w1, .do_version
    b       .err_long_opt

.opt_set_past:
    mov     w9, #1                 // set "past --" flag
.opt_next:
    add     x10, x10, #8          // advance to next argv entry
    b       .opt_loop

.opt_done:
    // All argv entries validated -- no errors found.
    b       .build_line

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
    // x0 = pointer to the offending option string (e.g. "--foo")
    mov     x23, x0                // save option string pointer

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
    // x0 = pointer to the offending option string (e.g. "-n")
    ldrb    w23, [x0, #1]         // save option char (e.g. 'n')

    mov     x0, #STDERR
    adr     x1, err_inval_pre
    mov     x2, #err_inval_pre_len
    mov     w8, #SYS_WRITE
    svc     #0

    // Write the single option char from the stack
    strb    w23, [sp, #-16]!      // push char (16-byte aligned)
    mov     x0, #STDERR
    mov     x1, sp
    mov     x2, #1
    mov     w8, #SYS_WRITE
    svc     #0
    add     sp, sp, #16           // pop

    mov     x0, #STDERR
    adr     x1, err_suffix
    mov     x2, #err_suffix_len
    mov     w8, #SYS_WRITE
    svc     #0
    b       .exit_fail

// ======================== Default "y\n" fast path ============================
.default_path:
    adr     x0, buf
    mov     x1, #(BUFSZ / 2)      // 8192 halfwords
    mov     w2, #0x0A79            // "y\n" as little-endian halfword
.fill_default:
    strh    w2, [x0], #2
    subs    x1, x1, #1
    b.ne    .fill_default

    adr     x19, buf               // x19 = write source
    mov     x22, #BUFSZ            // x22 = bytes per write
    b       .write_outer

// ======================== Build output line from argv =========================
//
// Join argv[1..] with spaces into argbuf, append \n.
// Skip first "--" encountered anywhere in argv (x12 flag).
// Fill buf with repeated copies, then enter write loop.
.build_line:
    adr     x24, argbuf            // x24 = argbuf base
    mov     x0, x24                // x0 = write cursor
    mov     x25, #0                // x25 = bytes written
    mov     w27, #0                // w27 = "any arg included" flag
    mov     w12, #0                // w12 = "--" skip flag

    // x26 = pointer to current argv slot (start at argv[1])
    add     x26, x21, #8          // x26 = &argv[1]

.bl_loop:
    ldr     x1, [x26], #8         // x1 = current arg, advance ptr
    cbz     x1, .bl_done          // NULL = end of argv

    // Check if this is exactly "--" and we should skip it (first one only)
    cbnz    w12, .bl_include       // already skipped a "--"? include this
    ldrb    w2, [x1]
    cmp     w2, #'-'
    b.ne    .bl_include
    ldrb    w2, [x1, #1]
    cmp     w2, #'-'
    b.ne    .bl_include
    ldrb    w2, [x1, #2]
    cbnz    w2, .bl_include        // not exactly "--": include it
    // First "--": skip it
    mov     w12, #1
    b       .bl_loop

.bl_include:
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
    mov     w27, #1                // mark: started including args

.bl_copy:
    mov     x9, #(ARGBUFSZ - 2)
    cmp     x25, x9
    b.ge    .bl_skip_rest
    ldrb    w2, [x1], #1          // load byte from arg
    cbz     w2, .bl_loop          // null -> next arg
    strb    w2, [x0], #1          // store in argbuf
    add     x25, x25, #1
    b       .bl_copy

.bl_skip_rest:
    ldrb    w2, [x1], #1
    cbnz    w2, .bl_skip_rest
    b       .bl_loop

.bl_done:
    cbz     w27, .default_path    // no args included -> default

    // Append newline
    mov     w2, #'\n'
    strb    w2, [x0]
    add     x25, x25, #1         // x25 = total line length

    // ---- Fill buf with repeated copies of the line ----
    adr     x19, buf               // x19 = buf base (write source)
    mov     x0, x19                // x0 = destination cursor
    mov     x28, #0                // x28 = bytes filled

.fill_loop:
    mov     x2, #BUFSZ
    sub     x2, x2, x28          // remaining space
    cmp     x2, x25              // room for another complete line?
    b.lt    .fill_done

    // Copy one line from argbuf to buf
    mov     x3, x25              // bytes to copy
    mov     x4, x24              // source = argbuf base
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
    udiv    x2, x28, x25         // complete lines
    mul     x22, x2, x25         // x22 = complete lines * line_len
    // x19 already = buf base
    b       .write_outer

.long_line:
    mov     x19, x24              // write from argbuf
    mov     x22, x25              // write count = line length

// ======================== Write loop =========================================
//
// Hot loop: write x22 bytes from x19 to stdout forever.
// Handles partial writes by tracking position with x26/x27.
//
// x19 = buffer base pointer (constant across iterations)
// x22 = total byte count per buffer cycle (constant)
// x26 = current write pointer (advances on partial writes)
// x27 = remaining bytes to write (decreases on partial writes)
.write_outer:
    mov     x26, x19              // current ptr = buffer start
    mov     x27, x22              // remaining = full count

.write_loop:
    mov     x0, #STDOUT
    mov     x1, x26               // buffer position
    mov     x2, x27               // remaining count
    mov     w8, #SYS_WRITE
    svc     #0

    // Check for EINTR: x0 == -4 means cmn(x0, 4) sets Z
    cmn     x0, #4
    b.eq    .write_loop            // retry on EINTR (same position)

    // Check for error (zero or negative = EPIPE, etc.)
    cmp     x0, #0
    b.le    .exit_ok               // exit cleanly

    // Success: x0 = bytes written (may be partial)
    add     x26, x26, x0          // advance write pointer
    subs    x27, x27, x0          // decrease remaining
    b.gt    .write_loop            // partial write: continue
    b       .write_outer           // full buffer written: restart

// ======================== Exit helpers =======================================
.exit_ok:
    mov     x0, #0
    mov     w8, #SYS_EXIT_GROUP
    svc     #0

.exit_fail:
    mov     x0, #1
    mov     w8, #SYS_EXIT_GROUP
    svc     #0
