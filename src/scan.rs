//! VirusTotal: a file checked by its SHA-256 (nothing of it leaves the computer), sent there only
//! when the person asks; the verdict shows in the preview pane and the status bar.

use super::*;
use rx_scan::Report;

/// Where Windows keeps the person's VirusTotal API key (Menedżer poświadczeń).
pub(crate) const KEY_NAME: &str = "RustExplorer/VirusTotal";
/// The page where a signed-in person finds their API key.
const KEY_PAGE: &str = "https://www.virustotal.com/gui/my-apikey";

pub(crate) enum Vt {
    /// hashing the file and asking VirusTotal
    Checking,
    /// VirusTotal's verdict, with the file's SHA-256 for its page
    Known(String, Report),
    /// VirusTotal has never seen this file
    Unknown(String),
    /// sent; the engines are at it
    Sending,
    Failed(String),
}

/// The same version of a file: modified in the same second (search results come from the index,
/// which keeps whole seconds; the file system keeps 100 ns).
fn same_version(a: Option<SystemTime>, b: Option<SystemTime>) -> bool {
    let s = |t: Option<SystemTime>| t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
    s(a) == s(b)
}

/// The verdict in words, for the preview and the status bar.
fn verdict(r: &Report) -> String {
    if r.clean() {
        t!("scan.clean", n = r.engines)
    } else if r.malicious > 0 {
        t!("scan.malicious", m = r.malicious, n = r.engines)
    } else {
        t!("scan.suspicious", m = r.suspicious, n = r.engines)
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// What a check or a send ended with, in words (Err: it failed); none while it still runs.
fn outcome(p: &Path, v: &Vt) -> Option<Result<String, String>> {
    let name = file_name(p);
    Some(match v {
        Vt::Known(_, r) => Ok(t!("scan.known", name = name, verdict = verdict(r))),
        Vt::Unknown(_) => Ok(t!("scan.unknown", name = name)),
        Vt::Failed(e) => Err(t!("scan.failed", name = name, e = e)),
        Vt::Checking | Vt::Sending => return None,
    })
}

/// A worker's end: its Operacje card closes with the outcome, the window gets the verdict.
fn done(ops: &rx_core::ops::Ops, id: u64, tx: &std::sync::mpsc::Sender<Msg>, ctx: &egui::Context, p: PathBuf, v: Vt) {
    if let Some(o) = outcome(&p, &v) {
        ops.finish(id, o);
    }
    let _ = tx.send(Msg::Vt(p, v));
    ctx.request_repaint();
}

/// The verdict as VirusTotal's page shows it, a ring with the number inside and its left edge's
/// middle at `at`: green with the number of engines for a clean file, red with the number that
/// found something (orange when they only call it suspicious). Returns the badge's width.
pub(crate) fn paint_badge(p: &Painter, at: egui::Pos2, r: &Report, h: f32) -> f32 {
    let (n, color) = if r.clean() {
        (r.engines, pal().new_text)
    } else if r.malicious > 0 {
        (r.malicious, pal().red)
    } else {
        (r.suspicious, pal().warn)
    };
    let g = p.layout_no_wrap(n.to_string(), FontId::new(h * 0.5, FontFamily::Name("semibold".into())), color);
    let w = (g.size().x + h * 0.5).max(h);
    let rect = Rect::from_min_size(pos2(at.x, at.y - h / 2.0), vec2(w, h));
    // filled, so the ring also reads on a selected row and on a picture's thumbnail
    p.rect(rect, cr(h / 2.0), pal().field, Stroke::new((h * 0.08).max(1.5), color), StrokeKind::Inside);
    p.galley(rect.center() - g.size() / 2.0, g, color);
    w
}

/// Verdicts kept between starts, one line each: path, modified time (ns since 1970, "-" when
/// unknown), SHA-256, malicious, suspicious, engines.
// shortcut: lines of deleted files stay; drop them on load if the list ever grows large
pub(crate) fn vt_to_text(vt: &HashMap<PathBuf, (Option<SystemTime>, Vt)>) -> String {
    let ns = |t: &Option<SystemTime>| t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or("-".into(), |d| d.as_nanos().to_string());
    vt.iter()
        .filter_map(|(p, (t, v))| match v {
            Vt::Known(sha, r) => Some(format!("{}\t{}\t{sha}\t{}\t{}\t{}", p.display(), ns(t), r.malicious, r.suspicious, r.engines)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn vt_from_text(text: &str) -> HashMap<PathBuf, (Option<SystemTime>, Vt)> {
    let line = |l: &str| {
        let f: Vec<&str> = l.split('\t').collect();
        let [path, t, sha, m, s, e] = f[..] else { return None };
        let t = if t == "-" { None } else { Some(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(t.parse().ok()?)) };
        let r = Report { malicious: m.parse().ok()?, suspicious: s.parse().ok()?, engines: e.parse().ok()? };
        Some((PathBuf::from(path), (t, Vt::Known(sha.to_string(), r))))
    };
    text.lines().filter_map(line).collect()
}

impl App {
    /// The verdict on this version of the file, for its badge in the list.
    pub(crate) fn vt_report(&self, path: &Path, modified: Option<SystemTime>) -> Option<Report> {
        match self.vt.get(path) {
            Some((t, Vt::Known(_, r))) if same_version(*t, modified) => Some(*r),
            _ => None,
        }
    }

    /// Checks the file in VirusTotal by its hash; without a key, asks for one first.
    pub(crate) fn vt_check(&mut self, p: PathBuf, ctx: &egui::Context) {
        if matches!(self.vt.get(&p), Some((_, Vt::Checking | Vt::Sending))) {
            return;
        }
        let Some(key) = sys::secret(KEY_NAME) else {
            self.ask_vt_key = Some((String::new(), Some(p), false));
            return;
        };
        // the verdict holds for this version of the file only (see vt_ui)
        let stamp = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
        self.vt.insert(p.clone(), (stamp, Vt::Checking));
        let (tx, ctx, ops) = (self.tx.clone(), ctx.clone(), self.ops.clone());
        let id = ops.start(t!("scan.checking_job").into(), file_name(&p));
        std::thread::spawn(move || {
            let mut wait = |t| {
                ops.queue(id, t);
                ctx.request_repaint();
            };
            let r = rx_scan::sha256(&p).map_err(|e| t!("scan.cant_read", e = e)).and_then(|sha| {
                Ok(match rx_scan::lookup(&key, &sha, &mut wait)? {
                    Some(rep) => Vt::Known(sha, rep),
                    None => Vt::Unknown(sha),
                })
            });
            done(&ops, id, &tx, &ctx, p, r.unwrap_or_else(Vt::Failed));
        });
    }

    /// Sends a file VirusTotal does not know, then waits for its engines (polled every 20 s, as
    /// the free key allows 4 requests a minute).
    pub(crate) fn vt_send(&mut self, p: PathBuf, ctx: &egui::Context) {
        let (Some(key), Some((stamp, Vt::Unknown(sha)))) = (sys::secret(KEY_NAME), self.vt.get(&p)) else { return };
        let (stamp, sha) = (*stamp, sha.clone());
        self.vt.insert(p.clone(), (stamp, Vt::Sending));
        let (tx, ctx, ops) = (self.tx.clone(), ctx.clone(), self.ops.clone());
        let job = ops.start(t!("scan.sending_job").into(), file_name(&p));
        std::thread::spawn(move || {
            let r = (|| {
                let id = rx_scan::upload(&key, &p, &mut |t| {
                    ops.queue(job, t);
                    ctx.request_repaint();
                })?;
                for _ in 0..30 {
                    std::thread::sleep(std::time::Duration::from_secs(20));
                    // a refused poll (a dropped line) is just tried again; its turn in the queue
                    // is not shown, the file already went
                    if let Ok(Some(rep)) = rx_scan::analysis(&key, &id, &mut |_| {}) {
                        return Ok(Vt::Known(sha.clone(), rep));
                    }
                }
                Err(t!("scan.slow").to_string())
            })();
            done(&ops, job, &tx, &ctx, p, r.unwrap_or_else(Vt::Failed));
        });
    }

    /// A result from a check or a send: kept for the preview, told in the status bar.
    pub(crate) fn vt_result(&mut self, p: PathBuf, v: Vt) {
        let Some(note) = outcome(&p, &v) else { return };
        self.msg = Some(note.unwrap_or_else(|e| e));
        // the file version it was asked about stays with the answer
        if let Some(e) = self.vt.get_mut(&p) {
            e.1 = v;
        }
    }

    /// The VirusTotal part of a file's preview: a button to check it, then the verdict (none for a
    /// file changed since it was checked: the verdict was about its old contents).
    pub(crate) fn vt_ui(&mut self, ui: &mut Ui, p: &Path, modified: Option<SystemTime>) {
        let button = |ui: &mut Ui, label: &str| {
            ui.add(egui::Button::new((icon(ic::SHIELD, 16.0).color(pal().text), RichText::new(label))).fill(pal().field).stroke(Stroke::new(1.0, pal().line)).min_size(vec2(0.0, 32.0))).clicked()
        };
        let line = |ui: &mut Ui, text: String, color: Color32| {
            ui.horizontal_wrapped(|ui| {
                ui.label(icon(ic::SHIELD, 16.0).color(color));
                ui.label(RichText::new(text).color(color));
            });
        };
        match self.vt.get(p).filter(|(stamp, _)| same_version(*stamp, modified)).map(|(_, v)| v) {
            None => {
                if button(ui, t!("scan.check")) {
                    self.pending.push(Act::VtCheck(p.to_path_buf()));
                }
            }
            Some(Vt::Checking) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(t!("scan.checking")).color(pal().dim));
                });
            }
            Some(Vt::Sending) => {
                ui.horizontal_wrapped(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(t!("scan.sent")).color(pal().dim));
                });
            }
            Some(Vt::Known(sha, r)) => {
                let color = if r.clean() { pal().new_text } else { pal().red };
                let (r, text, page) = (*r, verdict(r), rx_scan::page(sha));
                ui.allocate_ui_with_layout(vec2(ui.available_width(), 30.0), Layout::left_to_right(Align::Center), |ui| {
                    let w = paint_badge(ui.painter(), ui.cursor().left_top() + vec2(0.0, 15.0), &r, 30.0);
                    ui.add_space(w + 8.0);
                    ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
                });
                if ui.link(t!("scan.report")).clicked() {
                    let _ = sys::open(Path::new(&page));
                }
            }
            Some(Vt::Unknown(_)) => {
                line(ui, t!("scan.not_known").into(), pal().dim);
                ui.add(egui::Label::new(RichText::new(t!("scan.upload_warning")).size(12.0).color(pal().dim)).wrap());
                if button(ui, t!("scan.upload")) {
                    self.pending.push(Act::VtSend(p.to_path_buf()));
                }
            }
            Some(Vt::Failed(e)) => {
                line(ui, e.clone(), pal().red);
                if button(ui, t!("scan.check_again")) {
                    self.pending.push(Act::VtCheck(p.to_path_buf()));
                }
            }
        }
    }

    /// The window asking for the VirusTotal key (Plik → Ustawienia, or the first check).
    pub(crate) fn vt_key_dialog(&mut self, ctx: &egui::Context) {
        let Some((text, then, had)) = &mut self.ask_vt_key else { return };
        let had = *had;
        let (mut save, mut remove, mut cancel) = (false, false, false);
        let r = egui::Modal::new(egui::Id::unique("klucz_virustotal")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(t!("scan.key.title")).heading());
            ui.add_space(6.0);
            ui.add(egui::Label::new(t!("scan.key.info")).wrap());
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(t!("scan.key.limit")).color(pal().dim)).wrap());
            if ui.link(t!("scan.key.page")).clicked() {
                let _ = sys::open(Path::new(KEY_PAGE));
            }
            ui.add_space(8.0);
            let hint = if had { t!("scan.key.hint_new") } else { t!("scan.key.hint") };
            let e = ui.add(egui::TextEdit::singleline(text).password(true).hint_text(hint).desired_width(f32::INFINITY));
            if !e.has_focus() && !e.lost_focus() {
                e.request_focus();
            }
            save = e.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                let plain = |t| egui::Button::new(t).min_size(vec2(96.0, 32.0)).fill(pal().field).stroke(Stroke::new(1.0, pal().line));
                cancel = ui.add(plain(t!("common.cancel"))).clicked();
                save |= ui.add(plain(t!("scan.key.save"))).clicked();
                if had {
                    remove = ui.add(plain(t!("scan.key.remove"))).clicked();
                }
            });
        });
        let key = text.trim().to_string();
        let then = then.clone();
        if save && !key.is_empty() {
            self.ask_vt_key = None;
            match sys::set_secret(KEY_NAME, Some(&key)) {
                Ok(()) => {
                    self.msg = Some(t!("scan.key.saved").into());
                    if let Some(p) = then {
                        self.vt_check(p, ctx);
                    }
                }
                Err(e) => self.msg = Some(e),
            }
        } else if remove {
            self.ask_vt_key = None;
            self.msg = Some(sys::set_secret(KEY_NAME, None).map_or_else(|e| e, |_| t!("scan.key.removed").into()));
        } else if cancel || r.should_close() {
            self.ask_vt_key = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_survive_saving() {
        let t = Some(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(1_762_000_000_123_456_700));
        let r = Report { malicious: 2, suspicious: 0, engines: 70 };
        let p = PathBuf::from(r"C:\Pobrane\setup.exe");
        let m: HashMap<_, _> = [(p.clone(), (t, Vt::Known("ab".into(), r))), (PathBuf::from(r"C:\x"), (None, Vt::Checking))].into();
        let back = vt_from_text(&vt_to_text(&m));
        // only verdicts are kept, a check under way is not
        assert_eq!(back.len(), 1);
        assert!(matches!(&back[&p], (bt, Vt::Known(s, br)) if *bt == t && s == "ab" && *br == r));
        assert!(vt_from_text("zepsuta\tlinia").is_empty());
    }
}
