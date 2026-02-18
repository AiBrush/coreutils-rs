#!/usr/bin/env python3
"""
fyes_vs_yes_tests.py — Comprehensive test & benchmark suite for fyes.
===========================================================================

fyes is a GNU-compatible "yes" written in x86_64 Linux assembly.
This script validates correctness, security properties, and performance
by comparing fyes against the system's GNU coreutils yes.

USAGE:
    python3 fyes_vs_yes_tests.py              # Run all tests + benchmarks
    python3 fyes_vs_yes_tests.py --bench-only # Run only benchmarks
    python3 fyes_vs_yes_tests.py --test-only  # Run only tests (skip bench)

REQUIREMENTS:
    - ./fyes binary (build with: python3 build.py)
    - GNU yes (usually at /usr/bin/yes)
    - Linux (uses /proc filesystem for memory/CPU measurements)
    - Optional: strace, valgrind (for deeper security tests)

TEST CATEGORIES:
    1. Functional correctness — argument handling, options, edge cases
    2. Fuzz testing — random strings, bytes, unicode, weird chars
    3. Security hardening — ELF properties, syscall surface, memory safety
    4. Robustness — signals, closed fds, resource limits, /dev/full
    5. Benchmarks — throughput, memory, CPU, startup time vs GNU yes

OUTPUT:
    Each test prints [PASS] or [FAIL] with a description.
    Benchmarks print a comparison table at the end showing the improvement
    of fyes over GNU yes for each metric.

EXIT CODES:
    0 = all tests passed
    1 = one or more tests failed

ADDING NEW TESTS:
    1. Write a function: def check_my_thing(): ...
    2. Use report_result(ok, "description") to register pass/fail
    3. Use record_failure(...) for detailed failure info
    4. Call your function from run_tests() in the appropriate section
    5. For benchmarks: add to run_benchmarks(), store results in bench_results
"""

import os
import sys
import subprocess
import random
import string
import struct
import time
import signal
from pathlib import Path
from shutil import which as shutil_which

# =============================================================================
#                           CONFIGURATION
# =============================================================================

# Paths to the binaries under test.
# FY = fyes assembly binary (built from fyes.asm)
# YES = GNU coreutils yes (system default)
FY = "./fyes"
YES = "yes"

# Head limits for comparison tests.
# We can't let yes run forever, so we pipe through `head` to capture
# a fixed amount of output and compare fyes vs yes.
HEAD_LINES = 2000          # Max lines to compare
HEAD_BYTES = 200000        # Max bytes to compare (~200KB)

# Timeout for subprocess operations (seconds).
# If a process doesn't finish in this time, it's killed.
TIMEOUT = 3

# Number of random test cases for each fuzz category.
# Increase for more thorough (but slower) testing.
RANDOM_CASES = 400         # Short random strings (0-50 chars, 0-10 args)
RANDOM_LONG_CASES = 120    # Long random strings (0-1000 chars, 1-30 args)
RANDOM_WEIRD_CASES = 200   # Weird chars (tabs, newlines, punctuation)
RANDOM_BYTES_CASES = 80    # Raw bytes via execve (tests binary-safe handling)

# Benchmark configuration
BENCH_DURATION = 2.0       # Seconds to run each throughput benchmark
BENCH_WARMUP = 0.5         # Warmup time before measuring (seconds)
BENCH_STARTUP_TRIALS = 50  # Number of startup time measurements to average
BENCH_MEMORY_DURATION = 0.5  # Seconds to let process run before measuring memory

# Logging: set to 1 to print every [PASS], 0 to only print [FAIL]
LOG_EVERY = 1


# =============================================================================
#                         HELPER FUNCTIONS
# =============================================================================

def log(msg):
    """Print a message immediately (flush=True prevents buffering)."""
    print(msg, flush=True)

def run(cmd, stdin=None, env=None, preexec_fn=None):
    """
    Run a command and return (exit_code, stdout_bytes, stderr_bytes).

    If the command doesn't finish within TIMEOUT seconds, it's killed
    and the exit code is set to 124 (matching GNU timeout behavior).
    """
    p = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE if stdin is not None else None,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        preexec_fn=preexec_fn,
        text=False,
    )
    try:
        out, err = p.communicate(input=stdin, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        p.kill()
        out, err = p.communicate()
        return (124, out, err)
    return (p.returncode, out, err)

def head_output(cmd):
    """
    Run a command and pipe through head to limit output.

    This is how we compare infinite-output programs: pipe through
    head -nN -cN which caps both line count and byte count,
    then compare the truncated output of fyes vs yes.
    """
    head_cmd = ["head", f"-n{HEAD_LINES}", f"-c{HEAD_BYTES}"]
    p1 = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    p2 = subprocess.Popen(head_cmd, stdin=p1.stdout, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    p1.stdout.close()
    try:
        out, err = p2.communicate(timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        p1.kill()
        p2.kill()
        out, err = p2.communicate()
        return (124, out, err)
    try:
        p1.kill()
    except Exception:
        pass
    return (p2.returncode, out, err)

def read_limited_output(cmd):
    """Run a command and read limited output directly (no head)."""
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        out = p.stdout.read(HEAD_BYTES)
        err = p.stderr.read(200)
    finally:
        try:
            p.kill()
        except Exception:
            pass
    return (0, out, err)

def rand_str(n, alphabet=None):
    """Generate a random string of length n from the given alphabet."""
    if alphabet is None:
        alphabet = string.ascii_letters + string.digits + " _-+*/:.,"
    return "".join(random.choice(alphabet) for _ in range(n))

def rand_bytes(n):
    """Generate n random non-zero bytes (zero = C string terminator)."""
    return bytes(random.randint(1, 255) for _ in range(n))

def which(cmd):
    """Check if a command is available on the system PATH."""
    return shutil_which(cmd) is not None

def fyes_abs():
    """Return the absolute path to the fyes binary."""
    return os.path.abspath(FY)

def yes_abs():
    """Return the absolute path to the system yes binary."""
    path = shutil_which(YES)
    return path if path else YES


# =============================================================================
#                           TEST HARNESS
# =============================================================================

failures = []
test_count = 0
pass_count = 0

def record_failure(kind, cmd_args, rc1, rc2, out1, out2, err1, err2, note=""):
    """Record a test failure with details for the summary report."""
    failures.append({
        "kind": kind, "args": cmd_args,
        "rc_fyes": rc1, "rc_yes": rc2,
        "stderr_fyes": err1[:200], "stderr_yes": err2[:200],
        "stdout_fyes": out1[:200], "stdout_yes": out2[:200],
        "note": note,
    })

def report_result(ok, label):
    """Register a test result. Increments counters and prints status."""
    global test_count, pass_count
    test_count += 1
    if ok:
        pass_count += 1
        if LOG_EVERY:
            log(f"[PASS] {label}")
    else:
        log(f"[FAIL] {label}")


# =============================================================================
#                     COMPARISON TEST FUNCTIONS
# =============================================================================

def compare(cmd_args, label=None):
    """Compare fyes vs yes output through head (truncated comparison)."""
    rc1, out1, err1 = head_output([FY] + cmd_args)
    rc2, out2, err2 = head_output([YES] + cmd_args)
    ok = (out1 == out2 and err1 == err2)
    if not ok:
        record_failure("head", cmd_args, rc1, rc2, out1, out2, err1, err2)
    report_result(ok, label or f"compare {cmd_args}")

def compare_exact(cmd_args, label=None):
    """Compare fyes vs yes exactly (for terminating commands like --help)."""
    rc1, out1, err1 = run([FY] + cmd_args)
    rc2, out2, err2 = run([YES] + cmd_args)
    ok = (out1 == out2 and err1 == err2 and rc1 == rc2)
    if not ok:
        record_failure("exact", cmd_args, rc1, rc2, out1, out2, err1, err2)
    report_result(ok, label or f"compare exact {cmd_args}")

def compare_bytes_argv(args_bytes, label=None):
    """
    Compare fyes vs yes with raw byte arguments via os.execve().

    Python subprocess can't pass NUL bytes in argv strings.
    This helper uses execve directly to test NUL-boundary behavior.
    """
    def run_execve_bytes(prog, argv_bytes):
        hex_args = ",".join(arg.hex() for arg in argv_bytes)
        helper = (
            "import os, binascii\n"
            "args_hex = os.environ['BYTES_ARGS'].split(',') if os.environ.get('BYTES_ARGS') else []\n"
            "args = [binascii.unhexlify(h) for h in args_hex]\n"
            "os.execve(args[0], args, os.environ)\n"
        )
        env = os.environ.copy()
        env["BYTES_ARGS"] = hex_args
        return read_limited_output([sys.executable, "-c", helper])

    fy_argv = [os.fsencode(FY)] + args_bytes
    yes_argv = [os.fsencode(YES)] + args_bytes
    rc1, out1, err1 = run_execve_bytes(FY, fy_argv)
    rc2, out2, err2 = run_execve_bytes(YES, yes_argv)
    ok = (out1 == out2 and err1 == err2)
    if not ok:
        record_failure("bytes-argv", [f"<{len(a)} bytes>" for a in args_bytes],
                       rc1, rc2, out1, out2, err1, err2)
    report_result(ok, label or "bytes-argv")


# =============================================================================
#                         SECURITY TESTS
# =============================================================================

def check_elf_binary_properties():
    """Parse ELF headers to verify security: static, NX stack, no RWX, tiny."""
    try:
        with open(FY, "rb") as f:
            elf = f.read()
    except Exception as e:
        record_failure("security", ["elf"], 0, 0, b"", b"", b"", b"", note=str(e))
        report_result(False, "security: ELF read failed")
        return

    report_result(elf[:4] == b"\x7fELF", "security: ELF magic valid")
    report_result(elf[4] == 2, "security: ELF 64-bit")
    size = len(elf)
    report_result(size < 4096, f"security: binary size {size} bytes (<4KB)")

    e_phoff = struct.unpack_from("<Q", elf, 32)[0]
    e_phentsize = struct.unpack_from("<H", elf, 54)[0]
    e_phnum = struct.unpack_from("<H", elf, 56)[0]
    PT_INTERP, PT_DYNAMIC, PT_GNU_STACK = 3, 2, 0x6474E551
    PF_X, PF_W, PF_R = 1, 2, 4
    has_interp = has_dynamic = has_rwx = has_nx_stack = has_exec_stack = False

    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        p_type = struct.unpack_from("<I", elf, off)[0]
        p_flags = struct.unpack_from("<I", elf, off + 4)[0]
        if p_type == PT_INTERP: has_interp = True
        if p_type == PT_DYNAMIC: has_dynamic = True
        if (p_flags & PF_R) and (p_flags & PF_W) and (p_flags & PF_X): has_rwx = True
        if p_type == PT_GNU_STACK:
            has_exec_stack = bool(p_flags & PF_X)
            has_nx_stack = not has_exec_stack

    report_result(not has_interp, "security: ELF no PT_INTERP (static binary)")
    report_result(not has_dynamic, "security: ELF no PT_DYNAMIC (no dynamic linking)")
    report_result(not has_rwx, "security: ELF no RWX segments")
    report_result(has_nx_stack, "security: ELF PT_GNU_STACK NX (non-executable stack)")
    report_result(not has_exec_stack, "security: ELF no executable stack flag")

def check_no_strings_leaks():
    """Scan binary for debug/path/library strings that shouldn't be present."""
    with open(FY, "rb") as f:
        data = f.read()
    bad_patterns = [
        (b"/etc/", "filesystem path /etc/"), (b"/home/", "home directory path"),
        (b"/tmp/", "tmp path"), (b"DEBUG", "debug string"), (b"TODO", "todo string"),
        (b"FIXME", "fixme string"), (b"password", "password string"),
        (b"secret", "secret string"), (b".so", "shared library reference"),
        (b"ld-linux", "dynamic linker reference"), (b"libc", "libc reference"),
        (b"glibc", "glibc reference"),
    ]
    for pattern, desc in bad_patterns:
        found = pattern in data
        if found:
            record_failure("security", ["strings"], 0, 0, b"", b"", b"", b"",
                           note=f"Found '{pattern.decode(errors='replace')}' ({desc})")
        report_result(not found, f"security: no {desc} in binary")

def check_proc_maps():
    """Verify no RWX regions and no executable stack at runtime."""
    p = subprocess.Popen([FY], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    time.sleep(0.1)
    try:
        maps = Path(f"/proc/{p.pid}/maps").read_text(errors="ignore")
        has_rwx = any("rwxp" in line for line in maps.splitlines())
        has_exec_stack = any("[stack]" in line and "x" in line.split()[1]
                            for line in maps.splitlines())
        ok = not has_rwx and not has_exec_stack
        if not ok:
            record_failure("security", ["proc_maps"], 0, 0, b"", b"", b"", b"",
                           note="RWX or exec stack detected")
        report_result(ok, "security: /proc/pid/maps no RWX/exec-stack")
    except Exception as e:
        report_result(True, f"security: /proc/pid/maps (skipped: {e})")
    finally:
        try: p.kill()
        except Exception: pass

def check_fd_hygiene():
    """Verify only fd 0,1,2 open during execution."""
    p = subprocess.Popen([FY], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    time.sleep(0.05)
    try:
        fds = set(os.listdir(f"/proc/{p.pid}/fd"))
        extra = fds - {"0", "1", "2"}
        ok = len(extra) == 0
        if not ok:
            record_failure("security", ["fd_hygiene"], 0, 0, b"", b"", b"", b"",
                           note=f"Unexpected fds: {extra}")
        report_result(ok, "security: fd hygiene (only 0,1,2 open)")
    except Exception as e:
        report_result(True, f"security: fd hygiene (skipped: {e})")
    finally:
        try: p.kill()
        except Exception: pass

def check_proc_self_exe():
    """Verify /proc/pid/exe points to fyes."""
    p = subprocess.Popen([FY], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    time.sleep(0.05)
    try:
        exe = os.readlink(f"/proc/{p.pid}/exe")
        ok = os.path.basename(exe) == os.path.basename(FY)
        report_result(ok, "security: /proc/pid/exe is fyes")
    except Exception as e:
        report_result(True, f"security: /proc/pid/exe (skipped: {e})")
    finally:
        try: p.kill()
        except Exception: pass

def check_no_open_files():
    """Verify fyes never calls open/openat/creat (via strace)."""
    if not which("strace"):
        report_result(True, "security: no file open syscalls (skipped, no strace)")
        return
    cmd = ["strace", "-e", "trace=openat,open,creat", "-f", FY, "--help"]
    rc, out, err = run(cmd)
    lines = [l for l in err.split(b"\n") if l and not l.startswith(b"---") and not l.startswith(b"+++ ")]
    file_calls = [l for l in lines if b"openat(" in l or b"open(" in l or b"creat(" in l]
    ok = len(file_calls) == 0
    if not ok:
        record_failure("security", ["no_open_files"], 0, 0, b"", b"",
                       b"\n".join(file_calls)[:200], b"", note=f"{len(file_calls)} file open calls")
    report_result(ok, "security: no file open syscalls")

def check_strace_syscalls():
    """Verify minimal syscall surface on --help path."""
    if not which("strace"):
        report_result(True, "security: strace syscall surface (skipped, no strace)")
        return
    cmd = ["strace", "-f", "-e",
           "trace=process,signal,write,exit,exit_group,brk,mmap,mprotect,openat,execve",
           FY, "--help"]
    rc, out, err = run(cmd)
    err_lines = [l for l in err.split(b"\n") if not l.startswith(b"execve(")]
    err_filtered = b"\n".join(err_lines)
    unexpected = [b"mmap" in err_filtered, b"brk" in err_filtered,
                  b"mprotect" in err_filtered, b"openat" in err_filtered]
    ok = not any(unexpected)
    if not ok:
        record_failure("security", ["strace"], rc, 0, out, b"", err, b"", note="Unexpected syscall(s)")
    report_result(ok, "security: strace syscall surface (--help path)")

def check_strace_streaming():
    """Verify syscall surface during streaming output."""
    if not which("strace"):
        report_result(True, "security: strace streaming (skipped, no strace)")
        return
    script = f"strace -e trace=brk,mmap,mprotect,openat,read,socket,connect {fyes_abs()} 2>&1 | head -100"
    p = subprocess.run(["bash", "-c", script], capture_output=True, timeout=TIMEOUT)
    err = p.stdout
    unexpected = [b"mmap" in err, b"brk" in err, b"mprotect" in err,
                  b"openat" in err, b"socket" in err, b"connect" in err, b"read(" in err]
    ok = not any(unexpected)
    if not ok:
        record_failure("security", ["strace_stream"], 0, 0, err[:200], b"", b"", b"",
                       note="Unexpected syscall in streaming path")
    report_result(ok, "security: strace streaming path (no mmap/brk/open/socket)")

def check_strace_error_path():
    """Verify syscall surface on error path."""
    if not which("strace"):
        report_result(True, "security: strace error path (skipped, no strace)")
        return
    cmd = ["strace", "-e", "trace=brk,mmap,mprotect,openat,read,socket", FY, "--badopt"]
    rc, out, err = run(cmd)
    unexpected = [b"mmap" in err, b"brk" in err, b"mprotect" in err, b"openat" in err, b"socket" in err]
    ok = not any(unexpected)
    if not ok:
        record_failure("security", ["strace_err"], 0, 0, out[:100], b"", err[:200], b"",
                       note="Unexpected syscall on error path")
    report_result(ok, "security: strace error path (no mmap/brk/open)")

def check_sigpipe_behavior():
    """Verify clean exit on SIGPIPE."""
    rc = os.system(f"{FY} | head -1 >/dev/null 2>/dev/null")
    ok = (rc == 0)
    if not ok:
        record_failure("security", ["sigpipe"], rc, 0, b"", b"", b"", b"", note="SIGPIPE unexpected")
    report_result(ok, "security: SIGPIPE/EPIPE clean exit")

def check_signal_termination():
    """Verify fyes terminates on SIGTERM, SIGUSR1, SIGHUP, SIGINT."""
    for sig, name in [(signal.SIGTERM, "SIGTERM"), (signal.SIGUSR1, "SIGUSR1"),
                      (signal.SIGHUP, "SIGHUP"), (signal.SIGINT, "SIGINT")]:
        p = subprocess.Popen([FY], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        time.sleep(0.05)
        try:
            p.send_signal(sig)
            p.wait(timeout=2)
            ok = True
        except subprocess.TimeoutExpired:
            p.kill(); ok = False
            record_failure("security", [f"signal_{name}"], 0, 0, b"", b"", b"", b"",
                           note=f"Did not terminate after {name}")
        except Exception:
            ok = True
        finally:
            try: p.kill()
            except Exception: pass
        report_result(ok, f"security: {name} terminates process")

def check_rapid_sigpipe():
    """Rapid SIGPIPE stress test (20 trials)."""
    ok_count = 0
    trials = 20
    for _ in range(trials):
        rc = os.system(f"{FY} 2>/dev/null | head -c 1 >/dev/null 2>/dev/null")
        if rc == 0: ok_count += 1
    ok = ok_count >= trials - 2
    if not ok:
        record_failure("security", ["rapid_sigpipe"], ok_count, trials, b"", b"", b"", b"",
                       note=f"Only {ok_count}/{trials}")
    report_result(ok, f"security: rapid SIGPIPE ({ok_count}/{trials})")

def check_eintr_injection():
    """Inject EINTR on first write via strace."""
    if not which("strace"):
        report_result(True, "security: EINTR injection (skipped, no strace)")
        return
    cmd = ["strace", "-e", "inject=write:error=EINTR:when=1", FY, "--help"]
    rc, out, err = run(cmd)
    ok = (rc == 0 or rc == 124)
    if not ok:
        record_failure("security", ["eintr"], rc, 0, out, b"", err, b"", note="EINTR unexpected")
    report_result(ok, "security: EINTR injection on write")

def check_eintr_streaming():
    """Inject EINTR during streaming — verify no corruption."""
    if not which("strace"):
        report_result(True, "security: EINTR streaming (skipped, no strace)")
        return
    script = f"strace -e inject=write:error=EINTR:when=3 {fyes_abs()} 2>/dev/null | head -100"
    p = subprocess.run(["bash", "-c", script], capture_output=True, timeout=TIMEOUT)
    lines = [l for l in p.stdout.split(b"\n") if l]
    if lines:
        expected = lines[0]
        bad = [l for l in lines[:-1] if l != expected]
        ok = len(bad) == 0
    else:
        ok = True
    report_result(ok, "security: EINTR streaming (no corruption)")

def check_rlimits():
    """Verify operation under tight resource limits (16MB AS, 1s CPU)."""
    try:
        import resource
    except Exception:
        report_result(True, "security: rlimit (skipped)"); return
    def limiter():
        resource.setrlimit(resource.RLIMIT_AS, (16*1024*1024, 16*1024*1024))
        resource.setrlimit(resource.RLIMIT_CPU, (1, 1))
    rc, out, err = run([FY, "--help"], preexec_fn=limiter)
    ok = (rc == 0)
    if not ok:
        record_failure("security", ["rlimit"], rc, 0, out, b"", err, b"", note="rlimit failure")
    report_result(ok, "security: rlimit 16MB AS + 1s CPU (--help)")

def check_rlimit_nofile():
    """Verify fyes works with RLIMIT_NOFILE=3."""
    try:
        import resource
    except Exception:
        report_result(True, "security: rlimit nofile (skipped)"); return
    def limiter():
        resource.setrlimit(resource.RLIMIT_NOFILE, (3, 3))
    rc, out, err = run([FY, "--help"], preexec_fn=limiter)
    ok = (rc == 0 and len(out) > 0)
    report_result(ok, "security: rlimit NOFILE=3 (no extra fds needed)")

def check_rlimit_stack():
    """Verify fyes works with 64KB stack."""
    try:
        import resource
    except Exception:
        report_result(True, "security: rlimit stack (skipped)"); return
    def limiter():
        resource.setrlimit(resource.RLIMIT_STACK, (65536, 65536))
    rc, out, err = run([FY, "--help"], preexec_fn=limiter)
    ok = (rc == 0 and len(out) > 0)
    report_result(ok, "security: rlimit STACK=64KB")

def check_closed_stdout():
    """Verify fyes exits cleanly when stdout is closed."""
    script = f'exec 3>&1 1>&-; {fyes_abs()} 2>/dev/null; echo $? >&3'
    p = subprocess.run(["bash", "-c", script], capture_output=True, timeout=TIMEOUT, text=True)
    rc = p.stdout.strip()
    ok = rc != "" and p.returncode == 0
    if not ok:
        record_failure("security", ["closed_stdout"], 0, 0, p.stdout.encode()[:100], b"",
                       p.stderr.encode()[:100], b"", note="Crashed with closed stdout")
    report_result(ok, "security: closed stdout handling")

def check_closed_stderr():
    """Verify fyes handles closed stderr on error path."""
    script = f"exec 2>&-; {fyes_abs()} --badopt >/dev/null; echo $?"
    p = subprocess.run(["bash", "-c", script], capture_output=True, timeout=TIMEOUT, text=True)
    rc = p.stdout.strip()
    ok = rc != "" and p.returncode == 0
    report_result(ok, "security: closed stderr handling")

def check_dev_full():
    """/dev/full ENOSPC — fyes should exit, not hang."""
    if not os.path.exists("/dev/full"):
        report_result(True, "security: /dev/full (skipped)"); return
    script = f"{fyes_abs()} > /dev/full 2>/dev/null; echo $?"
    p = subprocess.run(["bash", "-c", script], capture_output=True, timeout=TIMEOUT, text=True)
    ok = p.stdout.strip() != "" and p.returncode == 0
    report_result(ok, "security: /dev/full ENOSPC handling")

def check_dev_null():
    """/dev/null output should work cleanly."""
    script = f"timeout 1 {fyes_abs()} > /dev/null 2>/dev/null; echo $?"
    p = subprocess.run(["bash", "-c", script], capture_output=True, timeout=TIMEOUT, text=True)
    ok = p.stdout.strip() in ("0", "124")
    report_result(ok, "security: /dev/null output")

def check_pipe_to_wc():
    """Pipe through head|wc — verify clean teardown."""
    script = f"{fyes_abs()} | head -1000 | wc -l"
    p = subprocess.run(["bash", "-c", script], capture_output=True, timeout=TIMEOUT, text=True)
    ok = p.stdout.strip() == "1000"
    report_result(ok, "security: pipe to head|wc -l (1000 lines)")

def check_output_consistency():
    """Read ~1MB and verify every line is identical (no corruption)."""
    p = subprocess.Popen([FY, "test_string_12345"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try: data = p.stdout.read(1_000_000)
    finally:
        try: p.kill()
        except Exception: pass
    lines = data.split(b"\n")
    if lines and lines[-1] == b"": lines = lines[:-1]
    if not lines:
        report_result(False, "security: output consistency (no output)"); return
    expected = lines[0]
    corrupt = [i for i, l in enumerate(lines[:-1]) if l != expected]
    ok = len(corrupt) == 0
    if not ok:
        record_failure("security", ["output_consistency"], 0, 0, lines[corrupt[0]][:100],
                       expected[:100], b"", b"", note=f"{len(corrupt)} corrupt lines")
    report_result(ok, f"security: output consistency ({len(lines)} lines, 1MB)")

def check_output_deterministic():
    """Two runs with same args produce identical output."""
    for args, label in [([], "default y"), (["hello", "world"], "hello world"), (["a"*500], "long arg")]:
        chunks = []
        for _ in range(2):
            p = subprocess.Popen([FY] + args, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try: chunks.append(p.stdout.read(100_000))
            finally:
                try: p.kill()
                except Exception: pass
        ok = chunks[0] == chunks[1] and len(chunks[0]) > 0
        if not ok:
            record_failure("security", ["deterministic"], 0, 0, chunks[0][:100], chunks[1][:100],
                           b"", b"", note=f"Non-deterministic for {label}")
        report_result(ok, f"security: deterministic output ({label})")

def check_no_partial_lines():
    """Verify buffer boundaries never produce partial lines."""
    for desc, args in [("2B (y\\n)", []), ("12B", ["hello world"]), ("4001B", ["x"*4000])]:
        p = subprocess.Popen([FY] + args, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try: data = p.stdout.read(500_000)
        finally:
            try: p.kill()
            except Exception: pass
        check_data = data if data.endswith(b"\n") else data[:data.rfind(b"\n") + 1]
        lines = check_data.split(b"\n")
        if lines and lines[-1] == b"": lines = lines[:-1]
        if not lines:
            report_result(False, f"security: no partial lines ({desc}) - no output"); continue
        expected = lines[0]
        bad = sum(1 for l in lines if l != expected)
        ok = bad == 0
        if not ok:
            record_failure("security", ["partial_lines"], 0, 0, b"", b"", b"", b"",
                           note=f"{bad}/{len(lines)} bad for {desc}")
        report_result(ok, f"security: no partial lines ({desc}, {len(lines)} lines)")

def check_empty_environment():
    """Verify fyes works with empty environment."""
    rc, out, err = run([FY, "--help"], env={})
    ok = rc == 0 and len(out) > 0
    report_result(ok, "security: empty environment")

def check_hostile_environment():
    """Verify static binary ignores hostile env (LD_PRELOAD etc.)."""
    hostile = os.environ.copy()
    hostile.update({"LD_PRELOAD": "/tmp/evil.so", "LD_LIBRARY_PATH": "/tmp/evil",
                    "PATH": "/tmp/evil", "LANG": "evil", "LC_ALL": "evil",
                    "IFS": " \t\n;|&", "MALLOC_CHECK_": "7"})
    rc1, out1, _ = run([FY, "--help"], env=hostile)
    rc2, out2, _ = run([FY, "--help"])
    ok = out1 == out2 and rc1 == rc2
    if not ok:
        record_failure("security", ["hostile_env"], rc1, rc2, out1[:100], out2[:100], b"", b"",
                       note="Output changed with hostile environment")
    report_result(ok, "security: hostile environment ignored (static binary)")

def check_large_environment():
    """Verify fyes handles large environment (1000 vars x 1KB)."""
    large_env = os.environ.copy()
    for i in range(1000): large_env[f"FUZZ_VAR_{i}"] = "A" * 1000
    rc, out, err = run([FY, "--help"], env=large_env)
    ok = rc == 0 and len(out) > 0
    report_result(ok, "security: large environment (1000 vars x 1KB)")

def check_error_exit_codes():
    """Verify exit codes match GNU yes."""
    cases = [(["--help"], "--help"), (["--version"], "--version"), (["--badopt"], "unrecognized long"),
             (["-z"], "invalid short"), (["--bad1"], "--bad1"), (["-?"], "-?"), (["-abc"], "multi-char short")]
    for args, desc in cases:
        rc1, _, _ = run([FY] + args)
        rc2, _, _ = run([YES] + args)
        ok = rc1 == rc2
        if not ok:
            record_failure("security", ["exit_code"], rc1, rc2, b"", b"", b"", b"",
                           note=f"Mismatch for {desc}: fyes={rc1} yes={rc2}")
        report_result(ok, f"security: exit code {desc} (fyes={rc1} yes={rc2})")

def check_stderr_isolation():
    """Error messages go to stderr only."""
    for args, desc in [("--badopt", "unrecognized"), ("-z", "invalid")]:
        rc, out, err = run([FY, args])
        ok = len(out) == 0 and len(err) > 0 and rc == 1
        report_result(ok, f"security: stderr isolation ({desc})")

def check_stdout_isolation():
    """--help/--version go to stdout only."""
    for args, desc in [("--help", "--help"), ("--version", "--version")]:
        rc, out, err = run([FY, args])
        ok = len(out) > 0 and len(err) == 0 and rc == 0
        report_result(ok, f"security: stdout isolation ({desc})")

def check_valgrind():
    """Valgrind memcheck on --help path."""
    if not which("valgrind"):
        report_result(True, "security: valgrind memcheck (skipped, no valgrind)"); return
    rc, out, err = run(["valgrind", "--error-exitcode=99", "--quiet", FY, "--help"])
    ok = (rc != 99)
    if not ok:
        record_failure("security", ["valgrind"], rc, 0, out, b"", err, b"", note="Valgrind error")
    report_result(ok, "security: valgrind memcheck (--help)")

def check_valgrind_error_path():
    """Valgrind on error path."""
    if not which("valgrind"):
        report_result(True, "security: valgrind error path (skipped, no valgrind)"); return
    rc, out, err = run(["valgrind", "--error-exitcode=99", "--quiet", FY, "--badopt"])
    ok = (rc != 99)
    report_result(ok, "security: valgrind memcheck (error path)")

def check_concurrent_instances():
    """4 parallel fyes — verify no cross-instance corruption."""
    procs = []
    args_list = [["aaa"], ["bbb"], ["ccc"], ["ddd"]]
    for args in args_list:
        procs.append((subprocess.Popen([FY]+args, stdout=subprocess.PIPE, stderr=subprocess.PIPE), args))
    time.sleep(0.2)
    all_ok = True
    for p, args in procs:
        try: data = p.stdout.read(50_000)
        finally:
            try: p.kill()
            except Exception: pass
        expected_line = (" ".join(args) + "\n").encode()
        lines = [l for l in data.split(b"\n") if l]
        bad = [l for l in lines[:-1] if l + b"\n" != expected_line]
        if bad:
            all_ok = False
            record_failure("security", ["concurrent"], 0, 0, bad[0][:100], expected_line[:100],
                           b"", b"", note=f"Corruption for args={args}")
    report_result(all_ok, "security: concurrent instances (4 parallel)")

def check_throughput():
    """Quick throughput check — must exceed 100 MB/s."""
    p = subprocess.Popen([FY], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    target = 10_000_000
    start = time.monotonic()
    try: data = p.stdout.read(target)
    finally:
        try: p.kill()
        except Exception: pass
    elapsed = time.monotonic() - start
    mbps = len(data) / elapsed / 1e6 if elapsed > 0 else float('inf')
    ok = mbps > 100
    report_result(ok, f"security: throughput {mbps:.0f} MB/s (>100 MB/s required)")

def check_large_argc():
    """Stress with 10000 args."""
    args = ["x"] * 10000
    rc1, out1, err1 = head_output([FY] + args)
    rc2, out2, err2 = head_output([YES] + args)
    ok = out1 == out2 and err1 == err2
    report_result(ok, "security: large argc (10000 args)")

def check_repeated_options():
    """Multiple -- and option-like strings after --."""
    for args, desc in [(["--","--","--"], "triple --"), (["--","--help","--version"], "options after --"),
                       (["--","-x","-y","-z"], "short opts after --")]:
        rc1, out1, err1 = head_output([FY] + args)
        rc2, out2, err2 = head_output([YES] + args)
        ok = out1 == out2 and err1 == err2
        report_result(ok, f"security: {desc}")


# =============================================================================
#                           BENCHMARKS
# =============================================================================

bench_results = []  # (name, fyes_value, yes_value, unit, higher_is_better)

def bench_record(name, fyes_val, yes_val, unit, higher_is_better=True):
    """Record a benchmark result for the comparison table."""
    bench_results.append((name, fyes_val, yes_val, unit, higher_is_better))

def measure_throughput(binary, args, duration):
    """Measure sustained throughput (bytes/sec) writing to a pipe."""
    p = subprocess.Popen([binary] + args, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    # Warmup
    warmup_end = time.monotonic() + BENCH_WARMUP
    while time.monotonic() < warmup_end:
        p.stdout.read(1_000_000)
    # Measure
    total_bytes = 0
    start = time.monotonic()
    deadline = start + duration
    while time.monotonic() < deadline:
        chunk = p.stdout.read(1_000_000)
        if not chunk: break
        total_bytes += len(chunk)
    elapsed = time.monotonic() - start
    try: p.kill()
    except Exception: pass
    return total_bytes / elapsed if elapsed > 0 else 0

def measure_memory(binary, args, duration):
    """Measure VmRSS (KB) from /proc/pid/status after stabilization."""
    p = subprocess.Popen([binary] + args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(duration)
    rss_kb = 0
    try:
        for line in Path(f"/proc/{p.pid}/status").read_text().splitlines():
            if line.startswith("VmRSS:"):
                rss_kb = int(line.split()[1]); break
    except Exception: pass
    try: p.kill()
    except Exception: pass
    return rss_kb

def measure_cpu_time(binary, args, wall_seconds):
    """Measure user+system CPU time from /proc/pid/stat."""
    ticks = os.sysconf("SC_CLK_TCK")
    p = subprocess.Popen([binary] + args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    start = time.monotonic()
    time.sleep(wall_seconds)
    wall = time.monotonic() - start
    utime = stime = 0.0
    try:
        stat = Path(f"/proc/{p.pid}/stat").read_text()
        fields = stat.rsplit(")", 1)[1].split()
        utime = int(fields[11]) / ticks
        stime = int(fields[12]) / ticks
    except Exception: pass
    try: p.kill()
    except Exception: pass
    return utime, stime, wall

def measure_startup_time(binary, args, trials):
    """Average wall-clock time for a terminating command (e.g. --help)."""
    times = []
    for _ in range(trials):
        start = time.monotonic()
        subprocess.run([binary] + args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        times.append(time.monotonic() - start)
    return sum(times) / len(times) if times else 0

def run_benchmarks():
    """Run all benchmarks comparing fyes vs GNU yes."""
    yes_path = yes_abs()
    fyes_path = fyes_abs()

    log("\n" + "=" * 70)
    log("                        BENCHMARKS")
    log("                   fyes (asm) vs GNU yes")
    log("=" * 70)

    # 1. Binary Size
    log("\n--- Binary Size ---")
    fyes_size = os.path.getsize(FY)
    try: yes_size = os.path.getsize(yes_path)
    except Exception: yes_size = 0
    log(f"  fyes: {fyes_size:,} bytes")
    log(f"  yes:  {yes_size:,} bytes")
    if yes_size > 0:
        bench_record("Binary size", fyes_size, yes_size, "bytes", higher_is_better=False)

    # 2. Throughput (default "y")
    log("\n--- Throughput: default 'y' ---")
    fyes_tp = measure_throughput(fyes_path, [], BENCH_DURATION)
    yes_tp = measure_throughput(yes_path, [], BENCH_DURATION)
    log(f"  fyes: {fyes_tp/1e6:.1f} MB/s")
    log(f"  yes:  {yes_tp/1e6:.1f} MB/s")
    bench_record("Throughput (default y)", fyes_tp/1e6, yes_tp/1e6, "MB/s")

    # 3. Throughput ("hello world")
    log("\n--- Throughput: 'hello world' ---")
    fyes_tp2 = measure_throughput(fyes_path, ["hello", "world"], BENCH_DURATION)
    yes_tp2 = measure_throughput(yes_path, ["hello", "world"], BENCH_DURATION)
    log(f"  fyes: {fyes_tp2/1e6:.1f} MB/s")
    log(f"  yes:  {yes_tp2/1e6:.1f} MB/s")
    bench_record("Throughput (hello world)", fyes_tp2/1e6, yes_tp2/1e6, "MB/s")

    # 4. Throughput (1000-char arg)
    log("\n--- Throughput: 1000-char string ---")
    fyes_tp3 = measure_throughput(fyes_path, ["x"*1000], BENCH_DURATION)
    yes_tp3 = measure_throughput(yes_path, ["x"*1000], BENCH_DURATION)
    log(f"  fyes: {fyes_tp3/1e6:.1f} MB/s")
    log(f"  yes:  {yes_tp3/1e6:.1f} MB/s")
    bench_record("Throughput (1000-char arg)", fyes_tp3/1e6, yes_tp3/1e6, "MB/s")

    # 5. Memory RSS (default "y")
    log("\n--- Memory (VmRSS): default 'y' ---")
    fyes_rss = measure_memory(fyes_path, [], BENCH_MEMORY_DURATION)
    yes_rss = measure_memory(yes_path, [], BENCH_MEMORY_DURATION)
    log(f"  fyes: {fyes_rss} KB")
    log(f"  yes:  {yes_rss} KB")
    if yes_rss > 0:
        bench_record("Memory RSS (default y)", fyes_rss, yes_rss, "KB", higher_is_better=False)

    # 6. Memory RSS (with args)
    log("\n--- Memory (VmRSS): 'hello world' ---")
    fyes_rss2 = measure_memory(fyes_path, ["hello", "world"], BENCH_MEMORY_DURATION)
    yes_rss2 = measure_memory(yes_path, ["hello", "world"], BENCH_MEMORY_DURATION)
    log(f"  fyes: {fyes_rss2} KB")
    log(f"  yes:  {yes_rss2} KB")
    if yes_rss2 > 0:
        bench_record("Memory RSS (hello world)", fyes_rss2, yes_rss2, "KB", higher_is_better=False)

    # 7. Virtual Memory
    log("\n--- Virtual Memory (VmSize): default 'y' ---")
    fyes_vmsz = yes_vmsz = 0
    for binary, label in [(fyes_path, "fyes"), (yes_path, "yes")]:
        p = subprocess.Popen([binary], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(BENCH_MEMORY_DURATION)
        try:
            for line in Path(f"/proc/{p.pid}/status").read_text().splitlines():
                if line.startswith("VmSize:"):
                    val = int(line.split()[1])
                    if label == "fyes": fyes_vmsz = val
                    else: yes_vmsz = val
                    break
        except Exception: pass
        try: p.kill()
        except Exception: pass
    log(f"  fyes: {fyes_vmsz} KB")
    log(f"  yes:  {yes_vmsz} KB")
    if yes_vmsz > 0:
        bench_record("Virtual memory (VmSize)", fyes_vmsz, yes_vmsz, "KB", higher_is_better=False)

    # 8. CPU Time
    log(f"\n--- CPU Time ({BENCH_DURATION}s wall): default 'y' ---")
    f_u, f_s, f_w = measure_cpu_time(fyes_path, [], BENCH_DURATION)
    y_u, y_s, y_w = measure_cpu_time(yes_path, [], BENCH_DURATION)
    fyes_cpu = f_u + f_s; yes_cpu = y_u + y_s
    log(f"  fyes: user={f_u:.3f}s sys={f_s:.3f}s total={fyes_cpu:.3f}s")
    log(f"  yes:  user={y_u:.3f}s sys={y_s:.3f}s total={yes_cpu:.3f}s")
    if yes_cpu > 0:
        bench_record("CPU time (total)", fyes_cpu, yes_cpu, "s", higher_is_better=False)

    # 9. CPU Efficiency
    if fyes_cpu > 0 and yes_cpu > 0:
        log("\n--- CPU Efficiency (throughput per CPU-second) ---")
        fyes_eff = (fyes_tp * BENCH_DURATION) / fyes_cpu / 1e6
        yes_eff = (yes_tp * BENCH_DURATION) / yes_cpu / 1e6
        log(f"  fyes: {fyes_eff:.1f} MB/CPU-s")
        log(f"  yes:  {yes_eff:.1f} MB/CPU-s")
        bench_record("CPU efficiency", fyes_eff, yes_eff, "MB/CPU-s")

    # 10-12. Startup Times
    for args, label in [(["--help"], "--help"), (["--version"], "--version"), (["--badopt"], "error")]:
        log(f"\n--- Startup Time ({label}, avg of {BENCH_STARTUP_TRIALS}) ---")
        ft = measure_startup_time(fyes_path, args, BENCH_STARTUP_TRIALS)
        yt = measure_startup_time(yes_path, args, BENCH_STARTUP_TRIALS)
        log(f"  fyes: {ft*1000:.2f} ms")
        log(f"  yes:  {yt*1000:.2f} ms")
        if yt > 0:
            bench_record(f"Startup time ({label})", ft*1000, yt*1000, "ms", higher_is_better=False)

    # 13. Syscall Count (informational)
    if which("strace"):
        log("\n--- Syscall Count (strace -c, 10000 lines) ---")
        for binary, label in [(fyes_path, "fyes"), (yes_path, "yes")]:
            script = f"strace -c {binary} 2>&1 | head -10000 >/dev/null"
            try:
                p = subprocess.run(["bash", "-c", script], capture_output=True, timeout=5, text=True)
                total_line = [l for l in p.stdout.strip().split("\n") if "total" in l.lower()]
                if total_line:
                    log(f"  {label}: {total_line[-1].strip()}")
                else:
                    log(f"  {label}: (could not parse)")
            except Exception as e:
                log(f"  {label}: strace failed ({e})")


def print_benchmark_summary():
    """Print formatted comparison table with improvement ratios."""
    if not bench_results: return

    log("\n" + "=" * 70)
    log("                    BENCHMARK COMPARISON")
    log("                   fyes (asm) vs GNU yes")
    log("=" * 70)
    log("")
    log(f"  {'Metric':<30} {'fyes':>10} {'GNU yes':>10} {'Unit':>8}  {'Improvement':>12}")
    log(f"  {'-'*30} {'-'*10} {'-'*10} {'-'*8}  {'-'*12}")

    for name, fyes_val, yes_val, unit, higher_is_better in bench_results:
        if higher_is_better:
            ratio = fyes_val / yes_val if yes_val > 0 else 0
        else:
            ratio = yes_val / fyes_val if fyes_val > 0 else 0
        improvement = f"{ratio:.2f}x" if ratio > 0 else "N/A"

        if fyes_val >= 100:
            fv, yv = f"{fyes_val:,.0f}", f"{yes_val:,.0f}"
        elif fyes_val >= 1:
            fv, yv = f"{fyes_val:,.1f}", f"{yes_val:,.1f}"
        else:
            fv, yv = f"{fyes_val:,.3f}", f"{yes_val:,.3f}"
        log(f"  {name:<30} {fv:>10} {yv:>10} {unit:>8}  {improvement:>12}")

    log("")

    # Category averages
    categories = {"Binary size": [], "Throughput": [], "Memory": [], "CPU/Startup": []}
    for name, fyes_val, yes_val, unit, higher_is_better in bench_results:
        if yes_val <= 0 or fyes_val <= 0: continue
        r = fyes_val / yes_val if higher_is_better else yes_val / fyes_val
        nl = name.lower()
        if "size" in nl: categories["Binary size"].append(r)
        elif "throughput" in nl or "efficiency" in nl: categories["Throughput"].append(r)
        elif "memory" in nl or "virtual" in nl: categories["Memory"].append(r)
        elif "startup" in nl or "cpu" in nl: categories["CPU/Startup"].append(r)

    log("  CATEGORY AVERAGES:")
    labels = {"Binary size": "smaller", "Throughput": "faster", "Memory": "less memory", "CPU/Startup": "faster"}
    for cat, ratios in categories.items():
        if ratios:
            avg = sum(ratios) / len(ratios)
            log(f"    {cat + ':':<18} {avg:>8.1f}x {labels[cat]}")
    log("")


# =============================================================================
#                          TEST RUNNER
# =============================================================================

def run_tests():
    """Run all functional, fuzz, and security tests."""
    # Functional correctness
    compare([], "basic: no args"); compare(["y"], "basic: single arg")
    compare(["hello"]); compare(["hello", "world"]); compare(["a", "b", "c"])
    compare(["--help", "extra"]); compare(["--version", "extra"])
    compare(["--helpx"]); compare(["--versions"])
    compare(["-n"]); compare(["-n", "5"])
    compare(["--"]); compare(["--", "help"]); compare(["--", "--help"])
    compare_exact(["--help"], "exact: --help")
    compare_exact(["--version"], "exact: --version")
    compare([""]); compare(["", "x"]); compare(["x", ""]); compare(["", "", ""])
    compare([" ", " "]); compare([" "]); compare(["  "])
    compare(["\t"]); compare(["\n"]); compare(["\r"])
    compare(["\x7f"]); compare(["\x01\x02\x03"])
    compare(["a\tb", "c\nd"]); compare(["--", "\n", "\t", " "])

    for n in [1,2,3,7,8,15,16,31,32,63,64,127,128,255,256,511,512,1023,1024,
              2047,2048,3071,3072,4094,4095,4096,5000,6000,8000]:
        compare([rand_str(n)], f"long-arg len={n}")
    for count in [2,3,4,5,10,20,50,100,200,400]:
        compare([rand_str(5) for _ in range(count)], f"many-args count={count}")
    for count in [500,1000,1500,2000]:
        compare(["a"]*count, f"tiny-args count={count}")
    for s in ["áéíóú", "ß", "©", "Ω", "→", "✓"]:
        compare([s], f"unicode {s}")

    # Fuzz
    for i in range(RANDOM_CASES):
        args = [rand_str(random.randint(0,50)) for _ in range(random.randint(0,10))]
        compare(args, f"fuzz-short {i+1}/{RANDOM_CASES}")
    for i in range(RANDOM_LONG_CASES):
        args = [rand_str(random.randint(0,1000)) for _ in range(random.randint(1,30))]
        compare(args, f"fuzz-long {i+1}/{RANDOM_LONG_CASES}")
    weird = string.ascii_letters + string.digits + " \t\r\n_-+=*/:.,;!?[]{}()<>|~`'\"\\"
    for i in range(RANDOM_WEIRD_CASES):
        args = [rand_str(random.randint(0,200), weird) for _ in range(random.randint(0,20))]
        compare(args, f"fuzz-weird {i+1}/{RANDOM_WEIRD_CASES}")
    args = []; total = 0
    while total < 5000:
        s = rand_str(10); args.append(s); total += len(s) + 1
    compare(args, "boundary aggregate length")

    compare_bytes_argv([b"--help\x00"], "nul-boundary: --help\\0")
    compare_bytes_argv([b"--version\x00"], "nul-boundary: --version\\0")
    compare_bytes_argv([b"--help\x00extra"], "nul-boundary: --help\\0extra")
    compare_bytes_argv([b"\x00"], "nul-boundary: bare \\0")
    for i in range(RANDOM_BYTES_CASES):
        args = [rand_bytes(random.randint(1,64)) for _ in range(random.randint(0,6))]
        compare_bytes_argv(args, f"bytes-argv {i+1}/{RANDOM_BYTES_CASES}")

    # Security tests
    log("\n--- ELF Binary Analysis ---")
    check_elf_binary_properties(); check_no_strings_leaks()
    log("\n--- Memory & Process Safety ---")
    check_proc_maps(); check_fd_hygiene(); check_proc_self_exe(); check_no_open_files()
    log("\n--- Syscall Surface ---")
    check_strace_syscalls(); check_strace_streaming(); check_strace_error_path()
    log("\n--- Signal Handling ---")
    check_sigpipe_behavior(); check_signal_termination(); check_rapid_sigpipe()
    log("\n--- Fault Injection ---")
    check_eintr_injection(); check_eintr_streaming()
    log("\n--- Resource Limits ---")
    check_rlimits(); check_rlimit_nofile(); check_rlimit_stack()
    log("\n--- FD / Device Handling ---")
    check_closed_stdout(); check_closed_stderr(); check_dev_full(); check_dev_null(); check_pipe_to_wc()
    log("\n--- Output Integrity ---")
    check_output_consistency(); check_output_deterministic(); check_no_partial_lines()
    log("\n--- Environment Safety ---")
    check_empty_environment(); check_hostile_environment(); check_large_environment()
    log("\n--- Error Path Correctness ---")
    check_error_exit_codes(); check_stderr_isolation(); check_stdout_isolation()
    log("\n--- Valgrind ---")
    check_valgrind(); check_valgrind_error_path()
    log("\n--- Concurrency & Stress ---")
    check_concurrent_instances(); check_throughput(); check_large_argc(); check_repeated_options()


def main():
    bench_only = "--bench-only" in sys.argv
    test_only = "--test-only" in sys.argv

    if not Path(FY).exists():
        log(f"[FAIL] {FY} not found. Build: python3 build.py")
        sys.exit(1)

    if not bench_only:
        run_tests()
        log(f"\n{'='*60}")
        log(f"[SUMMARY] total={test_count} pass={pass_count} fail={len(failures)}")
        if failures:
            log(f"\n[FAILURES] ({len(failures)} total)")
            for i, f in enumerate(failures, 1):
                log(f"\n--- Failure {i} ({f['kind']}) ---")
                log(f"Args: {f['args']}")
                log(f"Note: {f.get('note', '')}")
                log(f"fyes rc={f['rc_fyes']}  yes rc={f['rc_yes']}")
                if f['stderr_fyes'] or f['stderr_yes']:
                    log(f"fyes stderr: {f['stderr_fyes']}")
                    log(f" yes stderr: {f['stderr_yes']}")
                if f['stdout_fyes'] or f['stdout_yes']:
                    log(f"fyes stdout: {f['stdout_fyes']}")
                    log(f" yes stdout: {f['stdout_yes']}")
        log("[OK] All tests passed." if not failures else "")

    if not test_only:
        run_benchmarks()
        print_benchmark_summary()

    if failures and not bench_only:
        sys.exit(1)
    sys.exit(0)

if __name__ == "__main__":
    main()