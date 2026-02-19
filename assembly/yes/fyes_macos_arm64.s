// ============================================================================
//  fyes_macos_arm64.s -- GNU-compatible "yes" for macOS ARM64
//
//  Mach-O AArch64 executable for Apple Silicon Macs.
//  Linked via system linker with -lSystem.
//
//  BUILD (manual):
//    as -arch arm64 -o fyes_macos_arm64.o fyes_macos_arm64.s
//    ld -arch arm64 -o fyes fyes_macos_arm64.o -lSystem \
//       -syslibroot $(xcrun --show-sdk-path) -e _start
//
//  BUILD (recommended):
//    python3 build.py --target macos-arm64
//
//  COMPATIBILITY:
//    - --help / --version recognized in ALL argv entries (GNU permutation)
//    - "--" terminates option processing; first "--" stripped from output
//    - Unrecognized long options (--foo): error to stderr, exit 1
//    - Invalid short options (-x): error to stderr, exit 1
//    - Bare "-" is a literal string, not an option
//    - SIGPIPE/EPIPE: clean exit 0
//    - EINTR on write: automatic retry
//    - Partial writes: tracked and continued
//
//  macOS ARM64 SYSCALL ABI:
//    - svc #0x80 (NOT svc #0 like Linux!)
//    - Syscall number in x16 (NOT x8 like Linux!)
//    - BSD syscall numbers: write=4, exit=1, sigprocmask=48
//    - Error: CARRY FLAG set, x0 = positive errno
//    - Success: carry clear, x0 = return value
//    - SIG_BLOCK = 1 (Linux = 0)
//
//  REGISTER CONVENTIONS:
//    x19 = write buffer base pointer
//    x20 = argc
//    x21 = &argv[0] pointer
//    x22 = write byte count per iteration
//    x23 = scratch / saved option string for error messages
//    x24 = argbuf base pointer
//    x25 = line length
//    x26 = current write pointer (write loop) / argv scan (build_line)
//    x27 = remaining write bytes (write loop) / "any arg" flag (build_line)
//    x28 = bytes filled in buf
// ============================================================================

    .set    SYS_EXIT,           1
    .set    SYS_WRITE,          4
    .set    SYS_SIGPROCMASK,    48
    .set    STDOUT,             1
    .set    STDERR,             2
    .set    BUFSZ,              16384
    .set    ARGBUFSZ,           2097152

// ======================== BSS ================================================
    .section __DATA,__bss
    .p2align 12
buf:
    .zero   BUFSZ               // 16KB write buffer
    .p2align 12
argbuf:
    .zero   ARGBUFSZ            // 2MB argument assembly buffer

// ======================== Read-only data =====================================
    .section __TEXT,__const

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
    .section __TEXT,__text,regular,pure_instructions
    .globl  _start
    .p2align 2

_start:
    // Stack at entry: [sp]=argc, [sp+8]=argv[0], [sp+16]=argv[1], ...
    ldr     x20, [sp]              // x20 = argc
    add     x21, sp, #8            // x21 = &argv[0]

    // Block SIGPIPE so write() returns EPIPE instead of killing us.
    // macOS sigprocmask(SIG_BLOCK=1, &sigset, NULL)
    // SIGPIPE=13; sigset bit = 1<<(13-1) = 1<<12 = 0x1000
    sub     sp, sp, #16
    mov     w0, #0x1000            // SIGPIPE bit in uint32_t sigset_t
    str     w0, [sp]
    mov     x0, #1                 // SIG_BLOCK (macOS = 1)
    mov     x1, sp                 // &new_set
    mov     x2, #0                 // NULL (old_set)
    mov     x16, #SYS_SIGPROCMASK
    svc     #0x80
    add     sp, sp, #16

    cmp     x20, #2
    b.lt    .default_path          // argc < 2: no args, output "y\n"

    // ================================================================
    //  PASS 1: Option Validation (GNU permutation)
    //  Scan ALL argv for --help/--version. "--" terminates checking.
    //
    //  x9  = "past --" flag
    //  x10 = pointer to current argv entry
    //  x0  = current argument string pointer
    //  w1  = byte comparison temp
    // ================================================================

    mov     w9, #0
    add     x10, x21, #8          // x10 = &argv[1]

.opt_loop:
    ldr     x0, [x10]
    cbz     x0, .opt_done

    tbnz    w9, #0, .opt_next

    ldrb    w1, [x0]
    cmp     w1, #'-'
    b.ne    .opt_next

    ldrb    w1, [x0, #1]
    cbz     w1, .opt_next          // "-" alone: literal

    cmp     w1, #'-'
    b.ne    .err_short_opt         // -x: invalid

    ldrb    w1, [x0, #2]
    cbz     w1, .opt_set_past      // "--": set past flag

    // Check "--help"
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
    mov     w9, #1
.opt_next:
    add     x10, x10, #8
    b       .opt_loop

.opt_done:
    b       .build_line

// ======================== --help =============================================
.do_help:
    mov     x0, #STDOUT
    adrp    x1, help_text@PAGE
    add     x1, x1, help_text@PAGEOFF
    mov     x2, #help_text_len
    mov     x16, #SYS_WRITE
    svc     #0x80
    b       .exit_ok

// ======================== --version ==========================================
.do_version:
    mov     x0, #STDOUT
    adrp    x1, version_text@PAGE
    add     x1, x1, version_text@PAGEOFF
    mov     x2, #version_text_len
    mov     x16, #SYS_WRITE
    svc     #0x80
    b       .exit_ok

// ======================== Error: unrecognized long option (--foo) =============
.err_long_opt:
    mov     x23, x0

    mov     x0, #STDERR
    adrp    x1, err_unrec_pre@PAGE
    add     x1, x1, err_unrec_pre@PAGEOFF
    mov     x2, #err_unrec_pre_len
    mov     x16, #SYS_WRITE
    svc     #0x80

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
    mov     x16, #SYS_WRITE
    svc     #0x80

    mov     x0, #STDERR
    adrp    x1, err_suffix@PAGE
    add     x1, x1, err_suffix@PAGEOFF
    mov     x2, #err_suffix_len
    mov     x16, #SYS_WRITE
    svc     #0x80
    b       .exit_fail

// ======================== Error: invalid short option (-x) ===================
.err_short_opt:
    ldrb    w23, [x0, #1]

    mov     x0, #STDERR
    adrp    x1, err_inval_pre@PAGE
    add     x1, x1, err_inval_pre@PAGEOFF
    mov     x2, #err_inval_pre_len
    mov     x16, #SYS_WRITE
    svc     #0x80

    strb    w23, [sp, #-16]!
    mov     x0, #STDERR
    mov     x1, sp
    mov     x2, #1
    mov     x16, #SYS_WRITE
    svc     #0x80
    add     sp, sp, #16

    mov     x0, #STDERR
    adrp    x1, err_suffix@PAGE
    add     x1, x1, err_suffix@PAGEOFF
    mov     x2, #err_suffix_len
    mov     x16, #SYS_WRITE
    svc     #0x80
    b       .exit_fail

// ======================== Default "y\n" fast path ============================
.default_path:
    adrp    x0, buf@PAGE
    add     x0, x0, buf@PAGEOFF
    mov     x1, #(BUFSZ / 2)
    mov     w2, #0x0A79
.fill_default:
    strh    w2, [x0], #2
    subs    x1, x1, #1
    b.ne    .fill_default

    adrp    x19, buf@PAGE
    add     x19, x19, buf@PAGEOFF
    mov     x22, #BUFSZ
    b       .write_outer

// ======================== Build output line from argv =========================
.build_line:
    adrp    x24, argbuf@PAGE
    add     x24, x24, argbuf@PAGEOFF
    mov     x0, x24
    mov     x25, #0
    mov     w27, #0
    mov     w12, #0

    add     x26, x21, #8

.bl_loop:
    ldr     x1, [x26], #8
    cbz     x1, .bl_done

    cbnz    w12, .bl_include
    ldrb    w2, [x1]
    cmp     w2, #'-'
    b.ne    .bl_include
    ldrb    w2, [x1, #1]
    cmp     w2, #'-'
    b.ne    .bl_include
    ldrb    w2, [x1, #2]
    cbnz    w2, .bl_include
    mov     w12, #1
    b       .bl_loop

.bl_include:
    cbz     w27, .bl_first

    mov     x9, #(ARGBUFSZ - 2)
    cmp     x25, x9
    b.ge    .bl_done
    mov     w2, #' '
    strb    w2, [x0], #1
    add     x25, x25, #1
    b       .bl_copy

.bl_first:
    mov     w27, #1

.bl_copy:
    mov     x9, #(ARGBUFSZ - 2)
    cmp     x25, x9
    b.ge    .bl_skip_rest
    ldrb    w2, [x1], #1
    cbz     w2, .bl_loop
    strb    w2, [x0], #1
    add     x25, x25, #1
    b       .bl_copy

.bl_skip_rest:
    ldrb    w2, [x1], #1
    cbnz    w2, .bl_skip_rest
    b       .bl_loop

.bl_done:
    cbz     w27, .default_path

    mov     w2, #'\n'
    strb    w2, [x0]
    add     x25, x25, #1

    // Fill buf with repeated copies
    adrp    x19, buf@PAGE
    add     x19, x19, buf@PAGEOFF
    mov     x0, x19
    mov     x28, #0

.fill_loop:
    mov     x2, #BUFSZ
    sub     x2, x2, x28
    cmp     x2, x25
    b.lt    .fill_done

    mov     x3, x25
    mov     x4, x24
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
    cbz     x28, .long_line

    udiv    x2, x28, x25
    mul     x22, x2, x25
    b       .write_outer

.long_line:
    mov     x19, x24
    mov     x22, x25

// ======================== Write loop =========================================
//
// macOS error detection: CARRY FLAG (b.cs), not negative x0.
// x19 = buffer base, x22 = total count
// x26 = current write ptr, x27 = remaining bytes
.write_outer:
    mov     x26, x19
    mov     x27, x22

.write_loop:
    mov     x0, #STDOUT
    mov     x1, x26
    mov     x2, x27
    mov     x16, #SYS_WRITE
    svc     #0x80
    b.cs    .write_error           // carry set = macOS error

    // x0 = bytes written (>= 1 after carry-clear on macOS)
    cbz     x0, .exit_ok           // defensive guard against zero return
    add     x26, x26, x0
    subs    x27, x27, x0
    b.gt    .write_loop
    b       .write_outer

.write_error:
    cmp     x0, #4                 // EINTR?
    b.eq    .write_loop            // retry at same position
    // EPIPE or other: exit 0

// ======================== Exit ===============================================
.exit_ok:
    mov     x0, #0
    mov     x16, #SYS_EXIT
    svc     #0x80

.exit_fail:
    mov     x0, #1
    mov     x16, #SYS_EXIT
    svc     #0x80
