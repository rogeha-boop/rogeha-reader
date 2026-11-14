//! Finds the built reader and assistant so the installer can carry them inside itself.
//! Build the reader first: `cargo build --release`, then `cargo build --release -p rogeha-setup`.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    // <workspace>/target/<profile>/build/rogeha-setup-<hash>/out  ->  <workspace>/target/<profile>
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let profile_dir = out.ancestors().nth(3).expect("target folder").to_path_buf();
    for (var, name) in [("ROGEHA_READER_EXE", "rogeha-reader.exe"), ("ROGEHA_AGENT_EXE", "rogeha-agent.exe")] {
        let path = profile_dir.join(name);
        if !path.exists() {
            panic!("{} is missing. Build the reader first: cargo build --release", path.display());
        }
        println!("cargo:rustc-env={var}={}", path.display());
        println!("cargo:rerun-if-changed={}", path.display());
    }

    // The same build number the reader shows (the number of commits).
    let count = Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "0".into());
    println!("cargo:rustc-env=ROGEHA_BUILD_NUMBER={count}");
}
