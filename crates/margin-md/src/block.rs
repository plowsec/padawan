//! Block structure: one pass over lines, explicit container stack (CommonMark algorithm, simplified).
//!
//! Per line: match open containers → continue literal leaves (fence, HTML, indented code) → blank line →
//! lazy paragraph continuation → open new containers → leaf. No recursion; every step is O(line length)
//! times the (capped) depth.
//!
//! | Cap | Value |
//! |---|---|
//! | container depth (quotes, items, footnotes) | `MAX_DEPTH` (deeper markers stay text) |
//! | leaves / containers | `MAX_LEAVES` / `MAX_CTRS` (rest of the input dropped, `truncated`) |
//! | paragraph text | `PARA_CAP` bytes |
//! | table columns | `MAX_COLS` |

use std::collections::HashMap;

use margin_code::Lang;

use crate::doc::{Alert, Align, Ctr, CtrKind, NONE};
use crate::text::{cap, expand_tabs, from, indent_of, is_blank, norm_label};

pub const MAX_DEPTH: usize = 16;
pub const MAX_LEAVES: usize = 200_000;
pub const MAX_CTRS: usize = 200_000;
pub const PARA_CAP: usize = 128 * 1024;
pub const MAX_COLS: usize = 64;
const CELL_CAP: usize = 4096;

#[derive(Debug)]
pub enum RawBody {
    Para(String),
    Heading(u8, String),
    Title(Alert),
    Empty,
    Code { info: String, lines: Vec<String>, muted: bool, lang: Option<Lang> },
    Table { aligns: Vec<Align>, rows: Vec<Vec<String>> },
    Rule,
}

#[derive(Debug)]
pub struct RawLeaf {
    pub line: u32,
    pub ctr: u32,
    pub body: RawBody,
}

#[derive(Debug, Default)]
pub struct Blocks {
    pub leaves: Vec<RawLeaf>,
    pub ctrs: Vec<Ctr>,
    /// Normalised label → (dest, title).
    pub refs: HashMap<String, (String, String)>,
    /// Normalised footnote label → number (definition order).
    pub feet: HashMap<String, u32>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HtmlEnd {
    Tag(&'static str),
    Comment,
    Pi,
    Decl,
    Cdata,
    Blank,
}

#[derive(Debug)]
enum OK {
    Quote,
    List { ch: u8, ordered: bool, blank: bool, loose: bool, opened: u32, next: u64, max: u64 },
    Item { indent: usize },
    Foot,
}

#[derive(Debug)]
struct Open {
    kind: OK,
    ctr: u32,
    has_content: bool,
    line: u32,
}

#[derive(Debug, Default)]
enum OLeaf {
    #[default]
    None,
    Para { text: String, line: u32, last_start: usize, nlines: u32 },
    Fenced { ch: u8, len: usize, indent: usize, info: String, lines: Vec<String>, line: u32 },
    Indented { lines: Vec<String>, line: u32, blanks: usize },
    Html { end: HtmlEnd, lines: Vec<String>, line: u32 },
    Table { rows: Vec<Vec<String>>, aligns: Vec<Align>, line: u32 },
}

struct P {
    stack: Vec<Open>,
    leaf: OLeaf,
    out: Blocks,
    line_no: u32,
    stop: bool,
}

pub fn parse_blocks(text: &str) -> Blocks {
    let mut p = P { stack: Vec::new(), leaf: OLeaf::None, out: Blocks::default(), line_no: 0, stop: false };
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).peekable();
    let mut skip = front_matter(text, &mut p);
    let mut n: u32 = 0;
    while let Some(l) = lines.next() {
        if lines.peek().is_none() && l.is_empty() {
            break;
        }
        if skip > 0 {
            skip -= 1;
            n += 1;
            continue;
        }
        if p.stop {
            break;
        }
        p.line_no = n;
        let l = expand_tabs(l);
        p.line(&l);
        n = n.saturating_add(1);
    }
    p.line_no = n;
    p.close_to(0);
    p.out
}

/// YAML front matter at the very top: emitted as a muted code block. Returns lines consumed.
fn front_matter(text: &str, p: &mut P) -> usize {
    let mut it = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    if it.next().map(str::trim_end) != Some("---") {
        return 0;
    }
    let mut lines = Vec::new();
    for (i, l) in it.enumerate().take(2000) {
        let t = l.trim_end();
        if t == "---" || t == "..." {
            p.emit(RawBody::Code { info: "front matter".into(), lines, muted: true, lang: Some(Lang::Yaml) }, 0);
            return i + 2;
        }
        lines.push(expand_tabs(l).into_owned());
    }
    0
}

struct Marker {
    ordered: bool,
    ch: u8,
    num: u64,
    len: usize,
}

fn list_marker(t: &str) -> Option<Marker> {
    let b = t.as_bytes();
    let b0 = *b.first()?;
    if matches!(b0, b'-' | b'+' | b'*') {
        return (b.len() == 1 || b[1] == b' ').then_some(Marker { ordered: false, ch: b0, num: 0, len: 1 });
    }
    let d = b.iter().take(10).take_while(|c| c.is_ascii_digit()).count();
    if d == 0 || d > 9 {
        return None;
    }
    let delim = *b.get(d)?;
    if delim != b'.' && delim != b')' {
        return None;
    }
    if b.len() > d + 1 && b[d + 1] != b' ' {
        return None;
    }
    let num = t.get(..d)?.parse().ok()?;
    Some(Marker { ordered: true, ch: delim, num, len: d + 1 })
}

fn is_hr(t: &str) -> bool {
    let b = t.as_bytes();
    let Some(&c) = b.first() else { return false };
    if !matches!(c, b'*' | b'-' | b'_') {
        return false;
    }
    let mut n = 0;
    for &x in b {
        if x == c {
            n += 1;
        } else if x != b' ' {
            return false;
        }
    }
    n >= 3
}

fn setext(t: &str) -> Option<u8> {
    let s = t.trim_end();
    let c = *s.as_bytes().first()?;
    if (c == b'=' || c == b'-') && s.bytes().all(|x| x == c) {
        return Some(if c == b'=' { 1 } else { 2 });
    }
    None
}

fn atx(t: &str) -> Option<(u8, &str)> {
    let n = t.bytes().take(7).take_while(|&b| b == b'#').count();
    if n == 0 || n > 6 {
        return None;
    }
    let rest = from(t, n);
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let mut c = rest.trim();
    let stripped = c.trim_end_matches('#');
    if stripped.is_empty() {
        c = "";
    } else if stripped.len() < c.len() && stripped.ends_with(' ') {
        c = stripped.trim_end();
    }
    Some((n as u8, c))
}

fn fence_open(t: &str) -> Option<(u8, usize, &str)> {
    let c = *t.as_bytes().first()?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let n = t.bytes().take_while(|&b| b == c).count();
    if n < 3 {
        return None;
    }
    let info = from(t, n).trim();
    if c == b'`' && info.contains('`') {
        return None;
    }
    Some((c, n, info))
}

fn closing_fence(t: &str, ch: u8, len: usize) -> bool {
    let n = t.bytes().take_while(|&b| b == ch).count();
    n >= len && is_blank(from(t, n))
}

const BLOCK_TAGS: &[&str] = &[
    "address", "article", "aside", "base", "basefont", "blockquote", "body", "caption", "center", "col",
    "colgroup", "dd", "details", "dialog", "dir", "div", "dl", "dt", "fieldset", "figcaption", "figure",
    "footer", "form", "frame", "frameset", "h1", "h2", "h3", "h4", "h5", "h6", "head", "header", "hr",
    "html", "iframe", "legend", "li", "link", "main", "menu", "menuitem", "nav", "noframes", "ol",
    "optgroup", "option", "p", "param", "search", "section", "summary", "table", "tbody", "td", "tfoot",
    "th", "thead", "title", "tr", "track", "ul",
];

fn html_start(t: &str, interrupt: bool) -> Option<HtmlEnd> {
    let b = t.as_bytes();
    if b.first() != Some(&b'<') {
        return None;
    }
    let head: String = t.chars().take(12).collect::<String>().to_ascii_lowercase();
    for (tag, end) in [("<script", "</script>"), ("<pre", "</pre>"), ("<style", "</style>"), ("<textarea", "</textarea>")] {
        if let Some(r) = head.strip_prefix(tag) {
            if r.is_empty() || r.starts_with([' ', '>']) {
                return Some(HtmlEnd::Tag(end));
            }
        }
    }
    if head.starts_with("<!--") {
        return Some(HtmlEnd::Comment);
    }
    if head.starts_with("<?") {
        return Some(HtmlEnd::Pi);
    }
    if head.starts_with("<![cdata[") {
        return Some(HtmlEnd::Cdata);
    }
    if head.starts_with("<!") && b.get(2).is_some_and(u8::is_ascii_alphabetic) {
        return Some(HtmlEnd::Decl);
    }
    let close = b.get(1) == Some(&b'/');
    let start = if close { 2 } else { 1 };
    let name_len = b.iter().skip(start).take(16).take_while(|c| c.is_ascii_alphanumeric() || **c == b'-').count();
    if name_len == 0 || !b.get(start).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let name = t.get(start..start + name_len)?.to_ascii_lowercase();
    let after = from(t, start + name_len);
    if BLOCK_TAGS.contains(&name.as_str()) && (after.is_empty() || after.starts_with([' ', '>']) || after.starts_with("/>")) {
        return Some(HtmlEnd::Blank);
    }
    // Type 7: a lone complete tag on the line.
    if !interrupt && (after.is_empty() || after.starts_with([' ', '>', '/'])) {
        let s = t.trim_end();
        if s.ends_with('>') && !from(s, 1).contains('<') {
            return Some(HtmlEnd::Blank);
        }
    }
    None
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    n.is_empty() || h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

fn html_ends(end: HtmlEnd, line: &str) -> bool {
    match end {
        HtmlEnd::Tag(t) => contains_ci(line, t),
        HtmlEnd::Comment => line.contains("-->"),
        HtmlEnd::Pi => line.contains("?>"),
        HtmlEnd::Decl => line.contains('>'),
        HtmlEnd::Cdata => line.contains("]]>"),
        HtmlEnd::Blank => false,
    }
}

/// `[^label]:` at the start of `t`: (normalised label, bytes up to the content).
fn foot_def(t: &str) -> Option<(String, usize)> {
    let r = t.strip_prefix("[^")?;
    let end = r.bytes().take(201).position(|b| b == b']')?;
    let label = r.get(..end)?;
    if label.is_empty() || label.bytes().any(|b| b == b' ' || b == b'[') {
        return None;
    }
    if r.as_bytes().get(end + 1) != Some(&b':') {
        return None;
    }
    let mut k = 2 + end + 2;
    k += indent_of(from(t, k));
    Some((norm_label(label), k))
}

fn task_marker(r: &str) -> Option<(bool, usize)> {
    let b = r.as_bytes();
    if b.len() < 3 || b[0] != b'[' || b[2] != b']' {
        return None;
    }
    let checked = match b[1] {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    match b.get(3) {
        None => Some((checked, 3)),
        Some(b' ') => Some((checked, 4)),
        _ => None,
    }
}

/// Split a table row on unescaped pipes; `\|` → `|`. At most `MAX_COLS` cells.
pub fn split_cells(line: &str) -> Vec<String> {
    let mut s = line.trim();
    s = s.strip_prefix('|').unwrap_or(s);
    if s.ends_with('|') && !s.ends_with("\\|") {
        s = s.get(..s.len() - 1).unwrap_or("");
    }
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' && it.peek() == Some(&'|') {
            it.next();
            if cur.len() < CELL_CAP {
                cur.push('|');
            }
        } else if c == '|' {
            cells.push(std::mem::take(&mut cur).trim().to_owned());
            if cells.len() >= MAX_COLS {
                return cells;
            }
        } else if cur.len() < CELL_CAP {
            cur.push(c);
        }
    }
    cells.push(cur.trim().to_owned());
    cells.truncate(MAX_COLS);
    cells
}

fn delim_row(t: &str) -> Option<Vec<Align>> {
    if !t.contains('|') {
        return None;
    }
    let cells = split_cells(t);
    let mut out = Vec::with_capacity(cells.len());
    for c in &cells {
        let l = c.starts_with(':');
        let r = c.ends_with(':') && c.len() > 1;
        let mid = c.trim_start_matches(':').trim_end_matches(':');
        if mid.is_empty() || !mid.bytes().all(|b| b == b'-') {
            return None;
        }
        out.push(match (l, r) {
            (true, true) => Align::Center,
            (true, false) => Align::Left,
            (false, true) => Align::Right,
            (false, false) => Align::None,
        });
    }
    (!out.is_empty()).then_some(out)
}

/// Backslash escapes and entities resolved (link destinations, titles, info strings).
pub fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        let c = b[i];
        if c == b'\\' && b.get(i + 1).is_some_and(u8::is_ascii_punctuation) {
            o.push(b[i + 1] as char);
            i += 2;
        } else if c == b'&' {
            if let Some((ch, n)) = crate::text::entity(from(s, i)) {
                o.push(ch);
                i += n;
            } else {
                o.push('&');
                i += 1;
            }
        } else {
            let j = s.get(i..).and_then(|r| r.find(['\\', '&'])).map_or(b.len(), |k| i + k.max(1));
            o.push_str(s.get(i..j).unwrap_or(""));
            i = j;
        }
    }
    o
}

/// Link destination at the start of `s`: (raw dest, bytes consumed). Bounded scan.
pub fn link_dest(s: &str) -> Option<(&str, usize)> {
    let b = s.as_bytes();
    if b.first() == Some(&b'<') {
        let mut i = 1;
        while i < b.len() && i < 4096 {
            match b[i] {
                b'>' => return Some((s.get(1..i)?, i + 1)),
                b'<' | b'\n' => return None,
                b'\\' => i += 2,
                _ => i += 1,
            }
        }
        return None;
    }
    let mut depth = 0u32;
    let mut i = 0;
    while i < b.len() && i < 4096 {
        match b[i] {
            b'\\' if b.get(i + 1).is_some_and(u8::is_ascii_punctuation) => i += 2,
            b'(' => {
                depth += 1;
                if depth > 32 {
                    return None;
                }
                i += 1;
            }
            b')' if depth == 0 => break,
            b')' => {
                depth -= 1;
                i += 1;
            }
            c if c <= b' ' => break,
            _ => i += 1,
        }
    }
    if i == 0 || depth != 0 || i >= 4096 {
        return None;
    }
    let i = i.min(b.len());
    Some((s.get(..i)?, i))
}

/// Link title at the start of `s` (`"…"`, `'…'`, `(…)`): (raw title, bytes consumed). Bounded scan.
pub fn link_title(s: &str) -> Option<(&str, usize)> {
    let b = s.as_bytes();
    let close = match b.first()? {
        b'"' => b'"',
        b'\'' => b'\'',
        b'(' => b')',
        _ => return None,
    };
    let mut i = 1;
    while i < b.len() && i < 4096 {
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if c == close {
            return Some((s.get(1..i)?, i + 1));
        }
        if close == b')' && c == b'(' {
            return None;
        }
        if c == b'\n' && b.get(i + 1) == Some(&b'\n') {
            return None;
        }
        i += 1;
    }
    None
}

fn skip_ws_one_nl(s: &str, mut i: usize) -> (usize, bool) {
    let b = s.as_bytes();
    let mut nl = false;
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' => i += 1,
            b'\n' if !nl => {
                nl = true;
                i += 1;
            }
            _ => break,
        }
    }
    (i, nl)
}

/// Reference definition at the start of paragraph text: (label, dest, title, bytes consumed).
fn ref_def(s: &str) -> Option<(String, String, String, usize)> {
    let b = s.as_bytes();
    let mut i = indent_of(s);
    if i > 3 || b.get(i) != Some(&b'[') {
        return None;
    }
    i += 1;
    let ls = i;
    while i < b.len() && i - ls <= 999 {
        match b[i] {
            b'\\' => i += 2,
            b'[' => return None,
            b']' => break,
            _ => i += 1,
        }
    }
    if b.get(i) != Some(&b']') || i - ls > 999 {
        return None;
    }
    let label = s.get(ls..i)?;
    if label.trim().is_empty() || label.starts_with('^') {
        return None;
    }
    i += 1;
    if b.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    let (j, _) = skip_ws_one_nl(s, i);
    let (dest, n) = link_dest(from(s, j))?;
    let dest_end = j + n;
    // With a title?
    let (k, nl) = skip_ws_one_nl(s, dest_end);
    if (k > dest_end || nl) && k < b.len() {
        if let Some((title, tn)) = link_title(from(s, k)) {
            let e = k + tn;
            let rest = from(s, e);
            let eol = rest.find('\n').unwrap_or(rest.len());
            if is_blank(rest.get(..eol).unwrap_or("")) {
                let consumed = (e + eol + 1).min(s.len());
                return Some((norm_label(label), unescape(dest), unescape(title), consumed));
            }
        }
    }
    let rest = from(s, dest_end);
    let eol = rest.find('\n').unwrap_or(rest.len());
    if !is_blank(rest.get(..eol).unwrap_or("")) {
        return None;
    }
    let consumed = (dest_end + eol + 1).min(s.len());
    Some((norm_label(label), unescape(dest), String::new(), consumed))
}

impl P {
    fn depth(&self) -> usize {
        self.stack.iter().filter(|o| !matches!(o.kind, OK::List { .. })).count()
    }

    fn cur_ctr(&self) -> u32 {
        self.stack.last().map_or(NONE, |o| o.ctr)
    }

    fn emit(&mut self, body: RawBody, line: u32) {
        if self.out.leaves.len() >= MAX_LEAVES {
            self.stop = true;
            self.out.truncated = true;
            return;
        }
        for o in self.stack.iter_mut().rev() {
            if o.has_content {
                break;
            }
            o.has_content = true;
        }
        let ctr = self.cur_ctr();
        self.out.leaves.push(RawLeaf { line, ctr, body });
    }

    fn push_ctr(&mut self, kind: OK, ck: CtrKind) -> bool {
        if self.out.ctrs.len() >= MAX_CTRS {
            self.stop = true;
            self.out.truncated = true;
            return false;
        }
        let id = self.out.ctrs.len() as u32;
        self.out.ctrs.push(Ctr { kind: ck, parent: self.cur_ctr(), first_leaf: NONE });
        self.stack.push(Open { kind, ctr: id, has_content: false, line: self.line_no });
        true
    }

    fn close_leaf(&mut self) {
        match std::mem::take(&mut self.leaf) {
            OLeaf::None => {}
            OLeaf::Para { text, line, .. } => {
                let rest = self.strip_refs(&text);
                let rest = rest.trim_end();
                if !rest.is_empty() {
                    self.emit(RawBody::Para(rest.to_owned()), line);
                }
            }
            OLeaf::Fenced { info, lines, line, .. } => {
                self.emit(RawBody::Code { info, lines, muted: false, lang: None }, line);
            }
            OLeaf::Indented { lines, line, .. } => {
                self.emit(RawBody::Code { info: String::new(), lines, muted: false, lang: None }, line);
            }
            OLeaf::Html { lines, line, .. } => {
                self.emit(RawBody::Code { info: "html".into(), lines, muted: true, lang: Some(Lang::Plain) }, line);
            }
            OLeaf::Table { rows, aligns, line } => self.emit(RawBody::Table { aligns, rows }, line),
        }
    }

    fn close_to(&mut self, k: usize) {
        self.close_leaf();
        while self.stack.len() > k {
            let Some(top) = self.stack.last() else { break };
            if !matches!(top.kind, OK::List { .. }) && !top.has_content {
                let line = top.line;
                self.emit(RawBody::Empty, line);
            }
            let Some(o) = self.stack.pop() else { break };
            if let OK::List { loose, max, .. } = o.kind {
                if let Some(c) = self.out.ctrs.get_mut(o.ctr as usize) {
                    if let CtrKind::List { loose: l, max_num, .. } = &mut c.kind {
                        *l = loose;
                        *max_num = max;
                    }
                }
            }
        }
    }

    /// Strip leading reference definitions off paragraph text (registering them).
    fn strip_refs(&mut self, text: &str) -> String {
        let mut pos = 0;
        while pos < text.len() && text.as_bytes().get(indent_of(from(text, pos)) + pos) == Some(&b'[') {
            let Some((label, dest, title, n)) = ref_def(from(text, pos)) else { break };
            if self.out.refs.len() < 100_000 {
                self.out.refs.entry(label).or_insert((dest, title));
            }
            pos += n.max(1);
        }
        from(text, pos).to_owned()
    }

    fn para_push(&mut self, r: &str) {
        if let OLeaf::Para { text, last_start, nlines, .. } = &mut self.leaf {
            *nlines += 1;
            if text.len() < PARA_CAP {
                text.push('\n');
                *last_start = text.len();
                let room = PARA_CAP - text.len().min(PARA_CAP);
                text.push_str(cap(r.trim_start(), room));
            }
        }
    }

    /// Lazy-continuation test: does `rest` start a block (so it cannot continue a paragraph)?
    fn starts_block(&self, rest: &str) -> bool {
        let ind = indent_of(rest);
        if ind >= 4 {
            return false;
        }
        let t = from(rest, ind);
        t.starts_with('>') || list_marker(t).is_some() || self.starts_leaf(t, true) || foot_def(t).is_some()
    }

    fn starts_leaf(&self, t: &str, interrupt: bool) -> bool {
        atx(t).is_some() || fence_open(t).is_some() || is_hr(t) || html_start(t, interrupt).is_some()
    }

    /// Before opening the first new container of a line: close what did not match.
    fn prepare(&mut self, closed: &mut bool, matched: usize, list_cont: Option<usize>, item: Option<(bool, u8)>) {
        if *closed {
            return;
        }
        *closed = true;
        if let (Some(l), Some((ord, ch))) = (list_cont, item) {
            if matches!(self.stack.get(l), Some(Open { kind: OK::List { ordered, ch: c, .. }, .. }) if *ordered == ord && *c == ch) {
                self.close_to(l + 1);
                return;
            }
        }
        self.close_to(matched);
    }

    fn line(&mut self, s: &str) {
        let n = self.stack.len();
        let mut pos = 0usize;
        let mut matched = 0usize;
        while matched < n {
            let rest = from(s, pos);
            match self.stack[matched].kind {
                OK::Quote => {
                    let ind = indent_of(rest);
                    if ind <= 3 && rest.as_bytes().get(ind) == Some(&b'>') {
                        pos += ind + 1;
                        if s.as_bytes().get(pos) == Some(&b' ') {
                            pos += 1;
                        }
                        matched += 1;
                    } else {
                        break;
                    }
                }
                OK::List { .. } => matched += 1,
                OK::Item { indent } => {
                    if is_blank(rest) {
                        matched += 1;
                    } else if indent_of(rest) >= indent {
                        pos += indent;
                        matched += 1;
                    } else {
                        break;
                    }
                }
                OK::Foot => {
                    if is_blank(rest) {
                        matched += 1;
                    } else if indent_of(rest) >= 4 {
                        pos += 4;
                        matched += 1;
                    } else {
                        break;
                    }
                }
            }
        }
        let mut list_cont = None;
        if matched < n && matched > 0 && matches!(self.stack[matched - 1].kind, OK::List { .. }) {
            matched -= 1;
            list_cont = Some(matched);
        }
        let all = matched == n;
        let rest = from(s, pos);

        // Literal leaves continue.
        if all {
            match &mut self.leaf {
                OLeaf::Fenced { ch, len, indent, lines, .. } => {
                    let ind = indent_of(rest);
                    if ind <= 3 && closing_fence(from(rest, ind), *ch, *len) {
                        self.close_leaf();
                    } else {
                        lines.push(from(rest, ind.min(*indent)).to_owned());
                    }
                    return;
                }
                OLeaf::Html { end, lines, .. } => {
                    if *end == HtmlEnd::Blank && is_blank(rest) {
                        self.close_leaf();
                    } else {
                        lines.push(rest.to_owned());
                        if html_ends(*end, rest) {
                            self.close_leaf();
                        }
                        return;
                    }
                }
                OLeaf::Indented { lines, blanks, .. } => {
                    if is_blank(rest) {
                        *blanks += 1;
                        return;
                    }
                    if indent_of(rest) >= 4 {
                        for _ in 0..std::mem::take(blanks) {
                            lines.push(String::new());
                        }
                        lines.push(from(rest, 4).to_owned());
                        return;
                    }
                }
                _ => {}
            }
        }

        // Blank line.
        if is_blank(rest) {
            if !all {
                self.close_to(matched);
            } else if matches!(self.leaf, OLeaf::Para { .. } | OLeaf::Table { .. }) {
                self.close_leaf();
            }
            for o in &mut self.stack {
                if let OK::List { blank, .. } = &mut o.kind {
                    *blank = true;
                }
            }
            return;
        }

        // Lazy paragraph continuation.
        if !all && matches!(self.leaf, OLeaf::Para { .. }) && !self.starts_block(rest) {
            self.para_push(rest);
            self.after_nonblank();
            return;
        }

        // New containers.
        let mut cur = pos;
        let mut closed = false;
        let mut opened = false;
        loop {
            let r = from(s, cur);
            let ind = indent_of(r);
            if ind >= 4 {
                break;
            }
            let t = from(r, ind);
            let Some(&b0) = t.as_bytes().first() else { break };
            if is_hr(t) || self.depth() >= MAX_DEPTH || self.stop {
                break;
            }
            let para_tip = !opened && all && matches!(self.leaf, OLeaf::Para { .. });
            if b0 == b'>' {
                self.prepare(&mut closed, matched, list_cont, None);
                if !self.push_ctr(OK::Quote, CtrKind::Quote(None)) {
                    return;
                }
                cur += ind + 1;
                if s.as_bytes().get(cur) == Some(&b' ') {
                    cur += 1;
                }
                opened = true;
                continue;
            }
            if let Some(m) = list_marker(t) {
                let after = from(t, m.len);
                let blank = is_blank(after);
                if para_tip && (blank || (m.ordered && m.num != 1)) {
                    break;
                }
                self.prepare(&mut closed, matched, list_cont, Some((m.ordered, m.ch)));
                let sp = indent_of(after);
                let (cin, skip) = if blank {
                    (ind + m.len + 1, after.len())
                } else if sp >= 5 {
                    (ind + m.len + 1, 1)
                } else {
                    (ind + m.len + sp, sp)
                };
                let same = matches!(self.stack.last(), Some(Open { kind: OK::List { ch, ordered, .. }, .. }) if *ch == m.ch && *ordered == m.ordered);
                if !same {
                    let k = OK::List { ch: m.ch, ordered: m.ordered, blank: false, loose: false, opened: self.line_no, next: m.num, max: m.num };
                    if !self.push_ctr(k, CtrKind::List { ordered: m.ordered, start: m.num, loose: false, max_num: m.num }) {
                        return;
                    }
                }
                let mut num = 0;
                if let Some(Open { kind: OK::List { next, max, .. }, .. }) = self.stack.last_mut() {
                    num = *next;
                    *max = (*max).max(num);
                    *next = next.saturating_add(1);
                }
                cur += ind + m.len + skip;
                let task = task_marker(from(s, cur));
                if let Some((_, k)) = task {
                    cur += k;
                }
                let ck = CtrKind::Item { task: task.map(|t| t.0), num, line: self.line_no };
                if !self.push_ctr(OK::Item { indent: cin }, ck) {
                    return;
                }
                opened = true;
                continue;
            }
            if !para_tip && b0 == b'[' {
                if let Some((label, k)) = foot_def(t) {
                    self.prepare(&mut closed, matched, list_cont, None);
                    let next = self.out.feet.len() as u32 + 1;
                    let num = *self.out.feet.entry(label).or_insert(next);
                    if !self.push_ctr(OK::Foot, CtrKind::Foot { num }) {
                        return;
                    }
                    cur += ind + k;
                    opened = true;
                    continue;
                }
            }
            break;
        }
        if !closed && !all {
            self.close_to(matched);
        }

        // Leaf.
        let r = from(s, cur);
        if !is_blank(r) {
            let ind = indent_of(r);
            let t = from(r, ind);
            match self.leaf {
                OLeaf::Para { .. } => {
                    if ind < 4 {
                        if let Some(level) = setext(t) {
                            if self.setext(level) {
                                self.after_nonblank();
                                return;
                            }
                        }
                        if self.try_table(t) {
                            self.after_nonblank();
                            return;
                        }
                        if self.starts_leaf(t, true) {
                            self.close_leaf();
                            self.start_leaf(r);
                            self.after_nonblank();
                            return;
                        }
                    }
                    self.para_push(r);
                }
                OLeaf::Table { .. } => {
                    if ind < 4 && self.starts_leaf(t, false) {
                        self.close_leaf();
                        self.start_leaf(r);
                    } else {
                        self.table_row(r);
                    }
                }
                OLeaf::None => self.start_leaf(r),
                _ => {
                    self.close_leaf();
                    self.start_leaf(r);
                }
            }
        }
        self.after_nonblank();
    }

    /// Loose-list bookkeeping after a non-blank line.
    fn after_nonblank(&mut self) {
        let line = self.line_no;
        let mut target = true;
        for o in self.stack.iter_mut().rev() {
            if let OK::List { blank, loose, opened, .. } = &mut o.kind {
                if target && *opened != line {
                    if *blank {
                        *loose = true;
                    }
                    target = false;
                }
                *blank = false;
            }
        }
    }

    fn setext(&mut self, level: u8) -> bool {
        let OLeaf::Para { text, line, .. } = std::mem::take(&mut self.leaf) else { return false };
        let rest = self.strip_refs(&text);
        let rest = rest.trim();
        if rest.is_empty() {
            return false;
        }
        self.emit(RawBody::Heading(level, rest.to_owned()), line);
        true
    }

    fn try_table(&mut self, t: &str) -> bool {
        let OLeaf::Para { text, line, last_start, nlines } = &self.leaf else { return false };
        let Some(aligns) = delim_row(t) else { return false };
        let header = split_cells(from(text, *last_start));
        if header.len() != aligns.len() {
            return false;
        }
        let before = text.get(..last_start.saturating_sub(1)).unwrap_or("").to_owned();
        let (line0, hline) = (*line, line + nlines - 1);
        self.leaf = OLeaf::None;
        if !before.trim().is_empty() {
            let rest = self.strip_refs(&before);
            let rest = rest.trim_end();
            if !rest.is_empty() {
                self.emit(RawBody::Para(rest.to_owned()), line0);
            }
        }
        self.leaf = OLeaf::Table { rows: vec![header], aligns, line: hline };
        true
    }

    fn table_row(&mut self, r: &str) {
        if let OLeaf::Table { rows, aligns, .. } = &mut self.leaf {
            let mut cells = split_cells(r);
            cells.resize(aligns.len(), String::new());
            rows.push(cells);
        }
    }

    fn start_leaf(&mut self, r: &str) {
        let line = self.line_no;
        let ind = indent_of(r);
        if ind >= 4 {
            self.leaf = OLeaf::Indented { lines: vec![from(r, 4).to_owned()], line, blanks: 0 };
            return;
        }
        let t = from(r, ind);
        if let Some((level, text)) = atx(t) {
            self.emit(RawBody::Heading(level, text.to_owned()), line);
            return;
        }
        if let Some((ch, len, info)) = fence_open(t) {
            self.leaf = OLeaf::Fenced { ch, len, indent: ind, info: unescape(info), lines: Vec::new(), line };
            return;
        }
        if is_hr(t) {
            self.emit(RawBody::Rule, line);
            return;
        }
        if let Some(end) = html_start(t, false) {
            let lines = vec![r.to_owned()];
            if html_ends(end, from(t, 2)) {
                self.emit(RawBody::Code { info: "html".into(), lines, muted: true, lang: Some(Lang::Plain) }, line);
            } else {
                self.leaf = OLeaf::Html { end, lines, line };
            }
            return;
        }
        if let Some(top) = self.stack.last() {
            if matches!(top.kind, OK::Quote) && !top.has_content {
                if let Some(a) = Alert::from_tag(t) {
                    let id = top.ctr as usize;
                    if let Some(c) = self.out.ctrs.get_mut(id) {
                        c.kind = CtrKind::Quote(Some(a));
                    }
                    self.emit(RawBody::Title(a), line);
                    return;
                }
            }
        }
        self.leaf = OLeaf::Para { text: cap(t, PARA_CAP).to_owned(), line, last_start: 0, nlines: 1 };
    }
}
