//! Embed the git commit for `/version`. Falls back to "unknown" outside a git checkout
//! (e.g. container builds that copy sources) unless APP_GIT_SHA is provided.
use std::process::Command;

fn main() {
    let sha = std::env::var("APP_GIT_SHA").ok().filter(|s| !s.is_empty()).or_else(|| {
        Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    });
    println!("cargo:rustc-env=APP_GIT_SHA={}", sha.unwrap_or_else(|| "unknown".into()));
    println!("cargo:rerun-if-env-changed=APP_GIT_SHA");
    println!("cargo:rerun-if-changed=../../../.git/HEAD");
}
