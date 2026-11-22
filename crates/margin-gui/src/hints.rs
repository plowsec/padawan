//! Code hints: outline on a background thread → `Request::Lens` → one hint line per function.
//!
//! | Segment | id | Shown when | Click |
//! |---|---|---|---|
//! | `N callers` | 1 | N > 0 | `Request::Callers` → results panel |
//! | `N callees` | 2 | N > 0 (distinct names) | call sites in the function (outline) |
//! | `sinks: a, b` | 3 | direct sink calls | those call sites |
//! | `reaches: exec, eval` | 4 | `LensInfo::reach` non-empty | `Request::Reach` |
//!
//! Pipeline per tab: revision change → 300 ms debounce → outline job (≤ 2 MiB, `Lang::has_outline`) →
//! result for a stale revision dropped → `Lens` request (hints on) → lenses replace the previous ones.

use std::sync::mpsc::Sender;
use std::sync::Arc;

use margin_code::{FnDef, Lang, Outline};
use margin_editor::{Lens, LensSeg, LensTone};
use margin_proto::{LensFn, LensInfo};

pub const MAX_OUTLINE_BYTES: usize = 2 << 20;
pub const DEBOUNCE: f64 = 0.3;
pub const SEG_CALLERS: u32 = 1;
pub const SEG_CALLEES: u32 = 2;
pub const SEG_SINKS: u32 = 3;
pub const SEG_REACH: u32 = 4;

pub struct OutlineDone {
    pub tab: u64,
    pub rev: u64,
    pub outline: Arc<Outline>,
}

/// Run `margin_code::outline` off the UI thread; the result arrives on `tx` (then `wake`).
pub fn spawn_outline(tab: u64, rev: u64, lang: Lang, text: String, tx: Sender<OutlineDone>, wake: impl Fn() + Send + 'static) {
    let _ = std::thread::Builder::new().name("margin-outline".into()).spawn(move || {
        let outline = Arc::new(margin_code::outline(lang, &text));
        let _ = tx.send(OutlineDone { tab, rev, outline });
        wake();
    });
}

/// Distinct callee names of `f`, in first-call order.
pub fn callees(f: &FnDef) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for c in &f.calls {
        if !v.iter().any(|n| n == &c.name) {
            v.push(c.name.clone());
        }
        if v.len() >= 512 {
            break;
        }
    }
    v
}

pub fn lens_request(o: &Outline) -> Vec<LensFn> {
    o.fns.iter().take(margin_proto::limits::MAX_ITEMS).map(|f| LensFn { name: f.name.clone(), callees: callees(f) }).collect()
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn list(names: &[String], max: usize) -> String {
    let mut s = names.iter().take(max).map(|n| crate::sanitize::sanitize(n, 40)).collect::<Vec<_>>().join(", ");
    if names.len() > max {
        s += &format!(" +{}", names.len() - max);
    }
    s
}

/// Hint lines for `o` with the engine's numbers (`infos` may be shorter or empty: numbers unknown).
pub fn build(o: &Outline, infos: &[LensInfo]) -> Vec<Lens> {
    let mut out = Vec::with_capacity(o.fns.len());
    for (i, f) in o.fns.iter().enumerate() {
        let info = infos.get(i);
        let name = crate::sanitize::sanitize(&f.name, 60);
        let mut segs = Vec::new();
        if let Some(inf) = info.filter(|x| x.callers > 0) {
            let mut hover = format!("Call sites of `{name}` in this folder");
            if inf.defs > 1 {
                hover += &format!("\n`{name}` is defined {} times — counts are by name", inf.defs);
            }
            segs.push(LensSeg { text: plural(inf.callers, "caller", "callers"), tone: LensTone::Normal, id: SEG_CALLERS, hover });
        }
        let names = callees(f);
        if !names.is_empty() {
            let n = names.len() as u32;
            let hover = match info {
                Some(inf) => format!("{} of {n} resolve to a definition in this folder", inf.callees.min(n)),
                None => "Calls made by this function".into(),
            };
            segs.push(LensSeg { text: plural(n, "callee", "callees"), tone: LensTone::Normal, id: SEG_CALLEES, hover });
        }
        let mut sinks: Vec<(String, &'static str)> = Vec::new();
        for c in &f.calls {
            if let Some(k) = c.sink {
                if !sinks.iter().any(|(n, _)| n == &c.name) {
                    sinks.push((c.name.clone(), k.label()));
                }
            }
        }
        if !sinks.is_empty() {
            let names: Vec<String> = sinks.iter().map(|(n, _)| n.clone()).collect();
            let hover = sinks.iter().map(|(n, k)| format!("{} — {k}", crate::sanitize::sanitize(n, 60))).collect::<Vec<_>>().join("\n");
            segs.push(LensSeg { text: format!("sinks: {}", list(&names, 4)), tone: LensTone::Sink, id: SEG_SINKS, hover });
        }
        if let Some(inf) = info.filter(|x| !x.reach.is_empty()) {
            let reach: Vec<String> = inf.reach.iter().take(32).cloned().collect();
            let mut text = format!("reaches: {}", list(&reach, 4));
            if inf.reach_more {
                text += " …";
            }
            segs.push(LensSeg { text, tone: LensTone::Sink, id: SEG_REACH, hover: "Sinks reachable through calls from here".into() });
        }
        if !segs.is_empty() {
            out.push(Lens { line: f.line, segs });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_code::{Call, SinkKind};

    fn call(name: &str, sink: Option<SinkKind>) -> Call {
        Call { name: name.into(), qualified: String::new(), line: 3, col: 4, sink }
    }

    #[test]
    fn segments_only_with_something_to_say() {
        let o = Outline {
            fns: vec![
                FnDef { name: "parse".into(), line: 2, col: 3, start: 2, end: 9, calls: vec![call("memcpy", Some(SinkKind::Memory)), call("helper", None), call("memcpy", Some(SinkKind::Memory)), call("system", Some(SinkKind::Exec))] },
                FnDef { name: "empty".into(), line: 12, col: 0, start: 12, end: 13, calls: vec![] },
            ],
        };
        let req = lens_request(&o);
        assert_eq!(req[0].callees, vec!["memcpy", "helper", "system"]);
        let infos = vec![LensInfo { callers: 2, defs: 3, callees: 1, reach: vec!["exec".into()], reach_more: true }, LensInfo::default()];
        let l = build(&o, &infos);
        assert_eq!(l.len(), 1, "no hint for a function with nothing to say");
        let texts: Vec<&str> = l[0].segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["2 callers", "3 callees", "sinks: memcpy, system", "reaches: exec …"]);
        assert!(l[0].segs[0].hover.contains("defined 3 times"));
        assert!(l[0].segs[2].hover.contains("memory"));
        // Before the reply: local numbers only.
        let l = build(&o, &[]);
        assert_eq!(l[0].segs.len(), 2);
        let many: Vec<String> = (0..7).map(|i| format!("s{i}")).collect();
        assert_eq!(list(&many, 4), "s0, s1, s2, s3 +3");
    }
}
