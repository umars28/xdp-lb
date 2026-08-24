use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=bpf/xdp_lb.c");
    println!("cargo:rerun-if-changed=bpf/xdp_lb.h");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let object = out_dir.join("xdp_lb.o");
    let source = "bpf/xdp_lb.c";

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH not set");
    let bpf_arch = match target_arch.as_str() {
        "x86_64" => "x86",
        "aarch64" => "arm64",
        other => panic!("unsupported target arch for BPF: {other}"),
    };

    let clang = env::var("CLANG").unwrap_or_else(|_| "clang".to_string());
    let output = Command::new(&clang)
        .args(["-O2", "-g", "-target", "bpf", "-Wall", "-Werror"])
        .arg(format!("-D__TARGET_ARCH_{bpf_arch}"))
        .args(["-I", "bpf"])
        .arg("-c")
        .arg(source)
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {clang}: {e}"));

    if !output.status.success() {
        panic!(
            "clang failed to build {source}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    println!("cargo:rustc-env=BPF_OBJECT={}", object.display());
}
