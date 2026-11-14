//! Works out the build number: the number of commits in the repository, so every committed
//! change gets a higher number. The version shown in the app is MAJOR.MINOR.<commits>, with
//! the short commit id beside it (and a "+" when the build includes uncommitted changes).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    // Outside a git checkout (for example a source archive) the build number is 0.
    let count = git(&["rev-list", "--count", "HEAD"]).unwrap_or_else(|| "0".into());
    let mut commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    if git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty()) {
        commit.push('+');
    }
    println!("cargo:rustc-env=ROGEHA_BUILD_NUMBER={count}");
    println!("cargo:rustc-env=ROGEHA_COMMIT={commit}");

    // Run again when a commit is made or another branch is checked out.
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/logs/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
    }
}
