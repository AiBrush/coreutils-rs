use std::env;
use std::process::Command;

fn main() {
    // Re-run the build script if the assembly source changes.
    println!("cargo:rerun-if-changed=assembly/yes/fyes.asm");
    println!("cargo:rerun-if-changed=assembly/yes/build.py");
    println!("cargo:rerun-if-changed=assembly/yes/fyes_arm64.s");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // Assembly build only applies to Linux.
    if target_os != "linux" {
        return;
    }

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = env::var("OUT_DIR").unwrap();

    match target_arch.as_str() {
        "x86_64" => build_x86_64(&manifest_dir, &out_dir),
        "aarch64" => build_aarch64(&manifest_dir, &out_dir),
        _ => {}
    }
}

fn build_x86_64(manifest_dir: &str, out_dir: &str) {
    // Check for required tools.
    let has_nasm = Command::new("nasm")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let has_python = Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !has_nasm || !has_python {
        println!(
            "cargo:warning=fyes assembly (x86_64): nasm or python3 not found — using Rust fallback"
        );
        return;
    }

    // Build the assembly binary into OUT_DIR.
    let asm_dir = format!("{}/assembly/yes", manifest_dir);
    let fyes_asm_out = format!("{}/fyes_asm", out_dir);

    let status = Command::new("python3")
        .arg("build.py")
        .arg("-o")
        .arg(&fyes_asm_out)
        .arg("--no-verify")
        .current_dir(&asm_dir)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!(
                "cargo:warning=fyes assembly (x86_64): built at {}",
                fyes_asm_out
            );
            println!("cargo:rustc-env=FYES_ASM_PATH={}", fyes_asm_out);
            println!("cargo:rustc-cfg=fyes_has_asm");
        }
        _ => {
            println!("cargo:warning=fyes assembly (x86_64): build failed — using Rust fallback");
        }
    }
}

fn build_aarch64(manifest_dir: &str, out_dir: &str) {
    // Check for required tools: GNU assembler (as) and linker (ld).
    let has_as = Command::new("as")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let has_ld = Command::new("ld")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !has_as || !has_ld {
        println!(
            "cargo:warning=fyes assembly (aarch64): 'as' or 'ld' not found — using Rust fallback"
        );
        return;
    }

    let asm_src = format!("{}/assembly/yes/fyes_arm64.s", manifest_dir);
    let obj_out = format!("{}/fyes_arm64.o", out_dir);
    let fyes_asm_out = format!("{}/fyes_asm", out_dir);

    // Assemble.
    let as_status = Command::new("as").args(["-o", &obj_out, &asm_src]).status();

    if !matches!(as_status, Ok(s) if s.success()) {
        println!("cargo:warning=fyes assembly (aarch64): 'as' failed — using Rust fallback");
        return;
    }

    // Link.
    let ld_status = Command::new("ld")
        .args([
            "-static",
            "-s",
            "-e",
            "_start",
            "-o",
            &fyes_asm_out,
            &obj_out,
        ])
        .status();

    match ld_status {
        Ok(s) if s.success() => {
            println!(
                "cargo:warning=fyes assembly (aarch64): built at {}",
                fyes_asm_out
            );
            println!("cargo:rustc-env=FYES_ASM_PATH={}", fyes_asm_out);
            println!("cargo:rustc-cfg=fyes_has_asm");
        }
        _ => {
            println!("cargo:warning=fyes assembly (aarch64): 'ld' failed — using Rust fallback");
        }
    }
}
