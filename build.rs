use chrono::Utc;
use memory_serve::load_directory;
use std::process::Command;

fn main() {
    link_cpp_runtime();
    load_directory("admin-ui/dist");

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=Cargo.toml");
    println!("cargo::rerun-if-changed=.git/HEAD");
    println!("cargo::rerun-if-changed=.git/refs/heads/");

    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let build_time = Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();

    println!("cargo:rustc-env=GIT_HASH={git_hash}");
    println!("cargo:rustc-env=BUILD_TIME={build_time}");
}

// Include the C++ runtime in GNU/Linux binaries rather than requiring a shared
// libstdc++ on the runtime host (in particular, unmanaged NixOS processes).
fn link_cpp_runtime() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu")
    {
        return;
    }
    let output = cc::Build::new()
        .cpp(true)
        .get_compiler()
        .to_command()
        .arg("-print-file-name=libstdc++.a")
        .output()
        .expect("C++ compiler must resolve its static runtime");
    assert!(output.status.success(), "C++ runtime lookup failed");
    let path = String::from_utf8(output.stdout).expect("C++ runtime path must be UTF-8");
    let path = std::path::Path::new(path.trim());
    assert!(
        path.is_file(),
        "Install the static libstdc++ development archive"
    );
    let directory = path
        .parent()
        .expect("C++ runtime must have a parent directory");
    println!("cargo:rustc-link-search=native={}", directory.display());
    println!("cargo:rustc-link-lib=static=stdc++");
}
