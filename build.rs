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
    let mut command = Command::new(&clang);
    command
        .args([
            "-O2",
            "-g",
            "-target",
            "bpf",
            "-fno-addrsig",
            "-Wall",
            "-Werror",
        ])
        .arg(format!("-D__TARGET_ARCH_{bpf_arch}"))
        .args(["-I", "bpf"]);

    for dir in multiarch_include_dirs(&target_arch) {
        if PathBuf::from(&dir).is_dir() {
            command.arg(format!("-I{dir}"));
        }
    }

    let output = command
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

    strip_dwarf(&object);

    println!("cargo:rustc-env=BPF_OBJECT={}", object.display());
}

fn strip_dwarf(object: &PathBuf) {
    let mut candidates = Vec::new();
    if let Ok(explicit) = env::var("LLVM_STRIP") {
        candidates.push(explicit);
    }
    candidates.extend(
        [
            "llvm-strip",
            "llvm-strip-18",
            "llvm-strip-17",
            "llvm-strip-16",
        ]
        .into_iter()
        .map(String::from),
    );

    for candidate in &candidates {
        match Command::new(candidate).arg("-g").arg(object).output() {
            Ok(output) if output.status.success() => return,
            Ok(output) => panic!(
                "{candidate} failed to strip {}:\n{}",
                object.display(),
                String::from_utf8_lossy(&output.stderr)
            ),
            Err(_) => continue,
        }
    }

    panic!(
        "no llvm-strip found (tried {}); DWARF sections must be removed or the loader cannot parse the object",
        candidates.join(", ")
    );
}

fn multiarch_include_dirs(target_arch: &str) -> Vec<String> {
    let triples = match target_arch {
        "x86_64" => vec!["x86_64-linux-gnu", "x86_64-linux-musl"],
        "aarch64" => vec!["aarch64-linux-gnu", "aarch64-linux-musl"],
        _ => vec![],
    };
    triples
        .into_iter()
        .map(|triple| format!("/usr/include/{triple}"))
        .collect()
}
