; ============================================================================
;  fyes_macos_x86_64.asm -- GNU-compatible "yes" for macOS x86_64
;
;  Mach-O 64-bit executable, linked via system linker.
;  Produces a small binary with minimal dependencies (only libSystem).
;
;  BUILD (manual):
;    nasm -f macho64 fyes_macos_x86_64.asm -o fyes_macos_x86_64.o
;    ld -arch x86_64 -o fyes fyes_macos_x86_64.o -lSystem \
;       -syslibroot $(xcrun --show-sdk-path) -e _start
;
;  BUILD (recommended):
;    python3 build.py --target macos-x86_64
;
;  COMPATIBILITY:
;    - --help / --version recognized anywhere in argv (GNU permutation)
;    - "--" terminates option processing; first "--" stripped from output
;    - Unrecognized long options (--foo): error to stderr, exit 1
;    - Invalid short options (-x): error to stderr, exit 1
;    - Bare "-" is a literal string, not an option
;    - SIGPIPE/EPIPE: clean exit 0
;    - EINTR on write: automatic retry
;    - Partial writes: tracked and continued
;
;  macOS SYSCALL ABI:
;    - Syscall numbers = 0x2000000 + BSD_number
;    - Error indicated by CARRY FLAG (not negative rax like Linux)
;    - On error: CF=1, rax = positive errno value
;    - On success: CF=0, rax = return value
;    - SIG_BLOCK = 1 (Linux has SIG_BLOCK = 0)
; ============================================================================

BITS 64
default rel

%define SYS_WRITE       0x2000004
%define SYS_EXIT        0x2000001
%define SYS_SIGPROCMASK 0x2000030

%define STDOUT          1
%define STDERR          2
%define BUFSZ           16384
%define ARGBUFSZ        2097152
%define ARGBUF_MAX      (ARGBUFSZ - 2)

; macOS constants (differ from Linux!)
%define SIG_BLOCK       1           ; Linux = 0, macOS = 1
%define SIGPIPE_BIT     0x1000      ; 1 << (13-1)

; ======================== BSS =================================================
section .bss
align 4096
buf:    resb BUFSZ                  ; 16KB write buffer
align 4096
argbuf: resb ARGBUFSZ              ; 2MB argument assembly buffer

; ======================== Code ================================================
section .text
global _start

_start:
    pop     rcx                     ; rcx = argc
    mov     r14, rsp                ; r14 = &argv[0]

    ; Block SIGPIPE so write() returns EPIPE instead of killing us.
    ; macOS sigprocmask(SIG_BLOCK=1, &sigset, NULL)
    sub     rsp, 16
    mov     dword [rsp], SIGPIPE_BIT ; sigset: bit 12 = SIGPIPE
    mov     dword [rsp+4], 0
    mov     eax, SYS_SIGPROCMASK
    mov     edi, SIG_BLOCK          ; 1 on macOS
    lea     rsi, [rsp]
    xor     edx, edx                ; NULL (old_set)
    syscall
    add     rsp, 16

    cmp     ecx, 2
    jl      .default                ; argc < 2: use default "y\n"

    ; ================================================================
    ;  PASS 1: Option Validation (GNU permutation)
    ;  Scan ALL argv for --help/--version. "--" terminates checking.
    ; ================================================================

    xor     r15d, r15d             ; r15 = 0: not past "--" yet
    lea     rbx, [r14 + 8]        ; rbx = &argv[1]

.opt_loop:
    mov     rsi, [rbx]
    test    rsi, rsi
    jz      .opt_done

    test    r15d, r15d
    jnz     .opt_next

    cmp     byte [rsi], '-'
    jne     .opt_next
    cmp     byte [rsi+1], 0
    je      .opt_next               ; just "-": literal
    cmp     byte [rsi+1], '-'
    jne     .err_short_opt          ; -x: invalid
    cmp     byte [rsi+2], 0
    je      .opt_set_past           ; exactly "--"

    ; --- Check "--help" ---
    cmp     dword [rsi], 0x65682D2D ; "--he"
    jne     .chk_ver
    cmp     word [rsi+4], 0x706C    ; "lp"
    jne     .chk_ver
    cmp     byte [rsi+6], 0
    jne     .chk_ver
    lea     rsi, [help_text]
    mov     edx, help_text_len
    jmp     .print_exit_ok

.chk_ver:
    ; --- Check "--version" ---
    cmp     dword [rsi], 0x65762D2D ; "--ve"
    jne     .err_long_opt
    cmp     dword [rsi+4], 0x6F697372 ; "rsio"
    jne     .err_long_opt
    cmp     word [rsi+8], 0x006E    ; "n\0"
    jne     .err_long_opt
    lea     rsi, [version_text]
    mov     edx, version_text_len
    jmp     .print_exit_ok

    ; ============================================================
    ;  Error: Unrecognized long option (e.g. "--foo")
    ; ============================================================
.err_long_opt:
    mov     r12, rsi                ; save option string pointer

    mov     eax, SYS_WRITE
    mov     edi, STDERR
    lea     rsi, [err_unrec]
    mov     edx, err_unrec_len
    syscall

    mov     rsi, r12
    xor     ecx, ecx
.sl1:
    cmp     byte [rsi + rcx], 0
    je      .sl1d
    inc     ecx
    jmp     .sl1
.sl1d:
    mov     edx, ecx
    mov     rsi, r12
    mov     eax, SYS_WRITE
    mov     edi, STDERR
    syscall

    mov     eax, SYS_WRITE
    mov     edi, STDERR
    lea     rsi, [err_suffix]
    mov     edx, err_suffix_len
    syscall
    jmp     .exit_fail

    ; ============================================================
    ;  Error: Invalid short option (e.g. "-n", "-x")
    ; ============================================================
.err_short_opt:
    movzx   r12d, byte [rsi+1]

    mov     eax, SYS_WRITE
    mov     edi, STDERR
    lea     rsi, [err_inval]
    mov     edx, err_inval_len
    syscall

    push    r12
    mov     rsi, rsp
    mov     edx, 1
    mov     eax, SYS_WRITE
    mov     edi, STDERR
    syscall
    pop     r12

    mov     eax, SYS_WRITE
    mov     edi, STDERR
    lea     rsi, [err_suffix]
    mov     edx, err_suffix_len
    syscall
    jmp     .exit_fail

.opt_set_past:
    mov     r15d, 1
.opt_next:
    add     rbx, 8
    jmp     .opt_loop

.opt_done:
    jmp     .build_line

; ======================== Print and Exit (success) ============================
.print_exit_ok:
    mov     eax, SYS_WRITE
    mov     edi, STDOUT
    syscall                         ; ignore errors (about to exit)
    jmp     .exit

; ======================== Exit with code 1 ====================================
.exit_fail:
    mov     edi, 1
    mov     eax, SYS_EXIT
    syscall

; ======================== Default "y\n" Fast Path =============================
.default:
    lea     rdi, [buf]
    mov     ecx, BUFSZ / 2
    mov     eax, 0x0A79
    rep     stosw
    mov     r9d, BUFSZ
    jmp     .setup_write

; ======================== Argument Joining ====================================
.build_line:
    lea     rbx, [r14 + 8]        ; rbx = &argv[1]
    lea     rdi, [argbuf]
    xor     r8d, r8d               ; byte count
    xor     r12d, r12d             ; "--" skip flag
    xor     r13d, r13d             ; "any arg" flag

.bl_next:
    mov     rsi, [rbx]
    test    rsi, rsi
    jz      .bl_done
    add     rbx, 8

    test    r12d, r12d
    jnz     .bl_include
    cmp     word [rsi], 0x2D2D     ; "--"
    jne     .bl_include
    cmp     byte [rsi+2], 0
    jne     .bl_include
    mov     r12d, 1
    jmp     .bl_next

.bl_include:
    test    r13d, r13d
    jz      .bl_first_arg
    cmp     r8d, ARGBUF_MAX
    jge     .bl_done
    mov     byte [rdi], 0x20       ; space separator
    inc     rdi
    inc     r8d
    jmp     .bl_copy

.bl_first_arg:
    mov     r13d, 1

.bl_copy:
    cmp     r8d, ARGBUF_MAX
    jge     .bl_skip_rest
    lodsb
    test    al, al
    jz      .bl_next
    stosb
    inc     r8d
    jmp     .bl_copy

.bl_skip_rest:
    lodsb
    test    al, al
    jnz     .bl_skip_rest
    jmp     .bl_next

.bl_done:
    test    r13d, r13d
    jz      .default

    mov     byte [rdi], 0x0A       ; newline
    inc     r8d

    ; Fill buf with repeated copies of the line
    lea     rsi, [argbuf]
    lea     rdi, [buf]
    mov     r9, r8                  ; r9 = line length
    xor     r10d, r10d             ; r10 = bytes filled

.fill_loop:
    mov     rcx, BUFSZ
    sub     rcx, r10
    jle     .fill_done
    cmp     rcx, r9
    jle     .fill_copy
    mov     rcx, r9

.fill_copy:
    mov     r11, rcx
    push    rsi
    rep     movsb
    pop     rsi
    add     r10, r11
    cmp     r10, BUFSZ
    jb      .fill_loop

.fill_done:
    cmp     r9, BUFSZ
    jg      .long_line

    mov     rax, r10
    xor     edx, edx
    div     r9
    sub     r10, rdx
    mov     r9, r10
    jmp     .setup_write

.long_line:
    lea     r15, [argbuf]
    ; r9 already = line length
    jmp     .write_start

; ======================== Write Loop ==========================================
;
; Partial-write-safe: tracks position with rsi/rdx across iterations.
; macOS error detection: CARRY FLAG, not negative rax.
.setup_write:
    lea     r15, [buf]

.write_start:
    mov     edi, STDOUT             ; fd = 1 (survives syscall)

.write_outer:
    mov     rsi, r15                ; buffer start
    mov     rdx, r9                 ; total bytes

.write_loop:
    mov     eax, SYS_WRITE
    syscall
    jc      .write_error            ; carry flag = macOS error

    ; Success: rax = bytes written
    test    rax, rax
    jle     .exit                   ; unexpected zero/negative -> exit
    add     rsi, rax                ; advance pointer
    sub     rdx, rax                ; decrease remaining
    jg      .write_loop             ; partial write: continue
    jmp     .write_outer            ; buffer done: restart

.write_error:
    cmp     eax, 4                  ; EINTR?
    je      .write_loop             ; retry at same position
    ; EPIPE or other error: exit cleanly

; ======================== Exit (success, code 0) ==============================
.exit:
    xor     edi, edi
    mov     eax, SYS_EXIT
    syscall

; ############################################################################
;                           DATA SECTION
;
;  Default data for --help, --version, and error messages.
;  build.py replaces everything between @@DATA_START@@ and @@DATA_END@@
;  with byte-identical data from the system's GNU yes (if available).
; ############################################################################

; @@DATA_START@@
help_text:      db 0x55, 0x73, 0x61, 0x67, 0x65, 0x3a, 0x20, 0x79, 0x65, 0x73, 0x20, 0x5b, 0x53, 0x54, 0x52, 0x49
                db 0x4e, 0x47, 0x5d, 0x2e, 0x2e, 0x2e, 0x0a, 0x20, 0x20, 0x6f, 0x72, 0x3a, 0x20, 0x20, 0x79, 0x65
                db 0x73, 0x20, 0x4f, 0x50, 0x54, 0x49, 0x4f, 0x4e, 0x0a, 0x52, 0x65, 0x70, 0x65, 0x61, 0x74, 0x65
                db 0x64, 0x6c, 0x79, 0x20, 0x6f, 0x75, 0x74, 0x70, 0x75, 0x74, 0x20, 0x61, 0x20, 0x6c, 0x69, 0x6e
                db 0x65, 0x20, 0x77, 0x69, 0x74, 0x68, 0x20, 0x61, 0x6c, 0x6c, 0x20, 0x73, 0x70, 0x65, 0x63, 0x69
                db 0x66, 0x69, 0x65, 0x64, 0x20, 0x53, 0x54, 0x52, 0x49, 0x4e, 0x47, 0x28, 0x73, 0x29, 0x2c, 0x20
                db 0x6f, 0x72, 0x20, 0x27, 0x79, 0x27, 0x2e, 0x0a, 0x0a, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x2d
                db 0x2d, 0x68, 0x65, 0x6c, 0x70, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x64, 0x69, 0x73
                db 0x70, 0x6c, 0x61, 0x79, 0x20, 0x74, 0x68, 0x69, 0x73, 0x20, 0x68, 0x65, 0x6c, 0x70, 0x20, 0x61
                db 0x6e, 0x64, 0x20, 0x65, 0x78, 0x69, 0x74, 0x0a, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x2d, 0x2d
                db 0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x20, 0x20, 0x20, 0x20, 0x20, 0x6f, 0x75, 0x74, 0x70
                db 0x75, 0x74, 0x20, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x20, 0x69, 0x6e, 0x66, 0x6f, 0x72
                db 0x6d, 0x61, 0x74, 0x69, 0x6f, 0x6e, 0x20, 0x61, 0x6e, 0x64, 0x20, 0x65, 0x78, 0x69, 0x74, 0x0a
help_text_len equ $ - help_text

version_text:   db 0x79, 0x65, 0x73, 0x20, 0x28, 0x66, 0x63, 0x6f, 0x72, 0x65, 0x75, 0x74, 0x69, 0x6c, 0x73, 0x29
                db 0x0a
version_text_len equ $ - version_text

err_unrec:      db 0x79, 0x65, 0x73, 0x3a, 0x20, 0x75, 0x6e, 0x72, 0x65, 0x63, 0x6f, 0x67, 0x6e, 0x69, 0x7a, 0x65
                db 0x64, 0x20, 0x6f, 0x70, 0x74, 0x69, 0x6f, 0x6e, 0x20, 0x27
err_unrec_len equ $ - err_unrec

err_inval:      db 0x79, 0x65, 0x73, 0x3a, 0x20, 0x69, 0x6e, 0x76, 0x61, 0x6c, 0x69, 0x64, 0x20, 0x6f, 0x70, 0x74
                db 0x69, 0x6f, 0x6e, 0x20, 0x2d, 0x2d, 0x20, 0x27
err_inval_len equ $ - err_inval

err_suffix:     db 0x27, 0x0a, 0x54, 0x72, 0x79, 0x20, 0x27, 0x79, 0x65, 0x73, 0x20, 0x2d, 0x2d, 0x68, 0x65, 0x6c
                db 0x70, 0x27, 0x20, 0x66, 0x6f, 0x72, 0x20, 0x6d, 0x6f, 0x72, 0x65, 0x20, 0x69, 0x6e, 0x66, 0x6f
                db 0x72, 0x6d, 0x61, 0x74, 0x69, 0x6f, 0x6e, 0x2e, 0x0a
err_suffix_len equ $ - err_suffix
; @@DATA_END@@
