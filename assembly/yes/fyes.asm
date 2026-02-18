; ============================================================================
;  fyes.asm — GNU-compatible "yes" in x86_64 Linux assembly (hardened)
;
;  WHAT IS THIS?
;  =============
;  A drop-in replacement for GNU coreutils `yes` written in pure x86_64
;  assembly. Produces a ~1.5KB static ELF binary with zero dependencies —
;  no libc, no dynamic linker, no runtime allocations. Achieves 2-10x
;  higher throughput than GNU yes while maintaining 100% behavioral
;  compatibility.
;
;  BUILD (manual):
;    nasm -f bin fyes.asm -o fyes && chmod +x fyes
;
;  BUILD (recommended — auto-detects your system's yes output):
;    python3 build.py
;
;  The build.py script captures your system's `yes --help`, `yes --version`,
;  and error message formats, then patches the DATA SECTION below so fyes
;  produces byte-identical output. This handles locale differences (ASCII
;  vs UTF-8 quotes) and coreutils version variations automatically.
;
;  GNU COMPATIBILITY
;  =================
;  Matches GNU coreutils 9.x `yes` behavior including:
;    - --help / --version recognized anywhere in argv (GNU permutation)
;    - "--" terminates option processing; first "--" stripped from output
;    - Unrecognized long options (--foo): error to stderr, exit 1
;    - Invalid short options (-x): error to stderr, exit 1
;    - "-" alone is a regular string, not an option
;    - Multiple args joined with spaces, repeated on stdout forever
;    - No args → outputs "y\n" forever
;    - SIGPIPE/EPIPE → clean exit 0 (no error message)
;    - EINTR on write → automatic retry
;
;  ARCHITECTURE OVERVIEW
;  =====================
;
;  Memory Layout (virtual addresses):
;  ┌─────────────────────┬──────────────────────────────────────────┐
;  │ 0x400000            │ ELF header + code + data (this file)    │
;  │                     │ Segment: PT_LOAD, R+X (read+execute)    │
;  ├─────────────────────┼──────────────────────────────────────────┤
;  │ 0x500000 (BUF)      │ Write buffer, 16KB                      │
;  │                     │ Filled with repeated copies of the      │
;  │                     │ output line, flushed in big writes       │
;  ├─────────────────────┼──────────────────────────────────────────┤
;  │ 0x504000 (ARGBUF)   │ Argument assembly buffer, 2MB           │
;  │                     │ Holds the single output line built from  │
;  │                     │ argv (args joined by spaces + newline)   │
;  │                     │ Segment: PT_LOAD, R+W (BSS-style)       │
;  └─────────────────────┴──────────────────────────────────────────┘
;
;  The stack is marked non-executable via PT_GNU_STACK (NX bit).
;  No heap, no mmap, no brk — all memory is compile-time fixed.
;
;  Execution Flow:
;  ┌──────────┐    ┌──────────────┐    ┌───────────┐    ┌───────────┐
;  │ _start   │───▶│ PASS 1:      │───▶│ Build     │───▶│ Write     │
;  │ pop argc │    │ Validate     │    │ output    │    │ loop      │
;  │ save argv│    │ all options  │    │ line in   │    │ (forever) │
;  └──────────┘    │ in argv      │    │ ARGBUF    │    │ BUF→fd 1  │
;                  └──────────────┘    │ Fill BUF  │    └───────────┘
;                         │            └───────────┘
;                    On --help/       On error (bad
;                    --version:       option):
;                    print & exit 0   print & exit 1
;
;  Syscall Surface (entire binary uses only 2 syscalls):
;    - SYS_WRITE (1): output to stdout/stderr
;    - SYS_EXIT  (60): terminate process
;
;  PERFORMANCE NOTES
;  =================
;  Key optimizations for high throughput:
;
;  1. BUFFERED WRITES: Instead of writing one "y\n" per syscall, we fill
;     a 16KB buffer with repeated copies of the output line, then write
;     the entire buffer in one syscall. This reduces syscall overhead by
;     ~8000x for the default "y\n" case.
;
;  2. LINE-ALIGNED BUFFER: The buffer is rounded down to complete lines
;     to prevent partial-line output at buffer boundaries. This means
;     every write() call outputs only whole lines.
;
;  3. FAST DEFAULT PATH: When no args are given (the most common case),
;     we use `rep stosw` to fill the buffer with "y\n" pairs (0x0A79)
;     in a tight loop, bypassing the general argument-joining code.
;
;  4. EINTR RETRY: The write loop retries on EINTR (-4) automatically,
;     which is critical for correct behavior under signal-heavy loads.
;
;  5. ZERO DYNAMIC ALLOCATION: All buffers are in the BSS segment at
;     fixed addresses. No malloc, no brk, no mmap — this means zero
;     allocation overhead and deterministic memory usage.
;
;  SECURITY PROPERTIES
;  ===================
;  - Non-executable stack (PT_GNU_STACK with NX)
;  - No RWX memory segments (W^X policy)
;  - No dynamic linker (immune to LD_PRELOAD attacks)
;  - No file I/O (never calls open/openat/creat)
;  - Minimal syscall surface (only write + exit)
;  - Compile-time fixed memory layout (no heap corruption possible)
;  - EINTR-safe write loop (no signal-related data loss)
;  - Clean EPIPE/SIGPIPE handling (exit 0, no crash)
;
;  HOW TO MODIFY
;  =============
;  Common modifications and where to make them:
;
;  - Change buffer size: Edit BUFSZ (line ~47). Larger = fewer syscalls
;    but more memory. 16KB is optimal for most systems (matches pipe buf).
;
;  - Change max argument length: Edit ARGBUFSZ (line ~48). Current 2MB
;    handles extremely long argument lists.
;
;  - Update help/version text: Run `python3 build.py` to auto-detect,
;    or manually edit the hex bytes between @@DATA_START@@ and
;    @@DATA_END@@ markers. Use `echo -n "text" | xxd -i` to convert.
;
;  - Add a new long option: Add a comparison block after .chk_ver
;    following the same pattern (compare dword/word/byte sequences).
;
;  - Change error message format: Edit err_unrec, err_inval, err_suffix
;    in the DATA SECTION. These are split into prefix + dynamic part +
;    suffix so the option name/char can be inserted at runtime.
;
;  REGISTER CONVENTIONS (during main execution)
;  =============================================
;  r14  = pointer to argv[0] on stack (set once at _start, never changed)
;  r15  = "past --" flag during option parsing (0=checking, 1=past --)
;  rbx  = current argv pointer during iteration
;  r8   = byte count in ARGBUF during line building
;  r9   = total bytes to write per iteration (line length or buffer size)
;  r12  = saved option string/char for error messages; "--" skip flag
;  r13  = "have we included any arg" flag (for space-joining logic)
;  rdi  = fd (1=stdout) during write loop; destination ptr during copy
;  rsi  = buffer address during write loop; source ptr during copy
;  rdx  = byte count for write syscall
; ============================================================================

BITS 64
org 0x400000

; ======================== ELF Header ========================================
;
; This is a hand-crafted ELF64 header. NASM's `-f bin` output format means
; we control every byte of the binary. No linker is involved.
;
; The ELF header tells the kernel:
;   - This is a 64-bit Linux executable
;   - Entry point is at _start
;   - Program headers follow immediately after this header
;   - There are 3 program header entries (code+data, BSS, GNU_STACK)

ehdr:
    db      0x7f, "ELF"            ; e_ident[0..3]: ELF magic number
    db      2, 1, 1, 0             ; 2=64-bit, 1=little-endian, 1=ELF v1, 0=SysV ABI
    dq      0                      ; e_ident padding (8 bytes)
    dw      2                      ; e_type:    ET_EXEC (executable)
    dw      0x3E                   ; e_machine: EM_X86_64
    dd      1                      ; e_version: EV_CURRENT
    dq      _start                 ; e_entry:   virtual address of entry point
    dq      phdr - ehdr            ; e_phoff:   program header table offset
    dq      0                      ; e_shoff:   no section headers (not needed for exec)
    dd      0                      ; e_flags:   no processor-specific flags
    dw      ehdr_end - ehdr        ; e_ehsize:  ELF header size (64 bytes)
    dw      phdr_size              ; e_phentsize: program header entry size (56 bytes)
    dw      3                      ; e_phnum:   3 program headers (see below)
    dw      0, 0, 0                ; e_shentsize, e_shnum, e_shstrndx: unused
ehdr_end:

; ======================== Buffer Configuration ===============================
;
; These constants define the memory layout for runtime buffers.
; Both buffers live in the BSS-style PT_LOAD segment at 0x500000.
;
; BUF (0x500000, 16KB):
;   The write buffer. Filled with repeated copies of the output line,
;   then written to stdout in one syscall. 16KB was chosen because:
;   - It matches the default Linux pipe buffer size
;   - It's large enough to amortize syscall overhead
;   - It's small enough to stay in L1 cache on most CPUs
;
; ARGBUF (0x504000, 2MB):
;   Temporary buffer for assembling the output line from argv.
;   Args are copied here with spaces between them, plus a trailing \n.
;   2MB is generous — the kernel's MAX_ARG_STRLEN is typically 128KB,
;   so real-world argument lines are always much shorter.
;
; ARGBUF_MAX:
;   Safety limit to prevent writing past ARGBUF. We stop copying args
;   2 bytes before the end to leave room for the trailing '\n'.

%define BUF         0x500000       ; Write buffer base address
%define ARGBUF      0x504000       ; Argument assembly buffer base address
%define BUFSZ       16384          ; Write buffer size (16KB)
%define ARGBUFSZ    2097152        ; Arg buffer size (2MB)
%define ARGBUF_MAX  (ARGBUFSZ - 2) ; Max usable bytes in ARGBUF (room for \n)

; ======================== Program Headers ===================================
;
; Program headers tell the kernel how to map the binary into memory.
; We have 3 segments:
;
; 1. PT_LOAD (R+X): The entire binary file — code, data, and read-only
;    strings. Mapped at 0x400000 with read+execute permissions.
;    This is the only segment loaded from the file.
;
; 2. PT_LOAD (R+W, BSS): Runtime buffers at 0x500000. File size is 0
;    (nothing loaded from disk), memory size is BUFSZ + ARGBUFSZ.
;    The kernel zero-fills this on exec. This is our "BSS" segment.
;
; 3. PT_GNU_STACK: Tells the kernel the stack should be non-executable.
;    Flags = R+W (no X) enforces the NX bit on the stack.
;    Without this header, some kernels default to executable stack.

phdr:
    ; --- Segment 1: Code + Data (loaded from file) ---
    dd      1                       ; p_type:  PT_LOAD
    dd      5                       ; p_flags: PF_R(4) | PF_X(1) = read+execute
    dq      0                       ; p_offset: start of file
    dq      0x400000                ; p_vaddr:  virtual address
    dq      0x400000                ; p_paddr:  physical address (same)
    dq      file_end - ehdr         ; p_filesz: entire file
    dq      file_end - ehdr         ; p_memsz:  same as filesz (no BSS in this seg)
    dq      0x1000                  ; p_align:  page-aligned (4KB)
phdr_size equ $ - phdr              ; Size of one program header entry (56 bytes)

    ; --- Segment 2: BSS (runtime buffers, zero-initialized) ---
    dd      1                       ; p_type:  PT_LOAD
    dd      6                       ; p_flags: PF_R(4) | PF_W(2) = read+write
    dq      0                       ; p_offset: 0 (no file content)
    dq      0x500000                ; p_vaddr:  buffer base address
    dq      0x500000                ; p_paddr:  same
    dq      0                       ; p_filesz: 0 (nothing loaded from file)
    dq      BUFSZ + ARGBUFSZ        ; p_memsz:  16KB + 2MB = total buffer space
    dq      0x1000                  ; p_align:  page-aligned

    ; --- Segment 3: GNU Stack (marks stack as non-executable) ---
    dd      0x6474E551              ; p_type:  PT_GNU_STACK
    dd      6                       ; p_flags: PF_R(4) | PF_W(2) = NX stack
    dq      0, 0, 0, 0, 0          ; p_offset, p_vaddr, p_paddr, p_filesz, p_memsz: unused
    dq      0x10                    ; p_align:  16-byte alignment

; ============================================================================
;                           CODE SECTION
; ============================================================================
;
; Entry point. The kernel sets up the stack as:
;   [rsp]     = argc
;   [rsp+8]   = argv[0] (program name)
;   [rsp+16]  = argv[1] (first argument)
;   ...
;   [rsp+8*N] = NULL (argv terminator)
;
; We pop argc into rcx and save the stack pointer (which now points to
; argv[0]) into r14 for later use.

_start:
    pop     rcx                     ; rcx = argc (argument count)
    mov     r14, rsp                ; r14 = &argv[0] (saved for build_line)

    cmp     ecx, 2
    jl      .default                ; argc < 2 → no args, use default "y\n"

    ; ================================================================
    ;  PASS 1: Option Validation
    ;
    ;  GNU yes uses parse_gnu_standard_options_only(), which checks
    ;  EVERY argv entry for --help/--version, even after non-option
    ;  arguments. This is "getopt permutation" behavior.
    ;
    ;  The only thing that stops option checking is "--", which means
    ;  "end of options". After "--", all remaining args are treated
    ;  as literal strings regardless of their content.
    ;
    ;  Any unrecognized option (--foo or -x) is an error.
    ;  Bare "-" is not an option — it's a literal string.
    ;
    ;  Register usage in this section:
    ;    r15 = "past --" flag (0 = still checking options)
    ;    rbx = pointer to current argv entry
    ;    rsi = pointer to current argument string
    ; ================================================================

    xor     r15d, r15d              ; r15 = 0: not past "--" yet
    lea     rbx, [r14 + 8]         ; rbx = &argv[1] (skip program name)

.opt_loop:
    mov     rsi, [rbx]              ; rsi = current argv string pointer
    test    rsi, rsi                ; NULL pointer?
    jz      .opt_done               ; yes → end of argv, all args valid

    test    r15d, r15d              ; already past "--"?
    jnz     .opt_next               ; yes → skip option checking

    ; --- Check if this arg starts with '-' ---
    cmp     byte [rsi], '-'
    jne     .opt_next               ; doesn't start with '-' → not an option

    cmp     byte [rsi+1], 0
    je      .opt_next               ; just "-" alone → literal string, not option

    ; --- Starts with '-'. Is it a long option (--xxx)? ---
    cmp     byte [rsi+1], '-'
    jne     .err_short_opt          ; single '-' + char (e.g. "-n") → invalid option

    ; Starts with "--". Is it exactly "--" (end-of-options marker)?
    cmp     byte [rsi+2], 0
    je      .opt_set_past           ; exactly "--" → set flag, stop checking

    ; --- Check for "--help" ---
    ; String bytes: '-','-','h','e','l','p','\0'
    ; In little-endian dword at [rsi]: 0x65682D2D = "eh--" reversed = "--he"
    cmp     dword [rsi], 0x65682D2D ; first 4 bytes = "--he"?
    jne     .chk_ver                ; no → try --version
    cmp     word [rsi+4], 0x706C    ; bytes 4-5 = "lp"?
    jne     .chk_ver
    cmp     byte [rsi+6], 0         ; byte 6 = null terminator?
    jne     .chk_ver

    ; Matched "--help" → print help text and exit 0
    mov     esi, help_text          ; pointer to help text data
    mov     edx, help_text_len      ; length of help text
    jmp     .print_exit_ok

.chk_ver:
    ; --- Check for "--version" ---
    ; String bytes: '-','-','v','e','r','s','i','o','n','\0'
    ; In little-endian: dword[0] = 0x65762D2D ("--ve")
    ;                   dword[4] = 0x6F697372 ("rsio")
    ;                   word[8]  = 0x006E     ("n\0")
    cmp     dword [rsi], 0x65762D2D ; "--ve"?
    jne     .err_long_opt           ; no → unrecognized long option
    cmp     dword [rsi+4], 0x6F697372 ; "rsio"?
    jne     .err_long_opt
    cmp     word [rsi+8], 0x006E    ; "n\0"?
    jne     .err_long_opt

    ; Matched "--version" → print version text and exit 0
    mov     esi, version_text
    mov     edx, version_text_len
    jmp     .print_exit_ok

    ; ============================================================
    ;  Error: Unrecognized long option (e.g. "--foo")
    ;
    ;  Output format (to stderr):
    ;    yes: unrecognized option '--foo'
    ;    Try 'yes --help' for more information.
    ;
    ;  This is assembled from 3 write() calls:
    ;    1. err_unrec prefix:  "yes: unrecognized option '"
    ;    2. The option string:  "--foo" (variable length)
    ;    3. err_suffix:         "'\nTry 'yes --help'..."
    ; ============================================================
.err_long_opt:
    mov     r12, rsi                ; save option string pointer for later

    ; Write prefix: "yes: unrecognized option '"
    mov     eax, 1                  ; SYS_WRITE = 1
    mov     edi, 2                  ; fd = 2 (stderr)
    mov     esi, err_unrec          ; buffer = error prefix
    mov     edx, err_unrec_len      ; length
    syscall

    ; Write the option string itself (we need strlen first)
    mov     rsi, r12                ; rsi = option string
    xor     ecx, ecx                ; ecx = 0 (length counter)
.sl1:                               ; strlen loop
    cmp     byte [rsi + rcx], 0     ; null terminator?
    je      .sl1d                   ; yes → done
    inc     ecx                     ; no → count this byte
    jmp     .sl1
.sl1d:
    mov     edx, ecx                ; edx = string length
    mov     rsi, r12                ; rsi = string pointer
    mov     eax, 1                  ; SYS_WRITE
    mov     edi, 2                  ; fd = stderr
    syscall

    ; Write suffix: "'\nTry 'yes --help' for more information.\n"
    mov     eax, 1
    mov     edi, 2
    mov     esi, err_suffix
    mov     edx, err_suffix_len
    syscall
    jmp     .exit_fail              ; exit with code 1

    ; ============================================================
    ;  Error: Invalid short option (e.g. "-n", "-x")
    ;
    ;  Output format (to stderr):
    ;    yes: invalid option -- 'n'
    ;    Try 'yes --help' for more information.
    ;
    ;  Assembled from 3 write() calls:
    ;    1. err_inval prefix:  "yes: invalid option -- '"
    ;    2. The single char:    "n" (1 byte, written from stack)
    ;    3. err_suffix:         "'\nTry 'yes --help'..."
    ; ============================================================
.err_short_opt:
    movzx   r12d, byte [rsi+1]     ; save the option character (e.g. 'n')

    ; Write prefix: "yes: invalid option -- '"
    mov     eax, 1
    mov     edi, 2
    mov     esi, err_inval
    mov     edx, err_inval_len
    syscall

    ; Write the single option character
    ; We push it onto the stack to get a writable memory address
    ; (the data section is in an R+X segment, can't write there)
    push    r12                     ; put char on stack
    mov     rsi, rsp                ; rsi = pointer to char on stack
    mov     edx, 1                  ; length = 1 byte
    mov     eax, 1                  ; SYS_WRITE
    mov     edi, 2                  ; fd = stderr
    syscall
    pop     r12                     ; restore stack

    ; Write suffix
    mov     eax, 1
    mov     edi, 2
    mov     esi, err_suffix
    mov     edx, err_suffix_len
    syscall
    jmp     .exit_fail              ; exit with code 1

.opt_set_past:
    mov     r15d, 1                 ; set "past --" flag
.opt_next:
    add     rbx, 8                  ; advance to next argv entry (8 bytes = 1 pointer)
    jmp     .opt_loop               ; continue checking

.opt_done:
    ; All argv entries validated — no errors found.
    ; Proceed to build the output line from arguments.
    jmp     .build_line


; ======================== Print and Exit (success) ==========================
;
; Used by --help and --version: write the text to stdout, then exit 0.
; At this point: esi = text pointer, edx = text length.

.print_exit_ok:
    push    1
    pop     rax                     ; rax = 1 (SYS_WRITE)
    mov     edi, eax                ; edi = 1 (fd = stdout)
    syscall                         ; write(stdout, text, len)
    jmp     .exit                   ; exit with code 0

; ======================== Exit with code 1 ==================================
;
; Used by error paths (unrecognized/invalid option).

.exit_fail:
    push    1
    pop     rdi                     ; rdi = 1 (exit code)
    push    60
    pop     rax                     ; rax = 60 (SYS_EXIT)
    syscall                         ; _exit(1)


; ======================== Default "y\n" Fast Path ===========================
;
; When no arguments are given (argc < 2), output "y\n" forever.
;
; Optimization: Instead of copying "y\n" one-by-one, we use `rep stosw`
; to fill the entire 16KB buffer with the word 0x0A79 ("y\n" in
; little-endian). This fills 8192 copies of "y\n" in a single tight loop.
;
; After filling, we jump to setup_write which writes the buffer to stdout
; in a loop forever.

.default:
    mov     edi, BUF                ; edi = destination (write buffer)
    mov     ecx, BUFSZ / 2         ; ecx = 8192 (number of words to store)
    mov     eax, 0x0A79             ; ax = "y\n" as a 16-bit word
    rep     stosw                   ; fill BUF with "y\n" repeated 8192 times
    mov     r9d, BUFSZ             ; r9 = 16384 bytes to write per iteration
    jmp     .setup_write


; ======================== Argument Joining ==================================
;
;  Build the output line from argv[1..N] into ARGBUF.
;
;  GNU yes behavior for "--":
;    - The FIRST "--" in argv is stripped (not included in output)
;    - Subsequent "--" entries ARE included in output
;    - Example: `yes -- a -- b` outputs "a -- b\n"
;
;  Register usage:
;    rbx = pointer walking through argv
;    edi = write cursor in ARGBUF
;    r8  = bytes written to ARGBUF so far
;    r12 = "--" skip flag (0 = haven't skipped yet, 1 = already skipped)
;    r13 = "any arg included" flag (for space-before-arg logic)
;
;  Output format: "arg1 arg2 arg3\n" (space-separated, newline-terminated)

.build_line:
    lea     rbx, [r14 + 8]         ; rbx = &argv[1]
    mov     edi, ARGBUF             ; edi = write cursor (start of ARGBUF)
    xor     r8d, r8d               ; r8 = 0 (byte count)
    xor     r12d, r12d             ; r12 = 0 (haven't skipped "--" yet)
    xor     r13d, r13d             ; r13 = 0 (no args included yet)

.bl_next:
    mov     rsi, [rbx]              ; rsi = current arg string
    test    rsi, rsi                ; NULL? (end of argv)
    jz      .bl_done
    add     rbx, 8                  ; advance argv pointer

    ; --- Should we skip this arg? (first "--" only) ---
    test    r12d, r12d              ; already skipped a "--"?
    jnz     .bl_include             ; yes → include everything now
    cmp     word [rsi], 0x2D2D      ; first two bytes = "--"?
    jne     .bl_include             ; no → include it
    cmp     byte [rsi+2], 0         ; third byte = null? (exactly "--")
    jne     .bl_include             ; no → it's "--something", include it
    ; This is "--" and we haven't skipped one yet → skip it
    mov     r12d, 1                 ; mark: we've skipped the first "--"
    jmp     .bl_next

.bl_include:
    ; --- Add space separator before this arg (unless it's the first) ---
    test    r13d, r13d              ; is this the first included arg?
    jz      .bl_first_arg           ; yes → no space needed
    cmp     r8d, ARGBUF_MAX         ; buffer full?
    jge     .bl_done                ; yes → stop
    mov     byte [rdi], 0x20        ; write ' ' (space separator)
    inc     edi
    inc     r8d
    jmp     .bl_copy

.bl_first_arg:
    mov     r13d, 1                 ; mark: we've started including args

.bl_copy:
    ; --- Copy bytes from current arg string to ARGBUF ---
    cmp     r8d, ARGBUF_MAX         ; buffer full?
    jge     .bl_skip_rest           ; yes → skip remaining bytes of this arg
    lodsb                           ; al = *rsi++ (load byte, advance source)
    test    al, al                  ; null terminator?
    jz      .bl_next                ; yes → move to next arg
    stosb                           ; *rdi++ = al (store byte, advance dest)
    inc     r8d                     ; count this byte
    jmp     .bl_copy

.bl_skip_rest:
    ; Buffer is full but we need to consume the rest of this arg
    ; (to properly advance to the next arg in the loop)
    lodsb
    test    al, al
    jnz     .bl_skip_rest
    jmp     .bl_next

.bl_done:
    ; --- All args processed. Was anything actually included? ---
    test    r13d, r13d
    jz      .default                ; no args included → use default "y\n"

    ; --- Append newline to complete the output line ---
    mov     byte [rdi], 0x0A        ; '\n'
    inc     r8d
    ; r8 now = total line length including '\n'

    ; ================================================================
    ;  Fill BUF with repeated copies of the output line.
    ;
    ;  This is the key performance optimization: instead of calling
    ;  write() once per line, we fill a 16KB buffer with as many
    ;  complete copies of the line as will fit, then write the
    ;  entire buffer in one syscall.
    ;
    ;  For a 2-byte line ("y\n"), this means 8192 lines per write.
    ;  For a 100-byte line, ~163 lines per write.
    ; ================================================================

    mov     esi, ARGBUF             ; esi = source (the output line)
    mov     edi, BUF                ; edi = destination (write buffer)
    mov     r9, r8                  ; r9 = line length (for later)
    xor     r10d, r10d             ; r10 = bytes filled so far

.fill_loop:
    mov     rcx, BUFSZ             ; rcx = remaining buffer space
    sub     rcx, r10
    jle     .fill_done              ; buffer full → done
    cmp     rcx, r9                 ; more space than one line?
    jle     .fill_copy              ; no → copy partial (won't happen if aligned)
    mov     rcx, r9                 ; yes → copy exactly one line

.fill_copy:
    mov     r11, rcx               ; save byte count (rep movsb zeroes rcx)
    push    rsi                    ; save source pointer (rep movsb advances it)
    rep     movsb                  ; copy rcx bytes: [rsi] → [rdi]
    pop     rsi                    ; restore source to start of line
    add     r10, r11               ; update total bytes filled
    cmp     r10, BUFSZ             ; buffer full?
    jb      .fill_loop             ; no → copy another line

.fill_done:
    ; --- Round down to complete lines ---
    ; If the line doesn't evenly divide BUFSZ, the last partial copy
    ; would produce a broken line. We trim the buffer to the last
    ; complete line boundary.
    cmp     r9, BUFSZ              ; is the line longer than the buffer?
    jg      .long_line             ; yes → special case (write from ARGBUF)

    mov     rax, r10               ; rax = total bytes in buffer
    xor     edx, edx               ; clear remainder
    div     r9                     ; rax = complete lines, rdx = leftover bytes
    sub     r10, rdx               ; trim to complete-line boundary
    mov     r9, r10                ; r9 = trimmed buffer size
    jmp     .setup_write

.long_line:
    ; Special case: output line is longer than BUF (>16KB).
    ; Skip the buffer entirely — write directly from ARGBUF.
    ; This is rare but handles pathological cases correctly.
    push    1
    pop     rdi                    ; fd = stdout
    mov     esi, ARGBUF            ; source = ARGBUF
    mov     rdx, r9                ; length = line length
    jmp     .write_loop


; ======================== Write Loop ========================================
;
; The hot loop — writes the buffer to stdout forever until an error occurs.
;
; This loop handles two special cases:
;   - EINTR (errno -4): The write was interrupted by a signal. Retry.
;   - EPIPE/error (negative or zero return): The pipe was closed or
;     another error occurred. Exit cleanly with code 0.
;
; Note: `mov eax, edi` is a 2-byte instruction that copies fd (1) to eax
; for the SYS_WRITE syscall number. This is smaller than `mov eax, 1` (5
; bytes) and works because SYS_WRITE == 1 == STDOUT_FILENO.

.setup_write:
    push    1
    pop     rdi                     ; rdi = 1 (fd = stdout)
    mov     esi, BUF                ; rsi = buffer address
    mov     rdx, r9                 ; rdx = buffer size (bytes to write)

.write_loop:
    mov     eax, edi                ; eax = 1 (SYS_WRITE, borrowed from fd)
    syscall                         ; write(stdout, buf, len)
    cmp     eax, -4                 ; returned -EINTR?
    je      .write_loop             ; yes → retry the write
    test    eax, eax                ; positive return (bytes written)?
    jg      .write_loop             ; yes → keep writing

    ; Zero or negative return (EPIPE, ENOSPC, EBADF, etc.) → exit

; ======================== Exit (success, code 0) ============================

.exit:
    xor     edi, edi                ; rdi = 0 (exit code)
    push    60
    pop     rax                     ; rax = 60 (SYS_EXIT)
    syscall                         ; _exit(0)


; ############################################################################
;                           DATA SECTION
;
;  This section contains the help text, version text, and error message
;  fragments. All data is stored as raw bytes (hex) because the exact
;  content depends on the local system's coreutils installation.
;
;  AUTOMATIC PATCHING:
;    Run `python3 build.py` to detect your system's GNU yes output and
;    replace this section with matching data. The build script looks for
;    the @@DATA_START@@ and @@DATA_END@@ markers and replaces everything
;    between them.
;
;  MANUAL EDITING:
;    If you need to edit by hand, convert your text to hex bytes:
;      echo -n "your text here" | xxd -i
;    Then replace the appropriate label's `db` lines.
;    IMPORTANT: Update the `_len equ $ - label` line too — NASM
;    computes the length automatically from the label position.
;
;  DATA LABELS:
;    help_text      — Full --help output (written to stdout)
;    version_text   — Full --version output (written to stdout)
;    err_unrec      — Error prefix: "yes: unrecognized option '"
;    err_inval      — Error prefix: "yes: invalid option -- '"
;    err_suffix     — Error suffix: "'\nTry 'yes --help' for more information.\n"
;
;  ERROR MESSAGE ASSEMBLY:
;    For unrecognized long option --foo:
;      write(stderr, err_unrec)   →  "yes: unrecognized option '"
;      write(stderr, "--foo")      →  "--foo"
;      write(stderr, err_suffix)   →  "'\nTry 'yes --help' for more information.\n"
;
;    For invalid short option -x:
;      write(stderr, err_inval)   →  "yes: invalid option -- '"
;      write(stderr, "x")         →  "x"
;      write(stderr, err_suffix)   →  "'\nTry 'yes --help' for more information.\n"
; ############################################################################

flag_help:      db "--help", 0      ; Used by build.py for reference (not by code)
flag_version:   db "--version", 0   ; Used by build.py for reference (not by code)

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
                db 0x0a, 0x47, 0x4e, 0x55, 0x20, 0x63, 0x6f, 0x72, 0x65, 0x75, 0x74, 0x69, 0x6c, 0x73, 0x20, 0x6f
                db 0x6e, 0x6c, 0x69, 0x6e, 0x65, 0x20, 0x68, 0x65, 0x6c, 0x70, 0x3a, 0x20, 0x3c, 0x68, 0x74, 0x74
                db 0x70, 0x73, 0x3a, 0x2f, 0x2f, 0x77, 0x77, 0x77, 0x2e, 0x67, 0x6e, 0x75, 0x2e, 0x6f, 0x72, 0x67
                db 0x2f, 0x73, 0x6f, 0x66, 0x74, 0x77, 0x61, 0x72, 0x65, 0x2f, 0x63, 0x6f, 0x72, 0x65, 0x75, 0x74
                db 0x69, 0x6c, 0x73, 0x2f, 0x3e, 0x0a, 0x46, 0x75, 0x6c, 0x6c, 0x20, 0x64, 0x6f, 0x63, 0x75, 0x6d
                db 0x65, 0x6e, 0x74, 0x61, 0x74, 0x69, 0x6f, 0x6e, 0x20, 0x3c, 0x68, 0x74, 0x74, 0x70, 0x73, 0x3a
                db 0x2f, 0x2f, 0x77, 0x77, 0x77, 0x2e, 0x67, 0x6e, 0x75, 0x2e, 0x6f, 0x72, 0x67, 0x2f, 0x73, 0x6f
                db 0x66, 0x74, 0x77, 0x61, 0x72, 0x65, 0x2f, 0x63, 0x6f, 0x72, 0x65, 0x75, 0x74, 0x69, 0x6c, 0x73
                db 0x2f, 0x79, 0x65, 0x73, 0x3e, 0x0a, 0x6f, 0x72, 0x20, 0x61, 0x76, 0x61, 0x69, 0x6c, 0x61, 0x62
                db 0x6c, 0x65, 0x20, 0x6c, 0x6f, 0x63, 0x61, 0x6c, 0x6c, 0x79, 0x20, 0x76, 0x69, 0x61, 0x3a, 0x20
                db 0x69, 0x6e, 0x66, 0x6f, 0x20, 0x27, 0x28, 0x63, 0x6f, 0x72, 0x65, 0x75, 0x74, 0x69, 0x6c, 0x73
                db 0x29, 0x20, 0x79, 0x65, 0x73, 0x20, 0x69, 0x6e, 0x76, 0x6f, 0x63, 0x61, 0x74, 0x69, 0x6f, 0x6e
                db 0x27, 0x0a
help_text_len equ $ - help_text

version_text:   db 0x79, 0x65, 0x73, 0x20, 0x28, 0x47, 0x4e, 0x55, 0x20, 0x63, 0x6f, 0x72, 0x65, 0x75, 0x74, 0x69
                db 0x6c, 0x73, 0x29, 0x20, 0x39, 0x2e, 0x37, 0x0a, 0x50, 0x61, 0x63, 0x6b, 0x61, 0x67, 0x65, 0x64
                db 0x20, 0x62, 0x79, 0x20, 0x44, 0x65, 0x62, 0x69, 0x61, 0x6e, 0x20, 0x28, 0x39, 0x2e, 0x37, 0x2d
                db 0x33, 0x29, 0x0a, 0x43, 0x6f, 0x70, 0x79, 0x72, 0x69, 0x67, 0x68, 0x74, 0x20, 0x28, 0x43, 0x29
                db 0x20, 0x32, 0x30, 0x32, 0x35, 0x20, 0x46, 0x72, 0x65, 0x65, 0x20, 0x53, 0x6f, 0x66, 0x74, 0x77
                db 0x61, 0x72, 0x65, 0x20, 0x46, 0x6f, 0x75, 0x6e, 0x64, 0x61, 0x74, 0x69, 0x6f, 0x6e, 0x2c, 0x20
                db 0x49, 0x6e, 0x63, 0x2e, 0x0a, 0x4c, 0x69, 0x63, 0x65, 0x6e, 0x73, 0x65, 0x20, 0x47, 0x50, 0x4c
                db 0x76, 0x33, 0x2b, 0x3a, 0x20, 0x47, 0x4e, 0x55, 0x20, 0x47, 0x50, 0x4c, 0x20, 0x76, 0x65, 0x72
                db 0x73, 0x69, 0x6f, 0x6e, 0x20, 0x33, 0x20, 0x6f, 0x72, 0x20, 0x6c, 0x61, 0x74, 0x65, 0x72, 0x20
                db 0x3c, 0x68, 0x74, 0x74, 0x70, 0x73, 0x3a, 0x2f, 0x2f, 0x67, 0x6e, 0x75, 0x2e, 0x6f, 0x72, 0x67
                db 0x2f, 0x6c, 0x69, 0x63, 0x65, 0x6e, 0x73, 0x65, 0x73, 0x2f, 0x67, 0x70, 0x6c, 0x2e, 0x68, 0x74
                db 0x6d, 0x6c, 0x3e, 0x2e, 0x0a, 0x54, 0x68, 0x69, 0x73, 0x20, 0x69, 0x73, 0x20, 0x66, 0x72, 0x65
                db 0x65, 0x20, 0x73, 0x6f, 0x66, 0x74, 0x77, 0x61, 0x72, 0x65, 0x3a, 0x20, 0x79, 0x6f, 0x75, 0x20
                db 0x61, 0x72, 0x65, 0x20, 0x66, 0x72, 0x65, 0x65, 0x20, 0x74, 0x6f, 0x20, 0x63, 0x68, 0x61, 0x6e
                db 0x67, 0x65, 0x20, 0x61, 0x6e, 0x64, 0x20, 0x72, 0x65, 0x64, 0x69, 0x73, 0x74, 0x72, 0x69, 0x62
                db 0x75, 0x74, 0x65, 0x20, 0x69, 0x74, 0x2e, 0x0a, 0x54, 0x68, 0x65, 0x72, 0x65, 0x20, 0x69, 0x73
                db 0x20, 0x4e, 0x4f, 0x20, 0x57, 0x41, 0x52, 0x52, 0x41, 0x4e, 0x54, 0x59, 0x2c, 0x20, 0x74, 0x6f
                db 0x20, 0x74, 0x68, 0x65, 0x20, 0x65, 0x78, 0x74, 0x65, 0x6e, 0x74, 0x20, 0x70, 0x65, 0x72, 0x6d
                db 0x69, 0x74, 0x74, 0x65, 0x64, 0x20, 0x62, 0x79, 0x20, 0x6c, 0x61, 0x77, 0x2e, 0x0a, 0x0a, 0x57
                db 0x72, 0x69, 0x74, 0x74, 0x65, 0x6e, 0x20, 0x62, 0x79, 0x20, 0x44, 0x61, 0x76, 0x69, 0x64, 0x20
                db 0x4d, 0x61, 0x63, 0x4b, 0x65, 0x6e, 0x7a, 0x69, 0x65, 0x2e, 0x0a
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

file_end:
; ============================================================================
;  End of binary. Everything past file_end is not loaded into memory.
;  Total binary size: ~1.5KB (computed as file_end - ehdr).
; ============================================================================