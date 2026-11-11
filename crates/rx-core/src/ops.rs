//! The Operacje panel's model: file operations running off the UI thread, their speed over their
//! progress and the Polish texts shown for them. Workers report through [`Ops`]; the window only
//! reads a copy ([`Ops::list`]), so neither waits on the other for long.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::fmt_size;

/// How often a running job takes a speed sample; also the most a worker asks for a repaint.
pub const SAMPLE_EVERY: Duration = Duration::from_millis(200);
/// How long a finished job stays in the status bar and the panel.
pub const KEEP_DONE: Duration = Duration::from_secs(5);
/// A job done sooner leaves at once, with no confirmation (a delete of a few files).
const QUICK: Duration = Duration::from_secs(1);
/// Samples kept per job; past that every other one goes, so the chart still spans the whole run.
const MAX_SAMPLES: usize = 240;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Running,
    /// the outcome note; empty when the operation has none (a shell copy): the title stands in
    Done(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    /// what it does and where to ("Kopiowanie do Dokumenty")
    pub title: String,
    /// what it works on ("Nagranie.mp4", "a.txt (+4)")
    pub source: String,
    pub done: u64,
    /// None while the operation does not say how much there is: the bar is indeterminate
    pub total: Option<u64>,
    pub start: Instant,
    pub end: Option<Instant>,
    /// bytes per second, smoothed
    pub speed: f64,
    /// (fraction done, bytes per second) at each sample: the panel's chart
    pub samples: Vec<(f32, f32)>,
    pub state: State,
    /// waits in a queue until then (VirusTotal's 4 requests a minute)
    pub queued: Option<Instant>,
    /// the last sample's time and count
    last: (Instant, u64),
}

impl Job {
    fn new(id: u64, title: String, source: String, now: Instant) -> Self {
        Job { id, title, source, done: 0, total: None, start: now, end: None, speed: 0.0, samples: vec![], state: State::Running, queued: None, last: (now, 0) }
    }

    /// Takes a count; true when a sample was taken (worth a repaint, at most every SAMPLE_EVERY).
    fn progress(&mut self, done: u64, total: u64, now: Instant) -> bool {
        let total = (total > 0).then_some(total);
        // a job of two steps (a copy out of an archive, then its unpacking) counts from 0 again
        // and a size learnt after counts came in without one starts the samples there, or the
        // bytes so far would read as one burst
        let late = self.total.is_none() && total.is_some() && self.last.0 > self.start;
        if late || done < self.last.1 || (self.total.is_some() && total != self.total) {
            (self.samples, self.speed, self.last) = (vec![], 0.0, (now, done));
        }
        (self.done, self.total) = (done, total);
        let dt = now.duration_since(self.last.0).as_secs_f64();
        if dt < SAMPLE_EVERY.as_secs_f64() {
            return false;
        }
        let v = (done - self.last.1) as f64 / dt;
        self.speed = if self.speed == 0.0 { v } else { self.speed + (v - self.speed) * (dt * 2.0).min(1.0) };
        if let Some(f) = self.fraction() {
            self.samples.push((f, v as f32));
            if self.samples.len() > MAX_SAMPLES {
                let mut k = 0;
                self.samples.retain(|_| {
                    k += 1;
                    k % 2 == 1
                });
            }
        }
        self.last = (now, done);
        true
    }

    pub fn fraction(&self) -> Option<f32> {
        self.total.map(|t| (self.done as f64 / t as f64).min(1.0) as f32)
    }

    pub fn running(&self) -> bool {
        self.state == State::Running
    }

    /// Seconds to the end at the present speed; None before the first sample or with no total.
    pub fn remaining(&self) -> Option<f64> {
        let t = self.total?;
        (self.speed > 0.0).then(|| t.saturating_sub(self.done) as f64 / self.speed)
    }

    /// How long it still waits in its queue; None once its turn came.
    pub fn waiting(&self, now: Instant) -> Option<Duration> {
        self.queued.filter(|&t| self.running() && t > now).map(|t| t - now)
    }

    /// "1,20 GB z 1,90 GB · zostało 1 min 15 s", or what of it is known; "W kolejce: start za 40 s"
    /// while it waits its turn.
    pub fn status(&self) -> String {
        if let Some(w) = self.waiting(Instant::now()) {
            return format!("W kolejce: start za {}", fmt_secs(w.as_secs_f64().ceil() as u64));
        }
        let size = match self.total {
            Some(t) => format!("{} z {}", fmt_size(self.done), fmt_size(t)),
            None if self.done > 0 => fmt_size(self.done),
            None => String::new(),
        };
        let eta = self.remaining().map(fmt_eta).unwrap_or_default();
        [size, eta].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
    }

    /// A finished job's line under its note: "1,90 GB · 17 s · śr. 112 MB/s", or only the time
    /// when nothing was counted.
    pub fn summary(&self) -> String {
        let secs = self.end.unwrap_or(self.last.0).duration_since(self.start).as_secs_f64();
        let took = fmt_secs(secs.round() as u64);
        if self.done == 0 {
            return took;
        }
        format!("{} · {took} · śr. {}", fmt_size(self.done), fmt_speed(self.done as f64 / secs.max(0.1)))
    }

    /// What the job ended with, for the bar and the panel: its note or, without one, its title
    /// ("Zakończono: Przenoszenie do Kosza").
    pub fn outcome(&self) -> String {
        match &self.state {
            State::Done(n) | State::Failed(n) if !n.is_empty() => n.clone(),
            State::Done(_) => format!("Zakończono: {}", self.title),
            _ => self.title.clone(),
        }
    }
}

/// The running and recently finished operations, shared by the workers and the window.
#[derive(Default)]
pub struct Ops(Mutex<(u64, Vec<Job>)>);

impl Ops {
    fn with<R>(&self, f: impl FnOnce(&mut (u64, Vec<Job>)) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn job<R>(&self, id: u64, f: impl FnOnce(&mut Job) -> R) -> Option<R> {
        self.with(|(_, jobs)| jobs.iter_mut().find(|j| j.id == id).map(f))
    }

    pub fn start(&self, title: String, source: String) -> u64 {
        self.with(|(next, jobs)| {
            *next += 1;
            jobs.push(Job::new(*next, title, source, Instant::now()));
            *next
        })
    }

    /// A worker's count of `done` out of `total` (0: not known); true when the window should
    /// repaint (at most every SAMPLE_EVERY).
    pub fn progress(&self, id: u64, done: u64, total: u64) -> bool {
        self.job(id, |j| j.progress(done, total, Instant::now())).unwrap_or(false)
    }

    /// The job waits in a queue until `until` (it runs on by itself after).
    pub fn queue(&self, id: u64, until: Instant) {
        self.job(id, |j| j.queued = Some(until));
    }

    /// Ok(note) ends it done (shown for KEEP_DONE), Err(text) failed (kept until dismissed).
    pub fn finish(&self, id: u64, outcome: Result<String, String>) {
        self.finish_at(id, outcome, Instant::now());
    }

    fn finish_at(&self, id: u64, outcome: Result<String, String>, now: Instant) {
        // a job done within QUICK was seen through: no 5 s "✓ 4 kB · 0 s" after it (its note still shows)
        if outcome.is_ok() && self.job(id, |j| now.duration_since(j.start) < QUICK).unwrap_or(false) {
            return self.remove(id);
        }
        self.job(id, |j| {
            j.end = Some(now);
            if outcome.is_ok() {
                if let Some(t) = j.total {
                    j.done = t;
                }
            }
            j.state = outcome.map_or_else(State::Failed, State::Done);
        });
    }

    /// Gone without a trace: a job that ended in a question or was cancelled, or a failure the
    /// user closed.
    pub fn remove(&self, id: u64) {
        self.with(|(_, jobs)| jobs.retain(|j| j.id != id));
    }

    /// A copy for drawing; done jobs older than KEEP_DONE are dropped here. The chart's samples
    /// come only `with_samples` (the panel is open).
    pub fn list(&self, now: Instant, with_samples: bool) -> Vec<Job> {
        self.with(|(_, jobs)| {
            jobs.retain(|j| !matches!(j.state, State::Done(_)) || j.end.is_none_or(|e| now.duration_since(e) < KEEP_DONE));
            jobs.iter_mut()
                .map(|j| {
                    // taken out for the clone, so they are copied only when asked for
                    let s = std::mem::take(&mut j.samples);
                    let mut c = j.clone();
                    if with_samples {
                        c.samples = s.clone();
                    }
                    j.samples = s;
                    c
                })
                .collect()
        })
    }
}

/// The share done over the running jobs that know their size; None when none does.
pub fn overall(jobs: &[Job]) -> Option<f32> {
    let (done, total) = jobs.iter().filter(|j| j.running()).filter_map(|j| Some((j.done.min(j.total?), j.total?))).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    (total > 0).then(|| (done as f64 / total as f64) as f32)
}

/// "64 %": rounded down, so 100 % shows only at the end; a non-breaking space before the sign.
pub fn fmt_pct(f: f32) -> String {
    format!("{}\u{a0}%", (f.clamp(0.0, 1.0) * 100.0).floor() as u32)
}

pub fn fmt_speed(bytes_per_s: f64) -> String {
    format!("{}/s", fmt_size(bytes_per_s as u64))
}

/// "45 s", "1 min 15 s", "2 h 5 min", with non-breaking spaces before the units.
pub fn fmt_secs(s: u64) -> String {
    match s {
        0..60 => format!("{s}\u{a0}s"),
        60..3600 => format!("{}\u{a0}min {}\u{a0}s", s / 60, s % 60),
        _ => format!("{}\u{a0}h {}\u{a0}min", s / 3600, s % 3600 / 60),
    }
}

/// "zostało 1 min 15 s"; under a second "kończę…".
pub fn fmt_eta(secs: f64) -> String {
    if secs < 1.0 { "kończę…".into() } else { format!("zostało {}", fmt_secs(secs.ceil() as u64)) }
}

/// "1 operacja", "3 operacje", "5 operacji".
pub fn fmt_count(n: usize) -> String {
    let word = match (n % 10, n % 100) {
        _ if n == 1 => "operacja",
        (2..=4, r) if !(12..=14).contains(&r) => "operacje",
        _ => "operacji",
    };
    format!("{n} {word}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_speed_and_restart() {
        let t0 = Instant::now();
        let mut j = Job::new(1, "Kopiowanie do X".into(), "a".into(), t0);
        // too soon for a sample: no repaint asked
        assert!(!j.progress(10, 1000, t0 + Duration::from_millis(50)));
        assert_eq!(j.fraction(), Some(0.01));
        assert!(j.progress(200, 1000, t0 + Duration::from_millis(200)));
        assert_eq!(j.speed, 1000.0);
        assert_eq!(j.samples, vec![(0.2, 1000.0)]);
        // 800 left at 1000 B/s
        assert_eq!(j.remaining(), Some(0.8));
        assert_eq!(j.status(), "200 B z 1\u{a0}000 B · kończę…");
        // a second step starts its count again: old samples go
        j.progress(5, 50, t0 + Duration::from_millis(300));
        assert!(j.samples.is_empty() && j.speed == 0.0 && j.total == Some(50));
        // a long run keeps a bounded number of samples
        for k in 1..2000u64 {
            j.progress(k, 2000, t0 + Duration::from_millis(300 + 200 * k));
        }
        assert!(j.samples.len() <= MAX_SAMPLES && j.samples[0].0 < 0.01);
        // counts with no size first (a copy still sizing its sources): the size arriving starts
        // the samples there, not one burst of everything copied so far
        let mut j = Job::new(2, String::new(), String::new(), t0);
        j.progress(0, 0, t0 + Duration::from_millis(400));
        j.progress(900, 1000, t0 + Duration::from_millis(500));
        assert!(j.progress(950, 1000, t0 + Duration::from_millis(700)));
        assert_eq!(j.speed, 250.0);
    }

    #[test]
    fn unknown_total_and_registry() {
        let ops = Ops::default();
        let (a, b) = (ops.start("Kopiowanie do Y".into(), String::new()), ops.start("Wypakowywanie do Z".into(), "z.7z".into()));
        ops.progress(a, 0, 0);
        ops.progress(b, 300, 1000);
        let l = ops.list(Instant::now(), false);
        assert_eq!(l[0].fraction(), None);
        // the job with no size does not count in the total share
        assert_eq!(overall(&l), Some(0.3));
        // a quick success leaves at once, with no confirmation
        let c = ops.start("Przenoszenie do Kosza".into(), String::new());
        ops.finish(c, Ok(String::new()));
        assert_eq!(ops.list(Instant::now(), false).len(), 2);
        let later = Instant::now() + QUICK;
        ops.finish_at(b, Err("Błąd archiwum: uszkodzony wpis".into()), later);
        ops.finish_at(a, Ok(String::new()), later);
        assert_eq!(overall(&ops.list(later, false)), None);
        let l = ops.list(later, false);
        assert_eq!((l[0].outcome(), l[1].outcome()), ("Zakończono: Kopiowanie do Y".into(), "Błąd archiwum: uszkodzony wpis".into()));
        // done jobs leave after a while; a failure stays until removed
        let l = ops.list(later + KEEP_DONE, false);
        assert_eq!(l.len(), 1);
        ops.remove(b);
        assert!(ops.list(later, false).is_empty());
    }

    #[test]
    fn samples_only_when_asked() {
        let ops = Ops::default();
        let a = ops.start(String::new(), String::new());
        let t0 = Instant::now();
        ops.job(a, |j| j.progress(500, 1000, t0 + SAMPLE_EVERY));
        assert!(ops.list(t0, false)[0].samples.is_empty());
        assert_eq!(ops.list(t0, true)[0].samples.len(), 1);
        // and kept after a list without them
        assert_eq!(ops.list(t0, true)[0].samples.len(), 1);
    }

    #[test]
    fn polish_texts() {
        assert_eq!(fmt_eta(75.0), "zostało 1\u{a0}min 15\u{a0}s");
        assert_eq!(fmt_eta(0.4), "kończę…");
        assert_eq!(fmt_secs(7300), "2\u{a0}h 1\u{a0}min");
        assert_eq!(fmt_pct(0.999), "99\u{a0}%");
        assert_eq!(fmt_speed(112.0 * 1024.0 * 1024.0), "112 MB/s");
        assert_eq!([1, 2, 5, 12, 22, 25].map(fmt_count), ["1 operacja", "2 operacje", "5 operacji", "12 operacji", "22 operacje", "25 operacji"]);
        let t0 = Instant::now();
        let mut j = Job::new(1, String::new(), String::new(), t0);
        j.progress(2 * 1024 * 1024 * 1024, 2 * 1024 * 1024 * 1024, t0 + Duration::from_secs(16));
        j.end = Some(t0 + Duration::from_secs(16));
        assert_eq!(j.summary(), "2 GB · 16\u{a0}s · śr. 128 MB/s");
    }
}
