//! Markdown preview of a tab: parse scheduling, link resolution, task toggles. Drawing is `margin_md::show`.
//!
//! | Item | Rule |
//! |---|---|
//! | parse | 150 ms after the last edit; over `BIG` bytes on a thread (the old preview stays until it lands) |
//! | `#frag` link | `MdDoc::anchor_line` → editor cursor + preview scroll |
//! | relative link | no scheme, not `/…`; `#frag` / `?query` stripped, `%20` decoded, resolved against the file's dir, `RelPath`-checked → open the file |
//! | scheme or absolute | copied to the clipboard; never opened |
//! | task box | `[ ]` ⇄ `[x]` on that source line, one undo step |
//! | copy, select all | go to the preview while it is the pane used last (`active`), else to the editor |
//! | secondary click | menu: Copy, Copy Link (on a link), Select All |
//! | file in several groups | one parsed document; on/off, scroll, selection per group (`PvView`) |

use std::sync::mpsc::Sender;
use std::sync::Arc;

use margin_md::{MdDoc, PreviewState};
use margin_proto::RelPath;

pub const DEBOUNCE: f64 = 0.15;
/// Bigger sources parse off the UI thread.
pub const BIG: usize = 256 << 10;
/// Narrower than this: the preview alone.
pub const SPLIT_MIN_W: f32 = 700.0;
/// Each side of the split at least.
pub const SIDE_MIN_W: f32 = 240.0;
const MAX_LINK: usize = 4096;

#[derive(Default)]
pub struct Preview {
    pub on: bool,
    pub doc: Option<Arc<MdDoc>>,
    /// Revision `doc` was parsed from.
    pub parsed_rev: Option<u64>,
    /// Revision of a parse job in flight.
    pub parsing: Option<u64>,
    pub seen_rev: Option<u64>,
    pub changed_at: f64,
    pub state: PreviewState,
    /// Editor top line last frame (scroll sync).
    pub editor_top: Option<u32>,
    /// Narrow window: show the editor instead of the preview.
    pub show_editor: bool,
    /// The preview had focus more recently than the editor: Copy and Select All act on it.
    pub active: bool,
}

/// The part of `Preview` each group showing the file has for itself.
#[derive(Default)]
pub struct PvView {
    pub on: bool,
    pub state: PreviewState,
    pub editor_top: Option<u32>,
    pub show_editor: bool,
    pub active: bool,
}

pub struct MdDone {
    pub tab: u64,
    pub rev: u64,
    pub doc: Arc<MdDoc>,
}

impl Preview {
    /// Revision to parse now, if the debounce is over. `None` + `Some(wait)`: ask again later.
    pub fn due(&mut self, rev: u64, now: f64) -> (bool, Option<f64>) {
        if self.seen_rev != Some(rev) {
            self.seen_rev = Some(rev);
            self.changed_at = now;
        }
        if self.parsed_rev == Some(rev) || self.parsing == Some(rev) {
            return (false, None);
        }
        let left = DEBOUNCE - (now - self.changed_at);
        // First parse: no wait.
        if self.doc.is_some() && left > 0.0 {
            return (false, Some(left));
        }
        (true, None)
    }

    /// Exchange the per-group part with `v`.
    pub fn swap_view(&mut self, v: &mut PvView) {
        std::mem::swap(&mut self.on, &mut v.on);
        std::mem::swap(&mut self.state, &mut v.state);
        std::mem::swap(&mut self.editor_top, &mut v.editor_top);
        std::mem::swap(&mut self.show_editor, &mut v.show_editor);
        std::mem::swap(&mut self.active, &mut v.active);
    }

    pub fn apply(&mut self, rev: u64, doc: Arc<MdDoc>) {
        if self.parsing == Some(rev) {
            self.parsing = None;
        }
        if self.parsed_rev.is_none_or(|r| r <= rev) {
            self.parsed_rev = Some(rev);
            self.doc = Some(doc);
        }
    }
}

/// Parse `text` on a thread; the result arrives on `tx`, then `wake`.
pub fn spawn_parse(tab: u64, rev: u64, text: String, tx: Sender<MdDone>, wake: impl Fn() + Send + 'static) {
    let _ = std::thread::Builder::new().name("margin-md".into()).spawn(move || {
        let doc = Arc::new(margin_md::parse(&text));
        let _ = tx.send(MdDone { tab, rev, doc });
        wake();
    });
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// `#fragment` in this document.
    Anchor(String),
    /// A file in the folder (+ fragment).
    File(RelPath, Option<String>),
    /// Scheme or absolute: copy only.
    External(String),
    /// Leaves the folder or not a valid path.
    Bad,
}

fn has_scheme(s: &str) -> bool {
    let Some(i) = s.find(':') else { return false };
    let head = &s[..i];
    head.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && head.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Resolve a link destination written in a Markdown file whose directory is `dir` (`None`: untitled).
pub fn resolve(dir: Option<&RelPath>, dest: &str) -> Link {
    let d = dest.trim();
    if d.is_empty() {
        return Link::Bad;
    }
    if d.len() > MAX_LINK {
        let cut = crate::sanitize::floor_char(d, MAX_LINK);
        return Link::External(d[..cut].to_owned());
    }
    if let Some(f) = d.strip_prefix('#') {
        return Link::Anchor(f.to_owned());
    }
    if has_scheme(d) || d.starts_with('/') || d.starts_with('\\') {
        return Link::External(d.to_owned());
    }
    let (rest, frag) = match d.split_once('#') {
        Some((a, f)) => (a, Some(f.to_owned()).filter(|f| !f.is_empty())),
        None => (d, None),
    };
    let rest = rest.split_once('?').map_or(rest, |(a, _)| a);
    let path = rest.replace("%20", " ");
    if path.is_empty() {
        return frag.map_or(Link::Bad, Link::Anchor);
    }
    if path.contains('\\') {
        return Link::Bad;
    }
    let Some(dir) = dir else { return Link::Bad };
    let mut parts: Vec<&str> = dir.components().collect();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Link::Bad;
                }
            }
            s => parts.push(s),
        }
    }
    match RelPath::new(&parts.join("/")) {
        Ok(p) if !p.is_root() => Link::File(p, frag),
        _ => Link::Bad,
    }
}

/// `[ ]` ⇄ `[x]` of a task item line: (byte column of the box's inside, replacement).
pub fn task_flip(line: &str) -> Option<(u32, &'static str)> {
    let b = line.as_bytes();
    let mut i = 0;
    while b.get(i).is_some_and(|c| *c == b' ' || *c == b'\t') && i < 64 {
        i += 1;
    }
    while b.get(i) == Some(&b'>') {
        i += 1;
        while b.get(i) == Some(&b' ') {
            i += 1;
        }
    }
    match b.get(i)? {
        b'-' | b'*' | b'+' => i += 1,
        c if c.is_ascii_digit() => {
            let s = i;
            while b.get(i).is_some_and(u8::is_ascii_digit) && i - s < 9 {
                i += 1;
            }
            if !matches!(b.get(i), Some(b'.' | b')')) {
                return None;
            }
            i += 1;
        }
        _ => return None,
    }
    if !matches!(b.get(i), Some(b' ' | b'\t')) {
        return None;
    }
    while matches!(b.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    if b.get(i) != Some(&b'[') || b.get(i + 2) != Some(&b']') || !matches!(b.get(i + 3), None | Some(b' ' | b'\t')) {
        return None;
    }
    let new = match b.get(i + 1)? {
        b' ' => "x",
        b'x' | b'X' => " ",
        _ => return None,
    };
    Some(((i + 1) as u32, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }

    #[test]
    fn links() {
        let d = p("docs/guide");
        assert_eq!(resolve(Some(&d), "#install-it"), Link::Anchor("install-it".into()));
        assert_eq!(resolve(Some(&d), "../README.md"), Link::File(p("docs/README.md"), None));
        assert_eq!(resolve(Some(&d), "./a%20b.md#top"), Link::File(p("docs/guide/a b.md"), Some("top".into())));
        assert_eq!(resolve(Some(&d), "x.md?plain=1#L3"), Link::File(p("docs/guide/x.md"), Some("L3".into())));
        assert_eq!(resolve(Some(&RelPath::root()), "a/../b.md"), Link::File(p("b.md"), None));
        assert_eq!(resolve(Some(&d), "../../../etc/passwd"), Link::Bad);
        assert_eq!(resolve(Some(&RelPath::root()), ".."), Link::Bad);
        assert_eq!(resolve(None, "a.md"), Link::Bad, "untitled: nothing is relative");
        for ext in ["https://example.com/x", "mailto:a@b.c", "file:///etc/passwd", "/etc/passwd", "C:\\x", "javascript:alert(1)", "\\\\host\\share"] {
            assert!(matches!(resolve(Some(&d), ext), Link::External(_)), "{ext}");
        }
        assert_eq!(resolve(Some(&d), "a\\b.md"), Link::Bad);
        assert_eq!(resolve(Some(&d), "?q"), Link::Bad);
        assert_eq!(resolve(Some(&d), ""), Link::Bad);
        let long = "é".repeat(5000);
        assert!(matches!(resolve(Some(&d), &long), Link::External(s) if s.len() <= MAX_LINK));
    }

    #[test]
    fn task_boxes() {
        assert_eq!(task_flip("- [ ] buy milk"), Some((3, "x")));
        assert_eq!(task_flip("  * [x] done"), Some((5, " ")));
        assert_eq!(task_flip("12. [X]"), Some((5, " ")));
        assert_eq!(task_flip("> - [ ] quoted"), Some((5, "x")));
        assert_eq!(task_flip("- [y] no"), None);
        assert_eq!(task_flip("-[ ] no"), None);
        assert_eq!(task_flip("[ ] no"), None);
        assert_eq!(task_flip("- [ ]x"), None);
        assert_eq!(task_flip(""), None);
        assert_eq!(task_flip("- é"), None);
        assert_eq!(task_flip(&"1".repeat(100)), None);
    }

    #[test]
    fn debounce() {
        let mut pv = Preview::default();
        assert_eq!(pv.due(1, 0.0), (true, None), "first parse at once");
        pv.parsing = Some(1);
        pv.apply(1, Arc::new(MdDoc::default()));
        assert_eq!(pv.due(1, 0.1), (false, None));
        let (go, wait) = pv.due(2, 1.0);
        assert!(!go && wait.is_some());
        assert_eq!(pv.due(2, 1.2), (true, None));
    }
}
