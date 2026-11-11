//! The build's identity for the self-updater (src/lib.rs). scripts/publish.sh sets both
//! variables, so the EXE and its latest.json agree; other builds use the git commit and the
//! current time (and don't update themselves).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RUSTEXPLORER_BUILD");
    println!("cargo:rerun-if-env-changed=RUSTEXPLORER_BUILT_AT");
    println!("cargo:rerun-if-env-changed=RUSTEXPLORER_UPDATE_URL");
    let built_at = std::env::var("RUSTEXPLORER_BUILT_AT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    let published = std::env::var("RUSTEXPLORER_BUILD").ok().filter(|v| !v.trim().is_empty());
    println!("cargo:rustc-env=RUSTEXPLORER_PUBLISHED={}", if published.is_some() { "1" } else { "" });
    let build = published.unwrap_or_else(|| {
        let sha = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        format!("{}-{built_at}", sha.unwrap_or_else(|| "local".into()))
    });
    println!("cargo:rustc-env=RUSTEXPLORER_BUILD={build}");
    println!("cargo:rustc-env=RUSTEXPLORER_BUILT_AT={built_at}");
}
