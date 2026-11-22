//! Paths view (bottom panel): call paths from a function to sink calls, with heuristic argument flow
//! (`Request::Paths` → `Reply::Paths`). Everything is sanitised and capped once, in `set`.
//!
//! | Row | Shows |
//! |---|---|
//! | group | sink (sink tone) · kind · chain `a → b → memcpy` · `input reaches sink` / `no tracked input` |
//! | hop | number, `path:line`, enclosing function, source line with the callee highlighted |
//! | args | `arg ← from1, from2 · arg2 · …` (args without a tracked input muted) |
//!
//! Click / Enter on a hop opens the call site; Up / Down move across hops; a group row collapses.

use std::collections::HashSet;

use egui::{pos2, vec2, Color32, CursorIcon, FontId, Id, Rect, Sense, Ui};
use margin_proto::limits::{MAX_ARGS, MAX_HOPS};
use margin_proto::{CallPath, Site};
use margin_theme::Theme;

use crate::results_panel::chrome;
use crate::sanitize::{clean, floor_char, sanitize, split3};
use crate::search_panel::thousands;
use crate::widgets;

/// Paths kept from one reply.
pub const MAX_PATHS: usize = 1000;
/// Bytes of a hop's source line kept.
const MAX_LINE: usize = 2048;

/// Tone of a run in the args line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Faint,
    Normal,
}

#[derive(Clone, Debug)]
pub struct PHop {
    pub site: Site,
    pub loc: String,
    pub func: String,
    pub args: Vec<(String, Tone)>,
}

#[derive(Clone, Debug)]
pub struct PPath {
    pub sink: String,
    pub kind: String,
    pub chain: String,
    pub flows: bool,
    pub hops: Vec<PHop>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PRow {
    Header(usize),
    Hop(usize, usize),
    Args(usize, usize),
}

pub struct PathsPanel {
    /// Function the paths start from (sanitised).
    pub from: String,
    pub paths: Vec<PPath>,
    pub truncated: bool,
    pub loading: bool,
    pub height: f32,
    pub focus: bool,
    collapsed: HashSet<usize>,
    rows: Vec<PRow>,
    rows_dirty: bool,
    /// (path, hop).
    pub selected: Option<(usize, usize)>,
    reveal: bool,
}

pub enum PathsAction {
    Jump(Site),
    Close,
}

pub fn panel_id() -> Id {
    Id::unique("paths-panel")
}

/// Callee text of a hop (the highlighted part of its line).
fn callee(s: &Site) -> &str {
    split3(&s.text, s.text_col, s.len).1
}

fn arg_runs(args: &[margin_proto::FlowArg]) -> Vec<(String, Tone)> {
    let mut out: Vec<(String, Tone)> = Vec::new();
    for (i, a) in args.iter().take(MAX_ARGS).enumerate() {
        if i > 0 {
            out.push(("  ·  ".into(), Tone::Faint));
        }
        let text = sanitize(a.text.trim(), 48);
        let text = if text.is_empty() { "…".to_owned() } else { text };
        let from: Vec<String> = a.from.iter().take(MAX_ARGS).map(|f| sanitize(f, 40)).filter(|f| !f.is_empty()).collect();
        if from.is_empty() {
            out.push((text, Tone::Faint));
        } else {
            out.push((text, Tone::Muted));
            out.push((" ← ".into(), Tone::Faint));
            out.push((from.join(", "), Tone::Normal));
        }
    }
    if args.len() > MAX_ARGS {
        out.push(("  ·  …".into(), Tone::Faint));
    }
    if out.is_empty() {
        out.push(("arguments not read".into(), Tone::Faint));
    }
    out
}

/// Sanitise and cap one path from the engine.
pub fn clean_path(p: CallPath) -> PPath {
    let hops: Vec<PHop> = p
        .hops
        .into_iter()
        .take(MAX_HOPS)
        .map(|h| {
            let mut site = h.site;
            if site.text.len() > MAX_LINE {
                let cut = floor_char(&site.text, MAX_LINE);
                site.text.truncate(cut);
            }
            let loc = format!("{}:{}", sanitize(site.path.as_str(), 120), u64::from(site.line) + 1);
            let func = sanitize(&site.context, 60);
            let args = arg_runs(&h.args);
            PHop { site, loc, func, args }
        })
        .collect();
    let mut chain: Vec<String> = Vec::new();
    if let Some(first) = hops.first() {
        if !first.func.is_empty() {
            chain.push(first.func.clone());
        }
    }
    for h in &hops {
        let c = sanitize(callee(&h.site).trim(), 40);
        chain.push(if c.is_empty() { "?".into() } else { c });
    }
    PPath { sink: sanitize(&p.sink, 60), kind: sanitize(&p.kind, 40), chain: sanitize(&chain.join(" → "), 400), flows: p.flows, hops }
}

impl PathsPanel {
    pub fn new(from: &str, height: f32) -> PathsPanel {
        PathsPanel { from: sanitize(from, 60), paths: Vec::new(), truncated: false, loading: true, height, focus: true, collapsed: HashSet::new(), rows: Vec::new(), rows_dirty: true, selected: None, reveal: false }
    }

    pub fn set(&mut self, paths: Vec<CallPath>, truncated: bool) {
        let truncated = truncated || paths.len() > MAX_PATHS;
        self.paths = paths.into_iter().take(MAX_PATHS).map(clean_path).collect();
        self.truncated = truncated;
        self.loading = false;
        self.collapsed.clear();
        self.rows_dirty = true;
        self.selected = self.paths.iter().position(|p| !p.hops.is_empty()).map(|p| (p, 0));
    }

    pub fn header(&self) -> String {
        let n = self.paths.len() as u64;
        let mut s = format!("Paths from {} to sinks", self.from);
        if self.loading {
            s += " · …";
        } else if self.truncated {
            s += &format!(" · first {} paths", thousands(n));
        } else {
            s += &format!(" · {}", thousands(n));
        }
        s
    }

    pub fn hops(&self) -> usize {
        self.paths.iter().map(|p| p.hops.len()).sum()
    }

    fn rebuild(&mut self) {
        self.rows_dirty = false;
        self.rows.clear();
        for (i, p) in self.paths.iter().enumerate() {
            self.rows.push(PRow::Header(i));
            if !self.collapsed.contains(&i) {
                for h in 0..p.hops.len() {
                    self.rows.push(PRow::Hop(i, h));
                    self.rows.push(PRow::Args(i, h));
                }
            }
        }
    }

    /// Visible hops in order.
    fn hop_rows(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.rows.iter().filter_map(|r| if let PRow::Hop(p, h) = r { Some((*p, *h)) } else { None })
    }

    fn step(&mut self, d: isize) {
        let v: Vec<(usize, usize)> = self.hop_rows().collect();
        if v.is_empty() {
            return;
        }
        let cur = self.selected.and_then(|s| v.iter().position(|x| *x == s));
        let i = match cur {
            None => 0,
            Some(c) => (c as isize + d).clamp(0, v.len() as isize - 1) as usize,
        };
        self.selected = Some(v[i]);
        self.reveal = true;
    }

    fn site(&self, sel: (usize, usize)) -> Option<Site> {
        self.paths.get(sel.0)?.hops.get(sel.1).map(|h| h.site.clone())
    }

    /// Selected hop as `path:line` (tests).
    pub fn selected_loc(&self) -> Option<String> {
        let (p, h) = self.selected?;
        self.paths.get(p)?.hops.get(h).map(|x| x.loc.clone())
    }

    pub fn show(&mut self, ui: &mut Ui, rect: Rect, t: &Theme) -> Option<PathsAction> {
        if self.rows_dirty {
            self.rebuild();
        }
        let mut act = None;
        let id = panel_id();
        let header = self.header();
        let runs = [(header.as_str(), t.font_small(), t.ui.text_muted), ("    argument flow is heuristic", t.font_small(), t.ui.text_faint)];
        let c = chrome(ui, rect, t, id, &mut self.height, &mut self.focus, &runs);
        if let Some(k) = c.keys {
            if k.down {
                self.step(1);
            }
            if k.up {
                self.step(-1);
            }
            if k.enter {
                if let Some(s) = self.selected.and_then(|s| self.site(s)) {
                    act = Some(PathsAction::Jump(s));
                }
            }
            if k.esc {
                act = Some(PathsAction::Close);
            }
        }
        if c.close {
            act = Some(PathsAction::Close);
        }
        let row_h = t.row_h();
        let list = c.list;
        if self.paths.is_empty() && !self.loading {
            let r = Rect::from_min_size(list.min, vec2(list.width(), row_h));
            widgets::text_at(ui, r, r.left() + 12.0, "No path to a sink", t.font_small(), t.ui.text_faint);
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
        child.set_clip_rect(list);
        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let reveal = std::mem::take(&mut self.reveal).then(|| self.selected.and_then(|s| self.rows.iter().position(|r| *r == PRow::Hop(s.0, s.1)))).flatten();
        let sink = t.code.lens_sink;
        let loc_w = 200.0f32.min(list.width() * 0.25);
        let fn_w = 150.0f32.min(list.width() * 0.18);
        let mut clicked: Option<PRow> = None;
        let sel = self.selected;
        widgets::virtual_rows(&mut child, id.with("scroll"), self.rows.len(), row_h, reveal, |ui, i, r| {
            let row = self.rows[i];
            let resp = ui.interact(r, id.with(("row", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            match row {
                PRow::Header(pi) => {
                    let p = &self.paths[pi];
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 0.0, t.ui.row_hover);
                    }
                    widgets::chevron(ui.painter(), pos2(r.left() + 16.0, r.center().y), !self.collapsed.contains(&pi), t.ui.text_muted);
                    let (mark, mc) = if p.flows { ("input reaches sink", sink) } else { ("no tracked input", t.ui.text_faint) };
                    let mw = widgets::text_right(ui, r, r.right() - 12.0, mark, t.font_small(), mc);
                    let runs: [(&str, FontId, Color32); 5] = [(&p.sink, t.font_mono_ui(), sink), ("  ", t.font_small(), t.ui.text_muted), (&p.kind, t.font_small(), t.ui.text_muted), ("    ", t.font_small(), t.ui.text_muted), (&p.chain, t.font_small(), t.ui.text)];
                    widgets::paint_job(ui, r.with_max_x(r.right() - 24.0 - mw), r.left() + 28.0, &runs);
                }
                PRow::Hop(pi, hi) | PRow::Args(pi, hi) => {
                    let h = &self.paths[pi].hops[hi];
                    let selected = sel == Some((pi, hi));
                    let hovered = resp.hovered() || ui.rect_contains_pointer(if matches!(row, PRow::Hop(..)) { r.translate(vec2(0.0, row_h)) } else { r.translate(vec2(0.0, -row_h)) });
                    widgets::row_bg(ui.painter(), r, t, hovered, selected);
                    let x0 = r.left() + 28.0;
                    if matches!(row, PRow::Hop(..)) {
                        widgets::text_right(ui, r, x0 + 16.0, &format!("{}", hi + 1), t.font_mono_ui(), t.ui.text_faint);
                        let lx = x0 + 28.0;
                        widgets::text_at(ui, r.with_max_x(lx + loc_w - 12.0), lx, &h.loc, t.font_mono_ui(), t.ui.text_muted);
                        let fx = lx + loc_w;
                        widgets::text_at(ui, r.with_max_x(fx + fn_w - 12.0), fx, &h.func, t.font_small(), t.ui.text);
                        widgets::snippet(ui, r.with_max_x(r.right() - 12.0), fx + fn_w, &h.site.text, h.site.text_col, h.site.len, t);
                    } else {
                        let parts: Vec<(&str, FontId, Color32)> = h
                            .args
                            .iter()
                            .map(|(s, tone)| {
                                let c = match tone {
                                    Tone::Normal => t.ui.text,
                                    Tone::Muted => t.ui.text_muted,
                                    Tone::Faint => t.ui.text_faint,
                                };
                                (s.as_str(), t.font_small(), c)
                            })
                            .collect();
                        widgets::paint_job(ui, r.with_max_x(r.right() - 12.0), x0 + 28.0, &parts);
                    }
                }
            }
            if resp.clicked() {
                clicked = Some(row);
            }
        });
        match clicked {
            Some(PRow::Header(pi)) => {
                if !self.collapsed.remove(&pi) {
                    self.collapsed.insert(pi);
                }
                self.rows_dirty = true;
                ui.ctx().request_repaint();
            }
            Some(PRow::Hop(p, h) | PRow::Args(p, h)) => {
                self.selected = Some((p, h));
                ui.memory_mut(|m| m.request_focus(id));
                act = self.site((p, h)).map(PathsAction::Jump);
            }
            None => {}
        }
        act
    }
}

/// Display form of a `path:line` for tests and the probe.
pub fn loc_of(s: &Site) -> String {
    format!("{}:{}", clean(s.path.as_str()), u64::from(s.line) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_proto::{FlowArg, Hop, RelPath};

    fn hop(f: &str, text: &str, col: u32, len: u32, args: Vec<FlowArg>) -> Hop {
        Hop { site: Site { path: RelPath::new("src/a.c").unwrap(), line: 4, col, len, text: text.into(), text_col: col, context: f.into() }, args }
    }

    #[test]
    fn chain_args_and_hostile_values() {
        let p = CallPath {
            sink: "memcpy".into(),
            kind: "memory".into(),
            hops: vec![
                hop("main", "parse(line, n);", 0, 5, vec![FlowArg { text: "line".into(), from: vec!["argv".into()] }, FlowArg { text: "n".into(), from: vec![] }]),
                hop("parse", "memcpy(buf, line, n);", 0, 6, vec![FlowArg { text: "buf".into(), from: vec![] }, FlowArg { text: "line".into(), from: vec!["line".into(), "x".into()] }]),
            ],
            flows: true,
        };
        let c = clean_path(p);
        assert_eq!(c.chain, "main → parse → memcpy");
        let line: String = c.hops[1].args.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(line, "buf  ·  line ← line, x");
        assert_eq!(c.hops[1].args.last().map(|a| a.1), Some(Tone::Normal));
        assert_eq!(c.hops[0].loc, "src/a.c:5");
        // Hostile: bidi, huge args, offsets out of range, multi-byte cut.
        let evil = CallPath {
            sink: "mem\u{202e}cpy".into(),
            kind: "x".repeat(10_000),
            hops: vec![hop("f\u{2066}", &"é".repeat(5000), u32::MAX, u32::MAX, (0..500).map(|i| FlowArg { text: "a".repeat(9000), from: vec![format!("p{i}\u{200b}"); 300] }).collect()); 100],
            flows: false,
        };
        let c = clean_path(evil);
        assert_eq!(c.sink, "mem·cpy");
        assert!(c.kind.chars().count() <= 40);
        assert_eq!(c.hops.len(), MAX_HOPS);
        assert!(c.hops[0].site.text.len() <= MAX_LINE);
        assert!(c.hops[0].args.len() <= MAX_ARGS * 4 + 1);
        assert!(c.hops[0].args.iter().all(|(s, _)| s.chars().count() <= 16 * 41 + 16 && !s.contains('\u{200b}')));
        assert!(!c.chain.contains('\u{2066}'));
    }

    #[test]
    fn navigation_and_collapse() {
        let mut p = PathsPanel::new("main", 200.0);
        let one = |f: &str| CallPath { sink: "system".into(), kind: "exec".into(), hops: vec![hop(f, "system(x)", 0, 6, vec![]), hop(f, "system(y)", 0, 6, vec![])], flows: false };
        p.set(vec![one("a"), one("b")], false);
        p.rebuild();
        assert_eq!(p.selected, Some((0, 0)));
        p.step(1);
        p.step(1);
        assert_eq!(p.selected, Some((1, 0)), "across groups");
        p.collapsed.insert(1);
        p.rebuild();
        p.step(1);
        assert_eq!(p.selected, Some((0, 0)), "collapsed hops are skipped (selection resets into the visible ones)");
        assert_eq!(p.header(), "Paths from main to sinks · 2");
        p.set((0..2000).map(|_| one("z")).collect(), false);
        assert_eq!(p.paths.len(), MAX_PATHS);
        assert!(p.header().ends_with("first 1,000 paths"));
    }
}
