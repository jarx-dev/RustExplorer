//! RustExplorer rules that need no window and no Windows API: sizes, dates, attributes, natural
//! sorting, the name filter and file kinds, plus the app icon. They build and test on any OS
//! (`cargo test -p rx-core`), with no Windows machine needed.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

// first: t! is in scope for the modules after it
#[macro_use]
pub mod i18n;
pub mod icon;
pub mod index;
pub mod ops;
pub mod palette;
pub mod treemap;
pub mod undo;

pub const APP_NAME: &str = "RustExplorer";

// ---------- file kinds ----------
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Folder,
    Zip,
    Sheet,
    Data,
    Doc,
    Slides,
    Pdf,
    Image,
    Audio,
    Video,
    Exe,
    Text,
    Markdown,
    Code,
    Script,
    Backup,
    File,
    Drive,
    NetDrive,
    Computer,
    /// only the Pulpit place in the tree; no file is of this kind
    Desktop,
    /// only the Pobrane place
    Downloads,
    Home,
    Star,
    /// only the cloud places (OneDrive, Google Drive…)
    Cloud,
}

pub fn ext_of(name: &str) -> String {
    Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tif", "tiff"];
pub const TEXT_EXT: &[&str] = &[
    "txt", "md", "log", "csv", "json", "xml", "ini", "cfg", "conf", "toml", "yaml", "yml", "bat", "cmd", "ps1", "rs",
    "py", "js", "ts", "html", "htm", "css", "c", "cpp", "h", "hpp", "cs", "java", "go", "sql", "sh", "reg",
];

pub fn kind_of(name: &str, dir: bool) -> Kind {
    if dir {
        return Kind::Folder;
    }
    let e = ext_of(name);
    match e.as_str() {
        "zip" | "7z" | "rar" | "tar" | "gz" | "cab" => Kind::Zip,
        "xlsx" | "xls" | "xlsm" | "ods" => Kind::Sheet,
        "csv" | "tsv" => Kind::Data,
        "docx" | "doc" | "odt" | "rtf" => Kind::Doc,
        "pptx" | "ppt" | "pptm" | "ppsx" | "pps" | "odp" => Kind::Slides,
        // not in IMAGE_EXT: the preview and thumbnails cannot decode vectors
        "svg" => Kind::Image,
        "pdf" => Kind::Pdf,
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "wma" => Kind::Audio,
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" => Kind::Video,
        "exe" | "msi" | "com" | "scr" => Kind::Exe,
        "txt" | "log" | "ini" | "cfg" | "conf" => Kind::Text,
        "md" | "markdown" => Kind::Markdown,
        "ps1" | "psm1" | "bat" | "cmd" | "sh" | "vbs" => Kind::Script,
        "bkp" | "old" | "orig" | "backup" => Kind::Backup,
        // Ctrl+B names copies .bak, .bak2, .bak3...
        _ if e.strip_prefix("bak").is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())) => Kind::Backup,
        _ if IMAGE_EXT.contains(&e.as_str()) => Kind::Image,
        _ if TEXT_EXT.contains(&e.as_str()) => Kind::Code,
        _ => Kind::File,
    }
}

/// Type names for common extensions (lowercase, as `ext_of` gives them) in language `l`; they
/// win over the shell's, which follow the installed apps and mix languages ("Markdown Source File").
pub fn known_type_name(ext: &str, l: i18n::Lang) -> Option<&'static str> {
    let key = match ext {
        "pdf" => "type.pdf",
        "doc" | "docx" => "type.word",
        "xls" | "xlsx" => "type.excel",
        "ppt" | "pptx" => "type.powerpoint",
        "odt" => "type.odt",
        "ods" => "type.ods",
        "txt" => "type.txt",
        "md" | "markdown" => "type.md",
        "csv" => "type.csv",
        "json" => "type.json",
        "yaml" | "yml" => "type.yaml",
        "toml" => "type.toml",
        "log" => "type.log",
        "ps1" => "type.ps1",
        "bat" | "cmd" => "type.bat",
        "bak" => "type.bak",
        "png" => "type.png",
        "jpg" | "jpeg" => "type.jpeg",
        "gif" => "type.gif",
        "svg" => "type.svg",
        "mp4" => "type.mp4",
        "mp3" => "type.mp3",
        "zip" => "type.zip",
        _ => return None,
    };
    Some(i18n::tr_in(l, key))
}

// ---------- formatting rules ----------
/// Sizes as classic Windows file managers show them: base 1024, next unit once the whole part
/// reaches 1000; whole part < 10 -> 2 decimals, < 100 -> 1, else none; decimals cut off (integer
/// division) and dropped only when they are all zero ("45 kB", but "1,50 MB"; in English "45 KB",
/// "1.50 MB", as English Windows writes them).
pub fn fmt_size(b: u64) -> String {
    let en = i18n::lang() == i18n::Lang::En;
    let units: [&str; 5] = [if en { "KB" } else { "kB" }, "MB", "GB", "TB", "PB"];
    if b < 1024 {
        return format!("{} B", group(b));
    }
    let (mut div, mut i) = (1024u64, 0);
    while i < units.len() - 1 && b / div >= 1000 {
        div *= 1024;
        i += 1;
    }
    let (whole, rem) = (b / div, b % div);
    let dec = if whole < 10 { 2 } else if whole < 100 { 1 } else { 0 };
    let frac = (rem as u128 * 10u128.pow(dec) / div as u128) as u64;
    if frac == 0 {
        format!("{} {}", group(whole), units[i])
    } else {
        format!("{}{}{:0w$} {}", group(whole), if en { '.' } else { ',' }, frac, units[i], w = dec as usize)
    }
}

/// Whole numbers with their thousands grouped: by a (non-breaking) space in Polish, a comma in English.
pub fn group(n: u64) -> String {
    let sep = if i18n::lang() == i18n::Lang::En { ',' } else { '\u{a0}' };
    let s = n.to_string();
    let mut out = String::new();
    for (k, c) in s.chars().enumerate() {
        if k > 0 && (s.len() - k) % 3 == 0 {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// Column widths that fit `avail`: in narrow panes whole columns are hidden (width 0) from the
/// least useful first — Atrybuty, then Typ, then Zmodyfikowany — so the name keeps at least
/// 200 px. A set width of 0 is a column the person hid.
pub fn col_widths(set: [f32; 4], avail: f32) -> ([f32; 4], f32) {
    let mut cw = set;
    for k in [3, 1, 2] {
        if avail - cw.iter().sum::<f32>() >= 200.0 {
            break;
        }
        cw[k] = 0.0;
    }
    (cw, (avail - cw.iter().sum::<f32>()).max(160.0))
}

/// The window's hit-test rectangle for a caption button, in client pixels [left, top, right,
/// bottom), from its egui rectangle in points at `ppp` pixels per point. Unless maximized, the
/// top `edge` points stay the resize border, as above Windows's own caption buttons.
pub fn caption_rect_px(rect: [f32; 4], ppp: f32, maximized: bool, edge: f32) -> [i32; 4] {
    let top = if maximized { rect[1] } else { rect[1].max(edge) };
    [rect[0], top, rect[2], rect[3]].map(|v| (v * ppp).round() as i32)
}

/// Whether client pixel (x, y) is in a rectangle from `caption_rect_px`: right and bottom edges
/// excluded, so the pixel column a button shares with its neighbour belongs to the neighbour.
pub fn caption_hit(r: [i32; 4], x: i32, y: i32) -> bool {
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

pub fn fmt_time(t: Option<SystemTime>) -> String {
    // dates before 1601 (the start of Windows FILETIME) are left blank
    use chrono::Datelike;
    // the short date Explorer shows in Polish, and in US English
    let format = match i18n::lang() {
        i18n::Lang::Pl => "%d.%m.%Y %H:%M",
        i18n::Lang::En => "%-m/%-d/%Y %-I:%M %p",
    };
    t.map(chrono::DateTime::<chrono::Local>::from).filter(|d| d.year() >= 1601).map(|d| d.format(format).to_string()).unwrap_or_default()
}

/// How long ago, for files changed in the last hours: "przed chwilą", "40 min temu", "2 godz. 5 min temu".
pub fn fmt_ago(secs: u64) -> String {
    let (h, m) = (secs / 3600, secs % 3600 / 60);
    match (h, m) {
        (0, 0) => t!("ago.now").into(),
        (0, m) => t!("ago.min", m = m),
        (h, 0) => t!("ago.hr", h = h),
        (h, m) => t!("ago.hr_min", h = h, m = m),
    }
}

/// Attribute letters R A H S E C O: read-only, archive, hidden, system, encrypted, compressed, offline.
pub fn fmt_attrs(a: u32) -> String {
    [(0x1, 'r'), (0x20, 'a'), (0x2, 'h'), (0x4, 's'), (0x4000, 'e'), (0x800, 'c'), (0x1000, 'o')]
        .iter()
        .map(|&(m, c)| if a & m != 0 { c } else { '-' })
        .collect()
}

/// Natural order: numbers by value, letters case-insensitively (Zeszyt2 < Zeszyt27).
/// Called n·log n times per sort, so it walks the strings in place instead of allocating.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a, b);
    loop {
        let (Some(cx), Some(cy)) = (x.chars().next(), y.chars().next()) else {
            return (!x.is_empty()).cmp(&!y.is_empty());
        };
        if cx.is_ascii_digit() && cy.is_ascii_digit() {
            let ex = x.find(|c: char| !c.is_ascii_digit()).unwrap_or(x.len());
            let ey = y.find(|c: char| !c.is_ascii_digit()).unwrap_or(y.len());
            let (rx, ry) = (&x[..ex], &y[..ey]);
            let (nx, ny) = (rx.trim_start_matches('0'), ry.trim_start_matches('0'));
            // equal values: more leading zeros sorts first ("a01" before "a1")
            let o = nx.len().cmp(&ny.len()).then_with(|| nx.cmp(ny)).then_with(|| (ry.len() - ny.len()).cmp(&(rx.len() - nx.len())));
            if o != Ordering::Equal {
                return o;
            }
            (x, y) = (&x[ex..], &y[ey..]);
        } else {
            let (lx, ly) = (cx.to_lowercase().next(), cy.to_lowercase().next());
            if lx != ly {
                return lx.cmp(&ly);
            }
            (x, y) = (&x[cx.len_utf8()..], &y[cy.len_utf8()..]);
        }
    }
}

/// Wildcard filter: patterns separated by ';' or spaces, `*` and `?`, any case.
pub fn glob_any(patterns: &str, name: &str) -> bool {
    fn m(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => m(&p[1..], n) || (!n.is_empty() && m(p, &n[1..])),
            (Some('?'), Some(_)) => m(&p[1..], &n[1..]),
            (Some(a), Some(b)) => a == b && m(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let mut pats = patterns.split([';', ' ', ',']).filter(|p| !p.is_empty()).peekable();
    // no filter (the usual case): every name passes without being copied
    if pats.peek().is_none() {
        return true;
    }
    let n: Vec<char> = name.to_lowercase().chars().collect();
    pats.any(|pat| {
        // text with no wildcard matches anywhere in the name: "faktura" works as "*faktura*"
        let pat = if pat.contains(['*', '?']) { pat.to_string() } else { format!("*{pat}*") };
        m(&pat.to_lowercase().chars().collect::<Vec<_>>(), &n)
    })
}

/// A path on a network share as the Linux server has it, for Kopiuj ścieżkę:
/// `\\serwer\ala\projekt\app` → `/home/ala/projekt/app`.
/// shortcut: assumes a Samba [homes] share (share = user, path /home/<user>), the server's only one;
/// another kind of share needs its real path from the server
pub fn server_path(unc: &str) -> Option<String> {
    let mut parts = unc.strip_prefix(r"\\")?.split('\\').filter(|s| !s.is_empty());
    let share = parts.nth(1)?;
    Some(std::iter::once(format!("/home/{share}")).chain(parts.map(String::from)).collect::<Vec<_>>().join("/"))
}

/// Whether a file's Windows attributes say its content is still in the cloud (offline, recall on
/// open, recall on data access): reading it would download it, so nothing reads it unasked.
pub fn only_in_cloud(attrs: u32) -> bool {
    attrs & (0x1000 | 0x40000 | 0x400000) != 0
}

/// The bit of a path's drive letter as Windows numbers the drives in use (bit 0: A); 0 for a
/// path with no letter (a network share).
pub fn drive_bit(path: &str) -> u32 {
    match path.as_bytes() {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => 1 << (letter.to_ascii_uppercase() - b'A'),
        _ => 0,
    }
}

/// Whether a change at `changed` may have altered the list of the folder `dir`: both are
/// lower-case paths under the same watched root, without a backslash at either end (`dir` is
/// empty for the root itself). A thing right in the folder has; so has the folder itself or one
/// above it renamed, removed or made (`modified`: only its time or attributes changed, which says
/// nothing), as the path may name another folder now.
pub fn concerns(dir: &str, changed: &str, modified: bool) -> bool {
    let inside = changed.rsplit_once('\\').map_or("", |(up, _)| up) == dir;
    let above = dir.strip_prefix(changed).is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'));
    inside || (above && !modified) || short_name(changed)
}

/// A path with a short (8.3) name in it ("progra~1"), as Windows reports a change made through
/// one: it cannot be matched with a folder's long path, so it may concern any folder.
fn short_name(path: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    path.split('\\').any(|part| part.split_once('~').is_some_and(|(_, n)| digits(n.split('.').next().unwrap_or(""))))
}

/// One waiting request for a worker thread: a newer one replaces it, so a held arrow key builds
/// the preview only where the selection comes to rest. Beside it: when the last request came and
/// how long after the one before.
pub struct Latest<T>(Mutex<(Option<T>, Option<Instant>, Option<Duration>)>, Condvar);

impl<T> Latest<T> {
    pub fn new() -> Self {
        Latest(Mutex::new((None, None, None)), Condvar::new())
    }

    pub fn put(&self, v: T) {
        let mut slot = self.0.lock().unwrap();
        *slot = (Some(v), Some(Instant::now()), slot.1.map(|t| t.elapsed()));
        self.1.notify_one();
    }

    /// Waits for a request and hands it out at once when it came alone (more than `settle` after
    /// the one before); in a burst it waits until the newest has rested for `settle`.
    pub fn take(&self, settle: Duration) -> T {
        let mut slot = self.1.wait_while(self.0.lock().unwrap(), |s| s.0.is_none()).unwrap();
        while let (Some(at), Some(gap)) = (slot.1, slot.2) {
            let rest = settle.saturating_sub(at.elapsed());
            if gap > settle || rest.is_zero() {
                break;
            }
            slot = self.1.wait_timeout(slot, rest).unwrap().0;
        }
        slot.0.take().unwrap()
    }
}

/// Requests for a worker thread, handed out in the order asked, so a screen of thumbnails fills
/// from its first tile. The asker drops with `retain` those nobody waits for any more, which is
/// what brings the tiles on screen first after a jump through a long list.
pub struct Requests<T>(Mutex<VecDeque<T>>, Condvar);

impl<T> Requests<T> {
    pub fn new() -> Self {
        Requests(Mutex::new(VecDeque::new()), Condvar::new())
    }

    pub fn push(&self, v: T) {
        self.0.lock().unwrap().push_back(v);
        self.1.notify_one();
    }

    /// Waits for a request and hands out the one asked for first.
    pub fn take(&self) -> T {
        self.1.wait_while(self.0.lock().unwrap(), |q| q.is_empty()).unwrap().pop_front().unwrap()
    }

    pub fn retain(&self, keep: impl FnMut(&T) -> bool) {
        self.0.lock().unwrap().retain(keep);
    }
}

/// The oldest "last drawn" stamp to keep so that about `cap` cache entries stay (those sharing the
/// stamp at the cut go together), never dropping one drawn since `live`: it is still on screen.
pub fn keep_from(mut stamps: Vec<u64>, cap: usize, live: u64) -> u64 {
    if stamps.len() <= cap {
        return 0;
    }
    stamps.sort_unstable();
    (stamps[stamps.len() - cap - 1] + 1).min(live)
}

/// What is asked of other threads for one place on screen (a list's rows, a tree folder's
/// folders): each request gets a number no other has, and only the answer to the newest counts,
/// so a late answer to an earlier one (a folder left since, a share slow to answer) never
/// replaces newer content. Beside it: what is to follow that answer (`then`).
pub struct Asked<T> {
    /// the request waited for; 0: none
    number: u64,
    then: Option<T>,
}

impl<T> Default for Asked<T> {
    fn default() -> Self {
        Asked { number: 0, then: None }
    }
}

impl<T> Asked<T> {
    /// A new request: the answers to earlier ones are not waited for any more.
    pub fn ask(&mut self) -> u64 {
        static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        self.number = LAST.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        self.number
    }

    pub fn number(&self) -> u64 {
        self.number
    }

    pub fn waiting(&self) -> bool {
        self.number != 0
    }

    /// What follows the answer waited for; it stays through a repeated request.
    pub fn then(&mut self, t: T) {
        self.then = Some(t);
    }

    /// No answer is waited for and nothing follows one.
    pub fn cancel(&mut self) {
        *self = Self::default();
    }

    /// The answer to request `number` came. None: not the one waited for, to be dropped;
    /// otherwise what was to follow it, and nothing is waited for any more.
    pub fn take(&mut self, number: u64) -> Option<Option<T>> {
        (number != 0 && number == self.number).then(|| {
            self.number = 0;
            self.then.take()
        })
    }
}

/// What a list selects once its rows are in: the folder just made (`rename`: its name is edited
/// at once), the renamed file, what a change to an archive made, the folder just left.
pub struct Select {
    pub paths: Vec<PathBuf>,
    pub rename: bool,
    /// only the paths the list has (with none of them the selection stays as it is); otherwise
    /// all, as a row deep in a tree comes after the list
    pub listed: bool,
}

impl Select {
    /// The paths to select, the first of them taking the focus; `has`: the list has this path.
    pub fn pick(self, has: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
        if self.listed {
            self.paths.into_iter().filter(|p| has(p)).collect()
        } else {
            self.paths
        }
    }
}

/// The "Preferred DropEffect" a file list on the clipboard carries, as Explorer writes it: move
/// for Wytnij, copy or link for Kopiuj.
pub fn clip_effect(cut: bool) -> u32 {
    if cut { 2 } else { 5 }
}

/// Whether a file list on the clipboard was cut: it offers moving and not copying. A value that
/// also allows copying is copied, so a doubtful one never takes files away from where they are.
pub fn clip_is_cut(effect: u32) -> bool {
    effect & 2 != 0 && effect & 1 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_cut_or_copy() {
        // Explorer's own values round-trip; copy+move (3) or nothing (0) is a copy
        assert!(clip_is_cut(clip_effect(true)) && !clip_is_cut(clip_effect(false)));
        assert!(!clip_is_cut(3) && !clip_is_cut(0));
    }

    #[test]
    fn maximize_button_hit_rect() {
        // 46×36 button at 150 %: the top 4 points resize the window unless it is maximized
        assert_eq!(caption_rect_px([1000.0, 0.0, 1046.0, 36.0], 1.5, false, 4.0), [1500, 6, 1569, 54]);
        assert_eq!(caption_rect_px([1000.0, 0.0, 1046.0, 36.0], 1.5, true, 4.0), [1500, 0, 1569, 54]);
        // the pixel column shared with the next button (Zamknij) is that button's, not this one's
        let max = caption_rect_px([1000.0, 0.0, 1046.0, 36.0], 1.25, true, 4.0);
        let close = caption_rect_px([1046.0, 0.0, 1092.0, 36.0], 1.25, true, 4.0);
        assert!(caption_hit(close, max[2], 10) && !caption_hit(max, max[2], 10) && caption_hit(max, max[2] - 1, 10));
        assert!(caption_hit(max, max[0], 0) && !caption_hit(max, max[0], max[3]));
        // not maximized, the resize band above it is no part of the button
        let r = caption_rect_px([1000.0, 0.0, 1046.0, 36.0], 1.5, false, 4.0);
        assert!(!caption_hit(r, 1520, 5) && caption_hit(r, 1520, 6));
    }

    #[test]
    fn only_the_newest_request_is_answered_and_what_follows_waits_for_it() {
        let mut a = Asked::<Select>::default();
        assert!(!a.waiting() && a.take(0).is_none());
        let first = a.ask();
        a.then(Select { paths: vec!["nowy".into(), "inny".into()], rename: true, listed: true });
        // asked again before the answer (another program changed the folder meanwhile)
        let second = a.ask();
        assert!(second != first && a.waiting());
        // the late answer to the first request is dropped, and what follows still waits
        assert!(a.take(first).is_none() && a.waiting());
        let then = a.take(second).expect("the newest answer counts").expect("what follows comes with it");
        assert!(!a.waiting() && a.take(second).is_none());
        // another list's requests never share a number
        assert!(Asked::<()>::default().ask() > second);
        // of what a change made only the rows the list has; a row to come later is taken as it is
        assert!(then.rename);
        assert_eq!(then.pick(|p| p == Path::new("nowy")), [PathBuf::from("nowy")]);
        let deep = Select { paths: vec!["a/b".into()], rename: false, listed: false };
        assert_eq!(deep.pick(|_| false), [PathBuf::from("a/b")]);
        // a folder left before its answer: nothing follows any more
        a.ask();
        a.then(Select { paths: vec![], rename: false, listed: false });
        a.cancel();
        assert!(!a.waiting());
        let again = a.ask();
        assert!(a.take(again).is_some_and(|then| then.is_none()));
    }

    #[test]
    fn requests_go_in_the_order_asked_and_dropped_ones_never() {
        let q = Requests::new();
        (1..=5).for_each(|i| q.push(i));
        q.retain(|&i| i != 4);
        assert_eq!([q.take(), q.take(), q.take(), q.take()], [1, 2, 3, 5]);
    }

    #[test]
    fn a_change_concerns_the_folder_it_is_in_and_those_under_a_renamed_one() {
        let dir = r"dane\projekt\src";
        // a file or folder right in it, however it changed
        assert!(concerns(dir, r"dane\projekt\src\main.rs", true));
        assert!(concerns(dir, r"dane\projekt\src\nowy", false));
        // deeper, beside it, or a longer name: not its list
        assert!(!concerns(dir, r"dane\projekt\src\a\b.txt", false));
        assert!(!concerns(dir, r"dane\projekt\inne.txt", false));
        assert!(!concerns(dir, r"dane\projekt\src2", false));
        // itself or a folder above renamed, removed or made again; their own time changing is nothing
        assert!(concerns(dir, r"dane\projekt\src", false));
        assert!(concerns(dir, "dane", false));
        assert!(!concerns(dir, "dane", true));
        assert!(!concerns(dir, r"dane\projekt\src", true));
        // the volume's root folder: only what is right in it
        assert!(concerns("", "plik.txt", true));
        assert!(!concerns("", r"dane\plik.txt", false));
        // a short name hides which folder it is; a tilde elsewhere in a name does not
        assert!(concerns(dir, r"progra~1\x\y.txt", true));
        assert!(concerns(dir, r"inne\dokume~2.doc", true));
        assert!(!concerns(dir, r"inne\~$raport.docx", true));
        assert!(!concerns(dir, r"inne\preferences~rf1a2b.tmp", true));
        // a drive that came or went is known by its letter's bit
        assert_eq!([drive_bit(r"C:\Dane"), drive_bit("e:"), drive_bit(r"\\serwer\udział"), drive_bit("::pc")], [4, 16, 0, 0]);
    }

    #[test]
    fn a_request_that_comes_alone_is_handed_out_at_once() {
        let slot = Latest::new();
        slot.put(1);
        let t0 = Instant::now();
        assert_eq!(slot.take(Duration::from_secs(5)), 1);
        // and so is one that follows after more than the settle time
        std::thread::sleep(Duration::from_millis(30));
        slot.put(2);
        assert_eq!(slot.take(Duration::from_millis(10)), 2);
        assert!(t0.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_file_only_in_the_cloud_is_not_read() {
        // as OneDrive's files read on the test laptop: a placeholder, and one pinned but not here yet
        assert!(only_in_cloud(0x401620));
        assert!(only_in_cloud(0x481620));
        // an archived file of a network share (offline alone) is recalled by a read just the same
        assert!(only_in_cloud(0x1020));
        // on this device: hydrated, pinned, and a plain file outside any cloud folder
        assert!(!only_in_cloud(0x420));
        assert!(!only_in_cloud(0x80420));
        assert!(!only_in_cloud(0x20));
    }

    #[test]
    fn a_full_cache_drops_the_longest_unseen() {
        let kept = |stamps: &[u64], cap, live| {
            let from = keep_from(stamps.to_vec(), cap, live);
            stamps.iter().filter(|&&s| s >= from).count()
        };
        // under the cap nothing goes
        assert_eq!(keep_from(vec![3, 1, 2], 3, 9), 0);
        assert_eq!(kept(&[5, 1, 4, 2, 3], 3, 9), 3);
        // a row drawn in one frame shares its stamp and goes whole
        assert_eq!(kept(&[1, 2, 2, 2, 3], 3, 9), 1);
        // more on screen than the cap: all of it stays
        assert_eq!(kept(&[1, 7, 7, 7, 7], 2, 7), 4);
    }

    #[test]
    fn a_burst_of_requests_builds_the_last_one() {
        let slot = std::sync::Arc::new(Latest::new());
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = slot.clone();
        std::thread::spawn(move || while tx.send(worker.take(Duration::from_millis(60))).is_ok() {});
        for i in 0..50 {
            slot.put(i);
            std::thread::sleep(Duration::from_millis(1));
        }
        let built: Vec<i32> = std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(500)).ok()).collect();
        // one when nothing stalls; a few if this thread was held back longer than the settle time
        assert!(built.len() <= 3, "{built:?}");
        assert_eq!(built.last(), Some(&49));
    }

    #[test]
    fn network_paths_as_the_server_sees_them() {
        assert_eq!(server_path(r"\\serwer\ala\projekt\app").as_deref(), Some("/home/ala/projekt/app"));
        assert_eq!(server_path(r"\\serwer\ala\").as_deref(), Some("/home/ala"));
        assert_eq!(server_path(r"\\serwer").as_deref(), None);
        assert_eq!(server_path(r"C:\Users"), None);
    }

    #[test]
    fn polish_type_names_for_common_extensions() {
        assert_eq!(known_type_name(&ext_of("README.MD"), i18n::Lang::Pl), Some("Dokument Markdown"));
        assert_eq!(known_type_name("svg", i18n::Lang::Pl), Some("Obraz SVG"));
        assert_eq!(known_type_name("yml", i18n::Lang::Pl), Some("Plik YAML"));
        assert_eq!(known_type_name("bat", i18n::Lang::Pl), Some("Skrypt wsadowy"));
        assert_eq!(known_type_name("bak", i18n::Lang::Pl), Some("Kopia zapasowa"));
        assert_eq!(known_type_name("exe", i18n::Lang::Pl), None);
        assert_eq!(known_type_name("", i18n::Lang::Pl), None);
    }

    #[test]
    fn time_ago() {
        let got: Vec<String> = [0, 59, 60, 2400, 3600, 7500].iter().map(|&s| fmt_ago(s)).collect();
        assert_eq!(got, ["przed chwilą", "przed chwilą", "1 min temu", "40 min temu", "1 godz. temu", "2 godz. 5 min temu"]);
        i18n::set_lang(i18n::Lang::En);
        let got: Vec<String> = [0, 60, 3600, 7500].iter().map(|&s| fmt_ago(s)).collect();
        assert_eq!(got, ["just now", "1 min ago", "1 hr ago", "2 hr 5 min ago"]);
    }

    #[test]
    fn english_numbers_sizes_and_dates() {
        i18n::set_lang(i18n::Lang::En);
        assert_eq!(group(826_734), "826,734");
        assert_eq!(fmt_size(46_223), "45.1 KB");
        assert_eq!(fmt_size(1_572_864), "1.50 MB");
        assert_eq!(fmt_size(1_000_000_000_000), "931 GB");
        assert_eq!(known_type_name("zip", i18n::Lang::En), Some("Compressed (zipped) Folder"));
        let t = chrono::NaiveDate::from_ymd_opt(2026, 3, 7).unwrap().and_hms_opt(14, 5, 0).unwrap().and_local_timezone(chrono::Local).unwrap();
        assert_eq!(fmt_time(Some(t.into())), "3/7/2026 2:05 PM");
        i18n::set_lang(i18n::Lang::Pl);
        assert_eq!(fmt_time(Some(t.into())), "07.03.2026 14:05");
    }

    #[test]
    fn sizes_are_formatted_like_explorer_style_managers() {
        assert_eq!(fmt_size(292), "292 B");
        // sizes with the text Windows-style file managers show for them
        assert_eq!(fmt_size(46_223), "45,1 kB");
        assert_eq!(fmt_size(165_634), "161 kB");
        assert_eq!(fmt_size(10_104_223), "9,63 MB");
        assert_eq!(fmt_size(4_926_013), "4,69 MB");
        assert_eq!(fmt_size(4_182_016), "3,98 MB");
        assert_eq!(fmt_size(13 * 1024 * 1024 * 1024), "13 GB");
        assert_eq!(fmt_size(1024 * 1024 * 3 / 2), "1,50 MB");
        assert_eq!(fmt_size(1023), "1\u{a0}023 B");
        assert_eq!(fmt_size(1_023_999), "999 kB");
        assert_eq!(fmt_size(1_024_000), "0,97 MB");
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["Zeszyt27.xlsx", "Zeszyt2.xlsx", "zeszyt1.xlsx", "Zeszyt10.xlsx"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["zeszyt1.xlsx", "Zeszyt2.xlsx", "Zeszyt10.xlsx", "Zeszyt27.xlsx"]);
        assert_eq!(natural_cmp("a01", "a1"), Ordering::Less);
    }

    /// The previous, allocating version: the in-place one must order everything the same way.
    fn natural_cmp_reference(a: &str, b: &str) -> Ordering {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
                let (si, sj) = (i, j);
                while i < a.len() && a[i].is_ascii_digit() {
                    i += 1;
                }
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                let na: String = a[si..i].iter().collect::<String>().trim_start_matches('0').to_string();
                let nb: String = b[sj..j].iter().collect::<String>().trim_start_matches('0').to_string();
                let (za, zb) = (i - si - na.len(), j - sj - nb.len());
                let o = na.len().cmp(&nb.len()).then_with(|| na.cmp(&nb)).then_with(|| zb.cmp(&za));
                if o != Ordering::Equal {
                    return o;
                }
            } else {
                let (x, y) = (a[i].to_lowercase().next(), b[j].to_lowercase().next());
                if x != y {
                    return x.cmp(&y);
                }
                i += 1;
                j += 1;
            }
        }
        (a.len() - i).cmp(&(b.len() - j))
    }

    #[test]
    fn natural_order_matches_the_previous_version() {
        let names = [
            "", "a", "A", "a1", "a01", "a001", "a1b", "a1B", "a10", "a2", "a02b3", "a2b03", "1", "01", "10", "0", "00",
            "Zeszyt2.xlsx", "zeszyt10.xlsx", "żółw", "Żółw2", "ŻÓŁW10", "x.tar.gz", "x9", "x9 ", "9x", "99999999999999999999", "100000000000000000000",
        ];
        for a in names {
            for b in names {
                assert_eq!(natural_cmp(a, b), natural_cmp_reference(a, b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn filters() {
        assert!(glob_any("*.pdf;*.jpg", "Faktura.PDF"));
        assert!(!glob_any("*.pdf", "zdjecie.jpg"));
        assert!(glob_any("", "anything"));
        assert!(glob_any(" ; ", "anything"));
        assert!(glob_any("cennik", "Moj_Cennik_2025.xlsx"));
    }

    #[test]
    fn columns_fit_narrow_panes() {
        let set = [90.0, 210.0, 140.0, 80.0];
        assert_eq!(col_widths(set, 1200.0), (set, 680.0));
        // a split pane keeps the date before the type
        assert_eq!(col_widths(set, 560.0), ([90.0, 0.0, 140.0, 0.0], 330.0));
        assert_eq!(col_widths(set, 300.0), ([90.0, 0.0, 0.0, 0.0], 210.0));
        // hidden Atrybuty (width 0) leaves the rest as set
        assert_eq!(col_widths([90.0, 210.0, 140.0, 0.0], 1200.0), ([90.0, 210.0, 140.0, 0.0], 760.0));
    }

    #[test]
    fn thousands_grouping() {
        assert_eq!(group(826_734), "826\u{a0}734");
        assert_eq!(group(999), "999");
    }

    #[test]
    fn polish_plural() {
        assert_eq!([1, 2, 5, 12, 22].map(|n| i18n::plural("drives", n)), ["dysk", "dyski", "dysków", "dysków", "dyski"]);
    }

    #[test]
    fn attributes() {
        assert_eq!(fmt_attrs(0x20), "-a-----");
        assert_eq!(fmt_attrs(0), "-------");
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of("Faktura.PDF", false), Kind::Pdf);
        assert_eq!(kind_of("x.tiff", false), Kind::Image);
        assert_eq!(kind_of("main.rs", false), Kind::Code);
        assert_eq!(kind_of("dir.zip", true), Kind::Folder);
        assert_eq!(kind_of("Prezentacja.pptx", false), Kind::Slides);
        assert_eq!(kind_of("stara.ppt", false), Kind::Slides);
        assert_eq!(kind_of("logo.svg", false), Kind::Image);
        assert_eq!(kind_of("raport.xlsx.bak", false), Kind::Backup);
        assert_eq!(kind_of("raport.xlsx.bak3", false), Kind::Backup);
        assert_eq!(kind_of("a.bakery", false), Kind::File);
        assert_eq!(kind_of("README.md", false), Kind::Markdown);
        assert_eq!(kind_of("notatki.txt", false), Kind::Text);
        assert_eq!(kind_of("dane.csv", false), Kind::Data);
        assert_eq!(kind_of("arkusz.xlsx", false), Kind::Sheet);
        for code in ["a.py", "a.js", "a.json", "a.xml", "a.yaml", "Cargo.toml"] {
            assert_eq!(kind_of(code, false), Kind::Code, "{code}");
        }
        for script in ["uruchom.ps1", "a.psm1", "a.BAT", "a.cmd", "a.sh", "a.vbs"] {
            assert_eq!(kind_of(script, false), Kind::Script, "{script}");
        }
    }
}
