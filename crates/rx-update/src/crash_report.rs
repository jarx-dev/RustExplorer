//! Crash reports: after the first panic of a process, one small anonymous report goes to the
//! author's own Rustrak server (errors.jarx.pl, Sentry protocol). It holds exactly three facts:
//! - the build number (`rustexplorer@26.1104.2135`: the build time in UTC),
//! - the Windows version (`10.0.26100`),
//! - the place of the panic in the source code (`src/main.rs:1234`; for a library's code the
//!   path is cut to `ureq-2.12.1/src/pool.rs`, else to the file name alone, so no folder of the
//!   building machine goes out).
//! The request also carries the app's name and that version (User-Agent, X-Sentry-Auth) and the
//! project's public key. Never sent: the panic's message (it can hold a file name or text), file
//! names or paths of the user, the computer's or user's name, settings, logs, environment, any id
//! of the installation or the user. Only release builds send, at most once per process; the hook
//! waits at most 3 s and drops any error. Switched off in File ▸ Settings ▸ "Send crash reports".

use std::hash::{BuildHasher, Hasher};
use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const PROJECT: u32 = 9;
/// The ingest key is public by design (any Sentry client embeds it); the server limits its rate.
const KEY: &str = "2e31d321d0d749a2abc9b45397bb3f23";
const SLUG: &str = "rustexplorer";
const WAIT: Duration = Duration::from_secs(3);

/// Off until the app has read the stored setting (see main), so a stored "off" holds from the start.
static ENABLED: AtomicBool = AtomicBool::new(false);
static SENT: AtomicBool = AtomicBool::new(false);

/// The setting as the app last set it.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Called from the app's panic hook. Waits for at most 3 s (also on a DNS that does not answer:
/// the request runs on a thread of its own) and never panics.
pub fn report(info: &PanicHookInfo) {
    if cfg!(debug_assertions) || !enabled() || SENT.swap(true, Ordering::Relaxed) {
        return;
    }
    let (file, line) = info.location().map(|l| (l.file(), l.line())).unwrap_or(("unknown", 0));
    let version = utc_version(crate::built_at());
    let body = envelope(&event_id(), &version, &platform::windows_version(), file, line);
    let (tx, rx) = std::sync::mpsc::channel();
    let sending = std::thread::Builder::new().name("crash-report".into()).spawn(move || {
        send(&version, &body);
        let _ = tx.send(());
    });
    if sending.is_ok() {
        let _ = rx.recv_timeout(WAIT);
    }
}

fn send(version: &str, body: &str) {
    let Ok(tls) = native_tls::TlsConnector::new() else { return };
    let agent = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(tls))
        .timeout_connect(Duration::from_secs(2))
        .timeout(WAIT)
        .https_only(true)
        .redirects(0)
        // Cloudflare refuses a library's default User-Agent
        .user_agent(&format!("{SLUG}/{version}"))
        .build();
    let _ = agent
        .post(&format!("https://errors.jarx.pl/api/{PROJECT}/envelope/"))
        .set("Content-Type", "application/x-sentry-envelope")
        .set("X-Sentry-Auth", &format!("Sentry sentry_version=7, sentry_key={KEY}, sentry_client={SLUG}/{version}"))
        .send_string(body);
}

/// The build time as "26.1104.2135" in UTC: the number the window shows is in the local time
/// zone, which would tell the user's zone and give one build several releases.
fn utc_version(built_at: u64) -> String {
    chrono::DateTime::from_timestamp(built_at as i64, 0).map(|t| t.format("%y.%m%d.%H%M").to_string()).unwrap_or_default()
}

/// 128 random bits from the standard library's per-process random keys (no extra dependency).
fn event_id() -> String {
    let half = || std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", half(), half())
}

/// The place in the code without the building machine's folders: a path inside the workspace
/// (`src/main.rs`) stays; one in the cargo registry or a git checkout keeps the crate's folder
/// (`ureq-2.12.1/src/pool.rs`), one in the toolchain the part after `library/`
/// (`core/src/option.rs`); anything else only the file name (`C:/Users/<name>/x.rs` -> `x.rs`).
fn short_path(file: &str) -> String {
    let file = file.replace('\\', "/");
    let absolute = file.starts_with('/') || file.as_bytes().get(1) == Some(&b':');
    if !absolute && !file.split('/').any(|p| p == "..") {
        return file;
    }
    let name = file.rsplit('/').next().unwrap_or_default().to_string();
    // registry/src/<index>/<crate>-<version>/…, git/checkouts/<repo>/<hex rev>/…: the part after
    // <index>/<repo>, only when the next folder looks like one (else it could be anyone's folder)
    let crate_dir = |d: &str| d.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b)) && d.as_bytes().windows(2).skip(1).any(|w| w[0] == b'-' && w[1].is_ascii_digit());
    let rev = |d: &str| d.len() >= 7 && d.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    for (marker, looks_right) in [("/registry/src/", &crate_dir as &dyn Fn(&str) -> bool), ("/git/checkouts/", &rev)] {
        if let Some(i) = file.rfind(marker) {
            return match file[i + marker.len()..].split_once('/') {
                Some((_, rest)) if rest.split('/').next().is_some_and(looks_right) && rest.contains('/') => rest.to_string(),
                _ => name,
            };
        }
    }
    if let Some(i) = file.find("/rustc/") {
        if let Some(j) = file[i..].find("/library/") {
            return file[i + j + "/library/".len()..].to_string();
        }
    }
    name
}

/// The whole envelope; takes only what is sent, so the panic's message cannot get in.
fn envelope(event_id: &str, version: &str, os_version: &str, file: &str, line: u32) -> String {
    let place = format!("{}:{line}", short_path(file));
    let event = serde_json::json!({
        "event_id": event_id,
        "level": "fatal",
        "platform": "native",
        "release": format!("{SLUG}@{version}"),
        "environment": "production",
        "contexts": {"os": {"name": "Windows", "version": os_version}},
        "exception": {"values": [{"type": "panic", "value": place, "mechanism": {"type": "panic", "handled": false}}]},
        "fingerprint": [place],
    });
    format!("{}\n{}\n{event}\n", serde_json::json!({"event_id": event_id}), serde_json::json!({"type": "event"}))
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};

    /// "10.0.26100"; the real numbers because the app's manifest names Windows 10/11.
    pub fn windows_version() -> String {
        // SAFETY: a zeroed plain C struct with its size filled in, as the call requires
        let mut v: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
        v.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        if unsafe { GetVersionExW(&mut v) } == 0 {
            return String::new();
        }
        format!("{}.{}.{}", v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber)
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn windows_version() -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_lose_the_building_machine() {
        for (from, to) in [
            ("src/main.rs", "src/main.rs"),
            ("crates/rx-core/src/copy.rs", "crates/rx-core/src/copy.rs"),
            (r"C:\Users\someone\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\ureq-2.12.1\src\pool.rs", "ureq-2.12.1/src/pool.rs"),
            ("/home/someone/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-2.12.1/src/pool.rs", "ureq-2.12.1/src/pool.rs"),
            ("/home/someone/.cargo/git/checkouts/egui-1a2b3c/4d5e6f7/crates/egui/src/ui.rs", "4d5e6f7/crates/egui/src/ui.rs"),
            ("/rustc/abc123/library/core/src/option.rs", "core/src/option.rs"),
            ("/home/someone/target/release/build/x-1/out/gen.rs", "gen.rs"),
            ("/home/demo/src/gen.rs", "gen.rs"),
            (r"C:\Users\someone\x.rs", "x.rs"),
            ("../../x/src/a.rs", "a.rs"),
            (r"\\?\UNC\buildhost\registry\src\agents\alice\work\main.rs", "main.rs"),
            ("D:/registry/src/Users/jan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-2.12.1/src/pool.rs", "ureq-2.12.1/src/pool.rs"),
            ("/home/someone/.cargo/git/checkouts/egui-1a2b3c/main/crates/egui/src/ui.rs", "ui.rs"),
        ] {
            assert_eq!(short_path(from), to, "{from}");
        }
    }

    #[test]
    fn the_envelope_holds_only_version_os_and_place() {
        let (file, line) = (file!(), line!());
        let body = envelope("0123456789abcdef0123456789abcdef", &utc_version(1_762_270_500), "10.0.26100", file, line);
        assert!(body.ends_with('\n'));
        let lines: Vec<serde_json::Value> = body.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], serde_json::json!({"event_id": "0123456789abcdef0123456789abcdef"}));
        assert_eq!(lines[1], serde_json::json!({"type": "event"}));
        let place = format!("crates/rx-update/src/crash_report.rs:{line}");
        assert_eq!(
            lines[2],
            serde_json::json!({
                "event_id": "0123456789abcdef0123456789abcdef",
                "level": "fatal",
                "platform": "native",
                "release": "rustexplorer@25.1104.1535",
                "environment": "production",
                "contexts": {"os": {"name": "Windows", "version": "10.0.26100"}},
                "exception": {"values": [{"type": "panic", "value": place, "mechanism": {"type": "panic", "handled": false}}]},
                "fingerprint": [place],
            })
        );
    }
}
