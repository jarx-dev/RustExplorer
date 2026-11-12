//! The Operacje indicator in the status bar and the panel it opens: a card per running or just
//! finished file operation, with a throughput chart over its progress (rx_core::ops keeps the data).

use super::*;
use rx_core::ops::{fmt_count, fmt_pct, fmt_speed, overall, Job, State};
use std::time::Instant;

const PANEL_W: f32 = 390.0;

fn panel_id() -> egui::Id {
    egui::Id::unique("ops_panel")
}

/// An open popup that owns the keys and drops: any but the Operacje panel, which is only watched
/// (its Esc closes it and does nothing else).
pub(crate) fn popup_blocks(ctx: &egui::Context) -> bool {
    egui::Popup::is_any_open(ctx) && (!egui::Popup::is_id_open(ctx, panel_id()) || ctx.input(|i| i.key_pressed(Key::Escape)))
}

/// A share eased toward `f` between counts: counts come at most every SAMPLE_EVERY (200 ms), and
/// a bar jumping that often looks like stop motion. egui repaints only while it is still moving.
fn eased(ui: &Ui, key: impl std::hash::Hash + std::fmt::Debug, f: Option<f32>) -> Option<f32> {
    f.map(|f| ui.ctx().animate_value_with_time(egui::Id::unique(("ops_eased", key)), f, 0.3))
}

/// A 6px fill bar as the disk's in the status bar; `f` None: a segment runs along it (size unknown).
fn fill_bar(ui: &Ui, r: Rect, f: Option<f32>, col: Color32) {
    let p = ui.painter().with_clip_rect(r);
    p.rect_filled(r, cr(3.0), pal().track);
    let seg = match f {
        Some(f) => Rect::from_min_size(r.min, vec2(r.width() * f, r.height())),
        None => {
            let w = r.width() * 0.3;
            let x = ((ui.input(|i| i.time) * 0.7).fract() as f32) * (r.width() + w) - w;
            // about 30 frames a second is smooth for a 6px bar
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            Rect::from_min_size(pos2(r.left() + x, r.top()), vec2(w, r.height()))
        }
    };
    p.rect_filled(seg, cr(3.0), col);
}

/// The speed over the progress (x: 0–100 %), the present speed dashed, as Windows 11's copy dialog;
/// `shown` is the eased share, so the chart's edge moves on smoothly between samples.
fn chart(ui: &mut Ui, j: &Job, col: Color32, shown: Option<f32>) {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 54.0), Sense::hover());
    resp.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, format!("{}: {}", j.title, j.source));
        info.value = j.fraction().map(|f| (f as f64 * 100.0).floor());
        info
    });
    let p = ui.painter();
    p.rect(r, cr(4.0), pal().pane, Stroke::new(1.0, pal().line), StrokeKind::Inside);
    let inner = r.shrink(1.0);
    let p = p.with_clip_rect(inner);
    let vmax = j.samples.iter().map(|s| s.1).fold(j.speed as f32, f32::max).max(1.0) * 1.25;
    let y = |v: f32| inner.bottom() - 2.0 - v.min(vmax) / vmax * (inner.height() - 8.0);
    let mut pts: Vec<egui::Pos2> = j.samples.iter().map(|&(f, v)| pos2(inner.left() + f * inner.width(), y(v))).collect();
    // from the last sample on to where the bar is now, at the present speed
    if let (Some(f), Some(last), true) = (shown, pts.last().copied(), j.running()) {
        let x = inner.left() + f * inner.width();
        if x > last.x {
            pts.push(pos2(x, y(j.speed as f32)));
        }
    }
    let area = col.gamma_multiply(0.18);
    // a strip per step: egui fills only convex shapes
    for w in pts.windows(2) {
        let b = inner.bottom();
        p.add(egui::Shape::convex_polygon(vec![w[0], w[1], pos2(w[1].x, b), pos2(w[0].x, b)], area, Stroke::NONE));
    }
    if pts.len() > 1 {
        p.add(egui::Shape::line(pts, Stroke::new(1.5, col)));
    }
    if j.running() && j.speed > 0.0 {
        let cy = y(j.speed as f32);
        p.extend(egui::Shape::dashed_line(&[pos2(inner.left(), cy), pos2(inner.right(), cy)], Stroke::new(1.0, pal().dim), 3.0, 3.0));
        let font = egui::TextStyle::Small.resolve(ui.style());
        p.text(inner.right_bottom() + vec2(-6.0, -2.0), Align2::RIGHT_BOTTOM, t!("ops_panel.speed", v = fmt_speed(j.speed)), font, pal().dim);
    } else if j.total.is_none() && j.running() {
        if j.waiting(Instant::now()).is_some() {
            // waiting its turn, not working: no running bar; the countdown under it moves by the second
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        } else {
            // nothing counted yet: no chart to draw, the bar says it is working
            fill_bar(ui, Rect::from_center_size(inner.center(), vec2(inner.width() - 24.0, 6.0)), None, col);
        }
    }
}

impl App {
    /// The status bar's button for the file operations: what runs and how far, or how the last
    /// ended; a click opens the Operacje panel. Nothing while there is none.
    pub(crate) fn ops_indicator(&mut self, ui: &mut Ui) {
        let popup_id = panel_id();
        let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
        // the chart's samples are copied only for the open panel
        let jobs = self.ops.list(open);
        if jobs.is_empty() {
            egui::Popup::close_id(ui.ctx(), popup_id);
            return;
        }
        let running: Vec<&Job> = jobs.iter().filter(|j| j.running()).collect();
        let failed = jobs.iter().any(|j| matches!(j.state, State::Failed(_)));
        ui.add_space(16.0);
        let (text, frac) = match running.as_slice() {
            [] => (String::new(), None),
            [j] => (j.fraction().map_or(format!("{}…", j.title), |f| format!("{} · {}", j.title, fmt_pct(f))), j.fraction()),
            many => {
                let f = overall(&jobs);
                (f.map_or(fmt_count(many.len()), |f| format!("{} · {}", fmt_count(many.len()), fmt_pct(f))), f)
            }
        };
        let (bar_id, dot_id) = (egui::IdSalt::new("ops_bar"), egui::IdSalt::new("ops_dot"));
        let mut atoms = egui::Atoms::default();
        let label = if !running.is_empty() {
            atoms.push_right(text.as_str());
            atoms.push_right(egui::Atom::custom(bar_id, vec2(72.0, 6.0)));
            if failed {
                atoms.push_right(egui::Atom::custom(dot_id, vec2(7.0, 7.0)));
            }
            format!("{}{}", t!("ops_panel.a11y", text = text), if failed { t!("ops_panel.a11y_one_failed") } else { "" })
        } else if failed {
            atoms.push_right(icon(ic::CLOSE, 10.0).color(pal().red));
            atoms.push_right(RichText::new(t!("ops_panel.failed")).color(pal().red));
            t!("ops_panel.a11y_failed").to_string()
        } else {
            // what the last one did; its note (as before) is in the status bar too
            let t = match jobs.as_slice() {
                [j] if j.done > 0 => j.summary(),
                [j] => j.outcome(),
                many => t!("ops_panel.done_n", n = fmt_count(many.len())),
            };
            atoms.push_right(icon(ic::CHECK, 10.0).color(pal().new_text));
            atoms.push_right(t.clone());
            t!("ops_panel.a11y_done", t = t)
        };
        atoms.push_right(icon(ic::CHEV_U, 8.0).color(pal().dim));
        // the hover fill of the toolbar, kept while the panel is open (the mock-up's expanded button)
        let b = egui::Button::new(atoms).frame_when_inactive(open).fill(pal().nav_sel).truncate();
        let ar = ui.scope(|ui| {
            // a long title is cut, so the note after it keeps its place
            ui.set_max_width(ui.available_width().min(360.0));
            ui.spacing_mut().item_spacing.x = 7.0;
            b.atom_ui(ui)
        }).inner;
        if let Some(r) = ar.rect(bar_id) {
            fill_bar(ui, r, eased(ui, "indicator", frac), pal().accent);
        }
        if let Some(r) = ar.rect(dot_id) {
            ui.painter().circle_filled(r.center(), 3.5, pal().red);
        }
        let resp = ar.response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if open { t!("ops_panel.hide") } else { t!("ops_panel.show") });
        resp.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(egui::Role::Button, true, &label);
            info.value = frac.map(|f| (f as f64 * 100.0).floor());
            info
        });
        let mut close = None;
        let mut clear = false;
        egui::Popup::new(popup_id, ui.ctx().clone(), &resp, ui.layer_id())
            .open_memory(resp.clicked().then_some(egui::SetOpenCommand::Toggle))
            // one panel for the whole app; a click anywhere else in the window (a file, a tab) folds
            // it back into the status bar, as do its button and Esc
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(egui::RectAlign::TOP_START)
            .gap(6.0)
            .width(PANEL_W)
            .accessible_name(t!("ops_panel.title"))
            .frame(egui::Frame::popup(ui.style()).inner_margin(Margin::symmetric(12, 10)))
            .show(|ui| {
                ui.set_width(PANEL_W - 24.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t!("ops_panel.title")).font(FontId::new(14.0, FontFamily::Name("semibold".into()))));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if running.len() < jobs.len() {
                            let r = ui.add(egui::Button::new(RichText::new(t!("ops_panel.clear")).color(pal().accent)).frame(false));
                            clear = r.on_hover_text(t!("ops_panel.clear_tip")).clicked();
                        }
                        let n = running.len();
                        ui.label(RichText::new(if n > 0 { t!("ops_panel.running", n = n) } else { t!("ops_panel.none").into() }).color(pal().dim));
                    });
                });
                let mut card = |ui: &mut Ui, j: &Job| {
                    ui.add_space(5.0);
                    ui.separator();
                    ui.add_space(3.0);
                    close = close.or(self.op_card(ui, j));
                };
                // what runs stays in sight on top; the history from the newest scrolls under it, about
                // three finished cards (three lines each) high, so the panel does not grow with it
                for j in jobs.iter().filter(|j| j.running()) {
                    card(ui, j);
                }
                let three = 3.0 * (3.0 * ui.text_style_height(&egui::TextStyle::Body) + 28.0);
                egui::ScrollArea::vertical().id_salt("ops_history").max_height(three).auto_shrink([false, true]).show(ui, |ui| {
                    for j in jobs.iter().rev().filter(|j| !j.running()) {
                        card(ui, j);
                    }
                });
            });
        if clear {
            self.ops.clear_finished();
        }
        if let Some(id) = close {
            self.ops.remove(id);
        }
    }

    /// One job's card; returns its id when it was removed from the history (×).
    fn op_card(&self, ui: &mut Ui, j: &Job) -> Option<u64> {
        let mut close = None;
        let remove = |ui: &mut Ui, name: &str| {
            let r = ui.add(egui::Button::new(icon(ic::CLOSE, 10.0).color(pal().dim)).frame(false)).on_hover_text(t!("ops_panel.remove"));
            named(&r, egui::Role::Button, name);
            r.clicked()
        };
        if let State::Done(_) = &j.state {
            ui.horizontal_top(|ui| {
                ui.label(icon(ic::CHECK, 12.0).color(pal().new_text));
                let w = ui.available_width() - 24.0;
                ui.vertical(|ui| {
                    ui.set_width(w);
                    ui.add(egui::Label::new(j.outcome()).wrap());
                    // what it worked on, so the history says which files it was
                    if !j.source.is_empty() {
                        ui.add(egui::Label::new(RichText::new(&j.source).color(pal().dim)).truncate());
                    }
                    ui.label(RichText::new(format!("{} · {}", j.ended_clock(), j.summary())).color(pal().dim));
                });
                if remove(ui, t!("ops_panel.remove")) {
                    close = Some(j.id);
                }
            });
            return close;
        }
        let err = matches!(j.state, State::Failed(_));
        let col = if err { pal().red } else { pal().accent };
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(RichText::new(&j.title).font(FontId::new(14.0, FontFamily::Name("semibold".into())))).truncate());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if err && remove(ui, t!("ops_panel.close_error")) {
                    close = Some(j.id);
                }
                // pause and cancel, for a job that takes them (the app's own copy, once its files start)
                if let Some(c) = j.control.as_ref().filter(|c| j.running() && c.armed() && !c.cancelled()) {
                    let button = |ui: &mut Ui, code: &str, name: &str| {
                        let r = ui.add(egui::Button::new(icon(code, 12.0).color(pal().dim)).frame(false)).on_hover_text(name);
                        named(&r, egui::Role::Button, name);
                        r.clicked()
                    };
                    if button(ui, ic::CLOSE, t!("ops_panel.cancel")) {
                        c.cancel();
                    }
                    let paused = c.paused();
                    if button(ui, if paused { ic::PLAY } else { ic::PAUSE }, if paused { t!("ops_panel.resume") } else { t!("ops_panel.pause") }) {
                        c.set_paused(!paused);
                    }
                }
                if let Some(f) = j.fraction() {
                    ui.label(RichText::new(fmt_pct(f)).font(FontId::new(18.0, FontFamily::Name("semibold".into()))));
                }
            });
        });
        if !j.source.is_empty() {
            ui.add(egui::Label::new(RichText::new(&j.source).color(pal().dim)).truncate());
        }
        chart(ui, j, col, eased(ui, j.id, j.fraction()));
        if err {
            ui.horizontal_top(|ui| {
                ui.label(icon(ic::CLOSE, 10.0).color(pal().red));
                ui.add(egui::Label::new(RichText::new(j.outcome()).color(pal().red)).wrap());
            });
            ui.label(RichText::new(j.ended_clock()).color(pal().dim));
        } else {
            let s = j.status();
            let s = if j.control.as_ref().is_some_and(|c| c.armed() && c.paused() && !c.cancelled()) {
                [t!("ops_panel.paused").to_string(), s].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
            } else {
                s
            };
            if !s.is_empty() {
                ui.label(RichText::new(s).color(pal().dim));
            }
        }
        close
    }
}
