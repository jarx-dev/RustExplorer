//! A file's verdict from VirusTotal (API v3, the person's own key). A file is looked up by its
//! SHA-256, which sends nothing of the file itself; only [`upload`], asked for in so many words,
//! sends the file, and VirusTotal then shares it with security companies.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const API: &str = "https://www.virustotal.com/api/v3";
/// The largest file VirusTotal takes (bigger ones go to a one-off upload address over 32 MB).
pub const MAX_UPLOAD: u64 = 650 * 1024 * 1024;

/// How many of the engines that looked at the file found it malicious or suspicious.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Report {
    pub malicious: u32,
    pub suspicious: u32,
    /// engines that gave a verdict (clean, malicious or suspicious)
    pub engines: u32,
}

impl Report {
    pub fn clean(&self) -> bool {
        self.malicious == 0 && self.suspicious == 0
    }

    /// From VirusTotal's `last_analysis_stats` (a file) or `stats` (an analysis); none when no
    /// engine gave a verdict yet, which must not read as clean.
    fn from_stats(s: &serde_json::Value) -> Option<Report> {
        let n = |k: &str| s.get(k).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let r = Report { malicious: n("malicious"), suspicious: n("suspicious"), engines: n("malicious") + n("suspicious") + n("harmless") + n("undetected") };
        (r.engines > 0).then_some(r)
    }
}

/// Hex SHA-256 of the file, read in pieces (a large file never sits in memory whole).
pub fn sha256(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The file's page on VirusTotal, for the person to read the full report.
pub fn page(sha: &str) -> String {
    format!("https://www.virustotal.com/gui/file/{sha}")
}

fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(tls))
        .timeout_connect(Duration::from_secs(10))
        // per read and write, not overall: a big file on a slow line takes as long as it takes
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(120))
        .user_agent(concat!("RustExplorer/", env!("CARGO_PKG_VERSION")))
        .build())
}

/// Requests the free key takes a minute; the window is a second longer, so a minute counted on
/// VirusTotal's side never sees five.
const PER_MINUTE: usize = 4;
const MINUTE: Duration = Duration::from_secs(61);
/// The times given to the last [`PER_MINUTE`] requests of the whole program (some may be ahead).
static TURNS: Mutex<VecDeque<Instant>> = Mutex::new(VecDeque::new());

/// When the next request may go: at once, or a minute after the one [`PER_MINUTE`] back. Taken in
/// order under one lock, so files wait their turn in the order they were asked about.
fn next_turn(turns: &mut VecDeque<Instant>, now: Instant) -> Instant {
    let t = if turns.len() < PER_MINUTE { now } else { (turns[turns.len() - PER_MINUTE] + MINUTE).max(now) };
    turns.push_back(t);
    while turns.len() > PER_MINUTE {
        turns.pop_front();
    }
    t
}

/// VirusTotal's answer as JSON; its error codes in words the person can act on. Waits for its turn
/// first, telling `wait` until when (only when it has to wait).
fn call(req: ureq::Request, body: Option<(String, u64, Box<dyn Read>)>, wait: &mut dyn FnMut(Instant)) -> Result<Option<serde_json::Value>, String> {
    let t = next_turn(&mut TURNS.lock().unwrap_or_else(|e| e.into_inner()), Instant::now());
    let now = Instant::now();
    if t > now {
        wait(t);
        std::thread::sleep(t - now);
    }
    let r = match body {
        Some((kind, len, reader)) => req.set("Content-Type", &kind).set("Content-Length", &len.to_string()).send(reader),
        None => req.call(),
    };
    match r {
        Ok(resp) => serde_json::from_reader(resp.into_reader().take(4 << 20)).map(Some).map_err(|e| format!("odpowiedź VirusTotal: {e}")),
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(ureq::Error::Status(401, _)) => Err("VirusTotal nie przyjął klucza API: sprawdź go w Ustawieniach".into()),
        // the program keeps to 4 a minute itself, so this is the day's (or month's) allowance
        Err(ureq::Error::Status(429, _)) => Err("wyczerpany dzienny limit VirusTotal: sprawdź go na virustotal.com/gui/my-apikey".into()),
        Err(ureq::Error::Status(c, _)) => Err(format!("VirusTotal odpowiedział {c}")),
        Err(e) => Err(format!("brak połączenia z VirusTotal: {e}")),
    }
}

/// The verdict on a file VirusTotal already knows; `None`: it has never seen this file. `wait`
/// hears until when it waits in the queue (here and below).
pub fn lookup(key: &str, sha: &str, wait: &mut dyn FnMut(Instant)) -> Result<Option<Report>, String> {
    let json = call(agent()?.get(&format!("{API}/files/{sha}")).set("x-apikey", key), None, wait)?;
    Ok(json.and_then(|j| Report::from_stats(&j["data"]["attributes"]["last_analysis_stats"])))
}

/// Sends the file for analysis and returns the analysis id for [`analysis`]. The name sent is
/// only "plik" with the extension, so a private file name does not end up on VirusTotal.
pub fn upload(key: &str, path: &Path, wait: &mut dyn FnMut(Instant)) -> Result<String, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_UPLOAD {
        return Err("VirusTotal przyjmuje pliki do 650 MB".into());
    }
    let agent = agent()?;
    let url = if len > 32 * 1024 * 1024 {
        let j = call(agent.get(&format!("{API}/files/upload_url")).set("x-apikey", key), None, wait)?.unwrap_or_default();
        j["data"].as_str().ok_or("VirusTotal nie podał adresu do wysłania pliku")?.to_string()
    } else {
        format!("{API}/files")
    };
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let boundary = "rustexplorer-7d1f3a9c5e";
    let head = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"plik{ext}\"\r\nContent-Type: application/octet-stream\r\n\r\n");
    let tail = format!("\r\n--{boundary}--\r\n");
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let total = head.len() as u64 + len + tail.len() as u64;
    let body: Box<dyn Read> = Box::new(std::io::Cursor::new(head.into_bytes()).chain(file.take(len)).chain(std::io::Cursor::new(tail.into_bytes())));
    let j = call(agent.post(&url).set("x-apikey", key), Some((format!("multipart/form-data; boundary={boundary}"), total, body)), wait)?.unwrap_or_default();
    j["data"]["id"].as_str().map(str::to_string).ok_or_else(|| "VirusTotal nie przyjął pliku".into())
}

/// The verdict of an analysis started by [`upload`]; `None` while the engines are still at it.
pub fn analysis(key: &str, id: &str, wait: &mut dyn FnMut(Instant)) -> Result<Option<Report>, String> {
    let j = call(agent()?.get(&format!("{API}/analyses/{id}")).set("x-apikey", key), None, wait)?.unwrap_or_default();
    let a = &j["data"]["attributes"];
    Ok((a["status"] == "completed").then(|| Report::from_stats(&a["stats"])).flatten())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_count_only_the_ones_with_a_verdict() {
        let s = serde_json::json!({"malicious": 2, "suspicious": 1, "harmless": 3, "undetected": 60, "type-unsupported": 9, "timeout": 1});
        let r = Report::from_stats(&s).unwrap();
        assert_eq!(r, Report { malicious: 2, suspicious: 1, engines: 66 });
        assert!(!r.clean());
        assert!(Report::from_stats(&serde_json::Value::Null).is_none());
        assert!(Report::from_stats(&serde_json::json!({"malicious": 0, "type-unsupported": 9})).is_none());
    }

    #[test]
    fn the_fifth_request_waits_a_minute_after_the_first() {
        let (mut q, t0) = (VecDeque::new(), Instant::now());
        for _ in 0..4 {
            assert_eq!(next_turn(&mut q, t0), t0);
        }
        assert_eq!(next_turn(&mut q, t0), t0 + MINUTE);
        assert_eq!(next_turn(&mut q, t0), t0 + MINUTE);
        // later ones queue behind the ones already waiting, in order
        for _ in 0..2 {
            next_turn(&mut q, t0);
        }
        assert_eq!(next_turn(&mut q, t0), t0 + MINUTE * 2);
        // after a quiet spell a request goes at once
        assert_eq!(next_turn(&mut q, t0 + MINUTE * 4), t0 + MINUTE * 4);
    }

    #[test]
    fn the_hash_is_the_files_sha256() {
        let p = std::env::temp_dir().join(format!("rx-scan-{}.txt", std::process::id()));
        std::fs::write(&p, b"abc").unwrap();
        let h = sha256(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert_eq!(h, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
