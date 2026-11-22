//! Inline parser: one scan into nodes, CommonMark delimiter algorithm for emphasis (with the
//! `openers_bottom` bound, so linear), bracket stack for links/images, then a flat span list.
//!
//! Style ranges are applied with difference arrays (emphasis) and a skip list (images), so nested
//! constructs never cost more than linear time. Every forward scan (link tail, title, tag) is bounded.

use std::collections::HashMap;

use crate::block::{link_dest, link_title, unescape};
use crate::doc::{f, Link, Span, NONE};
use crate::text::{clean, clean_into, entity, from, is_punct, norm_label};

/// Spans per block.
pub const SPAN_CAP: usize = 4096;
/// Text bytes per block (after that: `…`).
pub const TEXT_CAP: usize = 64 * 1024;

pub struct Cx<'a> {
    pub refs: &'a HashMap<String, (String, String)>,
    pub feet: &'a HashMap<String, u32>,
    pub links: &'a mut Vec<Link>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Text,
    Delim,
    Open,
    Code,
    Break,
    Html,
    Foot,
}

#[derive(Debug)]
struct Node {
    k: K,
    text: String,
    /// Delimiter: chars left.
    n: u32,
    ch: u8,
    link: u32,
}

struct Delim {
    node: usize,
    ch: u8,
    count: u32,
    orig: u32,
    open: bool,
    close: bool,
}

struct Br {
    node: usize,
    delims: usize,
    image: bool,
    start: usize,
}

#[derive(Default)]
struct St {
    nodes: Vec<Node>,
    delims: Vec<Delim>,
    brs: Vec<Br>,
    /// Brackets below this index (non-image) are inactive (a link closed after them).
    inactive_below: usize,
    em: Vec<(usize, usize, u8)>,
    linkr: Vec<(usize, usize, u32)>,
    imgr: Vec<(usize, usize, u32)>,
    /// Backtick runs: length → sorted start positions. Built on first use.
    runs: Option<HashMap<usize, Vec<usize>>>,
    no_comment_end: bool,
}

const NIL: usize = usize::MAX;

impl St {
    fn text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if let Some(n) = self.nodes.last_mut() {
            if n.k == K::Text && n.link == NONE {
                n.text.push_str(s);
                return;
            }
        }
        self.push(K::Text, s.to_owned(), NONE);
    }

    fn push(&mut self, k: K, text: String, link: u32) {
        self.nodes.push(Node { k, text, n: 0, ch: 0, link });
    }

    /// Strip trailing spaces of the last text node; returns how many.
    fn trim_trailing(&mut self) -> usize {
        match self.nodes.last_mut() {
            Some(n) if n.k == K::Text && n.link == NONE => {
                let t = n.text.trim_end_matches(' ').len();
                let k = n.text.len() - t;
                n.text.truncate(t);
                k
            }
            _ => 0,
        }
    }

    fn code_closer(&mut self, src: &str, from_pos: usize, len: usize) -> Option<usize> {
        let runs = self.runs.get_or_insert_with(|| {
            let mut m: HashMap<usize, Vec<usize>> = HashMap::new();
            let b = src.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'`' {
                    let s = i;
                    while i < b.len() && b[i] == b'`' {
                        i += 1;
                    }
                    m.entry(i - s).or_default().push(s);
                } else {
                    i += 1;
                }
            }
            m
        });
        let v = runs.get(&len)?;
        let k = v.partition_point(|&p| p < from_pos);
        v.get(k).copied()
    }

    fn delim(&mut self, src: &str, i: usize, k: usize, c: u8) {
        let prev = src.get(..i).and_then(|s| s.chars().next_back()).unwrap_or(' ');
        let next = from(src, i + k).chars().next().unwrap_or(' ');
        let (pw, nw) = (prev.is_whitespace(), next.is_whitespace());
        let (pp, np) = (is_punct(prev), is_punct(next));
        let left = !nw && (!np || pw || pp);
        let right = !pw && (!pp || nw || np);
        let (open, close) = match c {
            b'_' => (left && (!right || pp), right && (!left || np)),
            _ => (left, right),
        };
        if c == b'~' && k > 2 {
            self.text(from(src, i).get(..k).unwrap_or(""));
            return;
        }
        let node = self.nodes.len();
        self.nodes.push(Node { k: K::Delim, text: String::new(), n: k as u32, ch: c, link: NONE });
        if open || close {
            self.delims.push(Delim { node, ch: c, count: k as u32, orig: k as u32, open, close });
        }
    }

    fn process_emphasis(&mut self, bottom: usize) {
        let len = self.delims.len();
        if len <= bottom {
            return;
        }
        let m = len - bottom;
        let mut prev: Vec<usize> = (0..m).map(|i| if i == 0 { NIL } else { i - 1 }).collect();
        let mut next: Vec<usize> = (0..m).map(|i| if i + 1 == m { NIL } else { i + 1 }).collect();
        let mut ob = [[[-1isize; 3]; 2]; 3];
        let d = &mut self.delims[bottom..];
        let unlink = |prev: &mut Vec<usize>, next: &mut Vec<usize>, x: usize| {
            let (p, n) = (prev[x], next[x]);
            if p != NIL {
                next[p] = n;
            }
            if n != NIL {
                prev[n] = p;
            }
        };
        let mut cur = 0usize;
        while cur != NIL {
            if !d[cur].close {
                cur = next[cur];
                continue;
            }
            let ch = d[cur].ch;
            let ci = match ch {
                b'*' => 0,
                b'_' => 1,
                _ => 2,
            };
            let key = (ci, usize::from(d[cur].open), (d[cur].orig % 3) as usize);
            let lim = ob[key.0][key.1][key.2];
            let mut o = prev[cur];
            let mut found = NIL;
            while o != NIL && (o as isize) > lim {
                let od = &d[o];
                if od.ch == ch && od.open && od.count > 0 {
                    let ok = if ch == b'~' {
                        od.count == d[cur].count
                    } else {
                        !((od.close || d[cur].open) && (od.orig + d[cur].orig).is_multiple_of(3) && !(od.orig.is_multiple_of(3) && d[cur].orig.is_multiple_of(3)))
                    };
                    if ok {
                        found = o;
                        break;
                    }
                }
                o = prev[o];
            }
            if found != NIL {
                let o = found;
                let (oc, cc) = (d[o].count, d[cur].count);
                let used = if ch == b'~' {
                    cc
                } else if oc >= 2 && cc >= 2 {
                    2
                } else {
                    1
                };
                let flag = if ch == b'~' {
                    f::STRIKE
                } else if used == 2 {
                    f::STRONG
                } else {
                    f::EM
                };
                self.em.push((d[o].node + 1, d[cur].node, flag));
                d[o].count -= used;
                d[cur].count -= used;
                if let Some(n) = self.nodes.get_mut(d[o].node) {
                    n.n = d[o].count;
                }
                if let Some(n) = self.nodes.get_mut(d[cur].node) {
                    n.n = d[cur].count;
                }
                next[o] = cur;
                prev[cur] = o;
                if d[o].count == 0 {
                    unlink(&mut prev, &mut next, o);
                }
                if d[cur].count == 0 {
                    let nx = next[cur];
                    unlink(&mut prev, &mut next, cur);
                    cur = nx;
                }
            } else {
                ob[key.0][key.1][key.2] = if prev[cur] == NIL { -1 } else { prev[cur] as isize };
                let nx = next[cur];
                if !d[cur].open {
                    unlink(&mut prev, &mut next, cur);
                }
                cur = nx;
            }
        }
        self.delims.truncate(bottom);
    }

    fn pop_br(&mut self) {
        self.brs.pop();
        self.inactive_below = self.inactive_below.min(self.brs.len());
    }

    fn open(&mut self, start: usize, image: bool) {
        let node = self.nodes.len();
        self.push(K::Open, if image { "![".into() } else { "[".into() }, NONE);
        self.brs.push(Br { node, delims: self.delims.len(), image, start });
    }

    /// `]` at `i`. Returns the next position.
    fn close(&mut self, src: &str, i: usize, cx: &mut Cx<'_>) -> usize {
        let b = src.as_bytes();
        let Some(br) = self.brs.last() else {
            self.text("]");
            return i + 1;
        };
        let (bnode, bdel, image, start) = (br.node, br.delims, br.image, br.start);
        let active = image || self.brs.len() > self.inactive_below;
        if !active {
            self.pop_br();
            self.text("]");
            return i + 1;
        }
        let label_raw = src.get(start..i).unwrap_or("");
        // Footnote reference.
        if !image {
            if let Some(l) = label_raw.strip_prefix('^') {
                if let Some(&num) = cx.feet.get(&norm_label(l)) {
                    self.nodes.truncate(bnode);
                    self.delims.truncate(bdel);
                    while self.em.last().is_some_and(|r| r.0 > bnode) {
                        self.em.pop();
                    }
                    while self.linkr.last().is_some_and(|r| r.0 > bnode) {
                        self.linkr.pop();
                    }
                    while self.imgr.last().is_some_and(|r| r.0 > bnode) {
                        self.imgr.pop();
                    }
                    self.pop_br();
                    let idx = cx.links.len() as u32;
                    cx.links.push(Link { dest: format!("#fn-{num}"), shown: format!("footnote {num}"), title: String::new(), foot: num });
                    self.push(K::Foot, num.to_string(), idx);
                    return i + 1;
                }
            }
        }
        let mut found: Option<(String, String, usize)> = None;
        if b.get(i + 1) == Some(&b'(') {
            found = inline_tail(src, i + 2);
        }
        if found.is_none() {
            let mut shortcut = true;
            if b.get(i + 1) == Some(&b'[') {
                if let Some(e) = ref_label_end(src, i + 2) {
                    let lab = src.get(i + 2..e).unwrap_or("");
                    let lab = if lab.is_empty() { label_raw } else { lab };
                    if let Some((d, t)) = lookup(cx, lab) {
                        found = Some((d, t, e + 1));
                    }
                    shortcut = src.get(i + 2..e).is_some_and(|l| !l.is_empty());
                }
            }
            if found.is_none() && shortcut {
                if let Some((d, t)) = lookup(cx, label_raw) {
                    found = Some((d, t, i + 1));
                }
            }
        }
        let Some((dest, title, end)) = found else {
            self.pop_br();
            self.text("]");
            return i + 1;
        };
        let idx = cx.links.len() as u32;
        cx.links.push(Link { shown: clean(&dest), title: clean(&title), dest, foot: NONE });
        self.process_emphasis(bdel);
        let s = bnode + 1;
        let e = self.nodes.len();
        if let Some(n) = self.nodes.get_mut(bnode) {
            n.text.clear();
            n.k = K::Text;
        }
        if image {
            self.imgr.push((s, e, idx));
        } else {
            self.linkr.push((s, e, idx));
        }
        self.pop_br();
        if !image {
            self.inactive_below = self.brs.len();
        }
        // Keep the next text from merging into a node inside the link.
        self.push(K::Text, String::new(), NONE);
        end
    }

    /// `<` at `i`: comment, `<br>`, autolink, raw tag, or a plain `<`.
    fn angle(&mut self, src: &str, i: usize, cx: &mut Cx<'_>) -> usize {
        let r = from(src, i);
        let rb = r.as_bytes();
        if r.starts_with("<!--") {
            if !self.no_comment_end {
                if let Some(e) = from(r, 4).find("-->") {
                    let end = i + 4 + e + 3;
                    // Hidden: do not leave a double space behind.
                    if self.nodes.last().is_some_and(|n| n.k == K::Text && n.text.ends_with(' ')) {
                        return skip_spaces(src.as_bytes(), end);
                    }
                    return end;
                }
                self.no_comment_end = true;
            }
            self.text("<");
            return i + 1;
        }
        for br in ["<br>", "<br/>", "<br />"] {
            if rb.len() >= br.len() && rb[..br.len()].eq_ignore_ascii_case(br.as_bytes()) {
                self.trim_trailing();
                self.push(K::Break, String::new(), NONE);
                return skip_spaces(rb, br.len()) + i;
            }
        }
        if let Some(n) = autolink(r) {
            let url = r.get(1..n - 1).unwrap_or("").to_owned();
            let idx = cx.links.len() as u32;
            cx.links.push(Link { shown: clean(&url), title: String::new(), dest: url.clone(), foot: NONE });
            self.push(K::Text, url, idx);
            return i + n;
        }
        let close = rb.get(1) == Some(&b'/');
        let ns = if close { 2 } else { 1 };
        if rb.get(ns).is_some_and(u8::is_ascii_alphabetic) {
            let mut j = ns;
            while j < rb.len() && j < 2048 {
                match rb[j] {
                    b'>' => {
                        let t = r.get(..=j).unwrap_or("").replace('\n', " ");
                        self.push(K::Html, t, NONE);
                        return i + j + 1;
                    }
                    b'<' => break,
                    _ => j += 1,
                }
            }
        }
        self.text("<");
        i + 1
    }
}

fn lookup(cx: &Cx<'_>, label: &str) -> Option<(String, String)> {
    if label.len() > 999 || label.trim().is_empty() {
        return None;
    }
    cx.refs.get(&norm_label(label)).cloned()
}

/// End (index of `]`) of a link label starting at `s` (just after `[`).
fn ref_label_end(src: &str, s: usize) -> Option<usize> {
    let b = src.as_bytes();
    let mut i = s;
    while i < b.len() && i - s <= 999 {
        match b[i] {
            b'\\' => i += 2,
            b'[' => return None,
            b']' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n') {
        i += 1;
    }
    i
}

/// `dest "title")` after `](`. Returns (dest, title, end).
fn inline_tail(src: &str, p: usize) -> Option<(String, String, usize)> {
    let b = src.as_bytes();
    let j = skip_ws(b, p);
    if b.get(j) == Some(&b')') {
        return Some((String::new(), String::new(), j + 1));
    }
    let (d, n) = link_dest(from(src, j))?;
    let k = j + n;
    let k2 = skip_ws(b, k);
    let mut title = String::new();
    let mut e = k2;
    if k2 > k {
        if let Some((t, tn)) = link_title(from(src, k2)) {
            title = unescape(t);
            e = skip_ws(b, k2 + tn);
        }
    }
    if b.get(e) != Some(&b')') {
        return None;
    }
    Some((unescape(d), title, e + 1))
}

/// `<scheme:…>` or `<a@b.c>` at the start of `r`: total length.
fn autolink(r: &str) -> Option<usize> {
    let b = r.as_bytes();
    let mut i = 1;
    // URI
    if b.get(1).is_some_and(u8::is_ascii_alphabetic) {
        while i < b.len() && i < 34 && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'+' | b'.' | b'-')) {
            i += 1;
        }
        if i >= 3 && b.get(i) == Some(&b':') {
            let mut j = i + 1;
            while j < b.len() && j < 4096 {
                match b[j] {
                    b'>' => return Some(j + 1),
                    b'<' | b' ' | b'\n' | 0..=0x1f => return None,
                    _ => j += 1,
                }
            }
            return None;
        }
    }
    // E-mail
    let mut j = 1;
    let mut at = false;
    while j < b.len() && j < 512 {
        let c = b[j];
        if c == b'>' {
            return (at && j > 3 && b[j - 1] != b'@').then_some(j + 1);
        }
        if c == b'@' {
            if at || j == 1 {
                return None;
            }
            at = true;
        } else if !(c.is_ascii_alphanumeric() || (!at && b".!#$%&'*+/=?^_`{|}~-".contains(&c)) || (at && matches!(c, b'.' | b'-'))) {
            return None;
        }
        j += 1;
    }
    None
}

/// GFM bare URL at the start of `r`: byte length.
fn bare_url(r: &str) -> Option<usize> {
    let pre = ["https://", "http://", "www."].into_iter().find(|p| r.len() > p.len() && r.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes()))?;
    let b = r.as_bytes();
    let mut n = 0;
    while n < b.len() && n < 4096 && !b[n].is_ascii_whitespace() && b[n] != b'<' {
        n += 1;
    }
    let mut s = crate::text::cap(r, n);
    loop {
        let t = s.trim_end_matches(['?', '!', '.', ',', ':', '*', '_', '~', '\'', '"', ';']);
        let t = if t.ends_with(')') && t.matches(')').count() > t.matches('(').count() { t.get(..t.len() - 1).unwrap_or("") } else { t };
        if t.len() == s.len() {
            break;
        }
        s = t;
    }
    let host = from(s, pre.len());
    if host.is_empty() || (pre == "www." && !host.contains('.')) || !host.as_bytes()[0].is_ascii_alphanumeric() {
        return None;
    }
    Some(s.len())
}

fn is_special(c: u8) -> bool {
    matches!(c, b'\\' | b'`' | b'*' | b'_' | b'~' | b'!' | b'[' | b']' | b'<' | b'&' | b'\n' | b'h' | b'w' | b'H' | b'W')
}

/// Inline content of one block → spans.
pub fn parse_inline(src: &str, cx: &mut Cx<'_>) -> Vec<Span> {
    let mut st = St::default();
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'\\' => match b.get(i + 1) {
                Some(b'\n') => {
                    st.trim_trailing();
                    st.push(K::Break, String::new(), NONE);
                    i = skip_spaces(b, i + 2);
                }
                Some(&n) if n.is_ascii_punctuation() => {
                    st.text(src.get(i + 1..i + 2).unwrap_or(""));
                    i += 2;
                }
                _ => {
                    st.text("\\");
                    i += 1;
                }
            },
            b'`' => {
                let k = b[i..].iter().take_while(|&&x| x == b'`').count();
                if let Some(e) = st.code_closer(src, i + k, k) {
                    let raw = src.get(i + k..e).unwrap_or("").replace('\n', " ");
                    let t = if raw.len() >= 2 && raw.starts_with(' ') && raw.ends_with(' ') && !raw.bytes().all(|x| x == b' ') {
                        raw.get(1..raw.len() - 1).unwrap_or("").to_owned()
                    } else {
                        raw
                    };
                    st.push(K::Code, t, NONE);
                    i = e + k;
                } else {
                    st.text(src.get(i..i + k).unwrap_or(""));
                    i += k;
                }
            }
            b'*' | b'_' | b'~' => {
                let k = b[i..].iter().take_while(|&&x| x == c).count();
                st.delim(src, i, k, c);
                i += k;
            }
            b'!' if b.get(i + 1) == Some(&b'[') => {
                st.open(i + 2, true);
                i += 2;
            }
            b'[' => {
                st.open(i + 1, false);
                i += 1;
            }
            b']' => i = st.close(src, i, cx),
            b'<' => i = st.angle(src, i, cx),
            b'&' => {
                if let Some((ch, n)) = entity(from(src, i)) {
                    let mut tmp = [0u8; 4];
                    st.text(ch.encode_utf8(&mut tmp));
                    i += n;
                } else {
                    st.text("&");
                    i += 1;
                }
            }
            b'\n' => {
                if st.trim_trailing() >= 2 {
                    st.push(K::Break, String::new(), NONE);
                } else {
                    st.text(" ");
                }
                i = skip_spaces(b, i + 1);
            }
            b'h' | b'w' | b'H' | b'W' => {
                let prev = src.get(..i).and_then(|s| s.chars().next_back());
                let boundary = prev.is_none_or(|p| p.is_whitespace() || matches!(p, '(' | '*' | '_' | '~'));
                match boundary.then(|| bare_url(from(src, i))).flatten() {
                    Some(n) => {
                        let url = from(src, i).get(..n).unwrap_or("").to_owned();
                        let idx = cx.links.len() as u32;
                        cx.links.push(Link { shown: clean(&url), title: String::new(), dest: url.clone(), foot: NONE });
                        st.push(K::Text, url, idx);
                        i += n;
                    }
                    None => {
                        st.text(src.get(i..i + 1).unwrap_or(""));
                        i += 1;
                    }
                }
            }
            _ => {
                let mut j = i + 1;
                while j < b.len() && !is_special(b[j]) {
                    j += 1;
                }
                st.text(src.get(i..j).unwrap_or(""));
                i = j;
            }
        }
    }
    st.trim_trailing();
    st.process_emphasis(0);
    build(st)
}

fn skip_spaces(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    i
}

fn find(nxt: &mut [usize], mut i: usize) -> usize {
    while nxt[i] != i {
        nxt[i] = nxt[nxt[i]];
        i = nxt[i];
    }
    i
}

fn build(st: St) -> Vec<Span> {
    let nodes = st.nodes;
    let nn = nodes.len();
    let mut d = vec![[0i32; 3]; nn + 1];
    for &(s, e, fl) in &st.em {
        if s < e && e <= nn {
            let k = match fl {
                f::EM => 0,
                f::STRONG => 1,
                _ => 2,
            };
            d[s][k] += 1;
            d[e][k] -= 1;
        }
    }
    let mut link_of: Vec<u32> = nodes.iter().map(|n| n.link).collect();
    for &(s, e, idx) in &st.linkr {
        for l in link_of.iter_mut().take(e.min(nn)).skip(s) {
            *l = idx;
        }
    }
    let mut img_of = vec![NONE; nn];
    let mut img_start = vec![NIL; nn];
    let mut nxt: Vec<usize> = (0..=nn).collect();
    for &(s, e, idx) in st.imgr.iter().rev() {
        let e = e.min(nn);
        if s >= e {
            continue;
        }
        let mut k = find(&mut nxt, s);
        while k < e {
            img_of[k] = idx;
            img_start[k] = s;
            nxt[k] = k + 1;
            k = find(&mut nxt, k + 1);
        }
    }
    let mut spans: Vec<Span> = Vec::new();
    let mut total = 0usize;
    let mut run = [0i32; 3];
    let mut flags = Vec::with_capacity(nn);
    for row in d.iter().take(nn) {
        for k in 0..3 {
            run[k] += row[k];
        }
        let mut fl = 0;
        if run[0] > 0 {
            fl |= f::EM;
        }
        if run[1] > 0 {
            fl |= f::STRONG;
        }
        if run[2] > 0 {
            fl |= f::STRIKE;
        }
        flags.push(fl);
    }
    let node_text = |n: &Node| -> String {
        match n.k {
            K::Delim => (n.ch as char).to_string().repeat(n.n as usize),
            K::Break => String::new(),
            _ => n.text.clone(),
        }
    };
    let mut i = 0;
    while i < nn {
        if total > TEXT_CAP || spans.len() >= SPAN_CAP {
            spans.push(Span { text: "\u{2026}".into(), f: 0, link: NONE, img: NONE });
            break;
        }
        if img_of[i] != NONE {
            let idx = img_of[i];
            let s = img_start[i];
            let mut alt = String::new();
            while i < nn && img_of[i] == idx && img_start[i] == s {
                if alt.len() < 1024 {
                    alt.push_str(&node_text(&nodes[i]));
                }
                i += 1;
            }
            let link = s.checked_sub(1).and_then(|o| link_of.get(o).copied()).unwrap_or(NONE);
            let text = clean(crate::text::cap(&alt, 1024));
            total += text.len();
            spans.push(Span { text, f: f::IMAGE, link, img: idx });
            continue;
        }
        let n = &nodes[i];
        let extra = match n.k {
            K::Code => f::CODE,
            K::Html => f::HTML,
            K::Foot => f::FOOT,
            _ => 0,
        };
        let fl = flags[i] | extra;
        let link = link_of[i];
        let t = node_text(n);
        if t.is_empty() && n.k != K::Break {
            i += 1;
            continue;
        }
        let merge = matches!(spans.last(), Some(l) if l.f == fl && l.link == link && fl & (f::IMAGE | f::FOOT) == 0);
        if !merge {
            spans.push(Span { text: String::new(), f: fl, link, img: NONE });
        }
        if let Some(last) = spans.last_mut() {
            let before = last.text.len();
            if n.k == K::Break {
                last.text.push('\n');
            } else {
                clean_into(&mut last.text, crate::text::cap(&t, TEXT_CAP), false);
            }
            total += last.text.len() - before;
        }
        i += 1;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(s: &str) -> String {
        let refs = HashMap::from([("ref".to_owned(), ("/r".to_owned(), "T".to_owned()))]);
        let feet = HashMap::from([("1".to_owned(), 1u32)]);
        let mut links = Vec::new();
        let spans = parse_inline(s, &mut Cx { refs: &refs, feet: &feet, links: &mut links });
        let inner = crate::doc::Inner { links, ..Default::default() };
        let mut o = String::new();
        inner_dump(&inner, &spans, &mut o);
        o
    }

    fn inner_dump(inner: &crate::doc::Inner, spans: &[Span], o: &mut String) {
        let leaf = crate::doc::Leaf {
            line: 0,
            ctr: NONE,
            gap: crate::doc::Gap::Start,
            hash: 0,
            weight: 0,
            body: crate::doc::Body::Text { kind: crate::doc::TextKind::Para, spans: spans.to_vec() },
        };
        let d = crate::doc::Inner { leaves: vec![leaf], links: inner.links.clone(), ..Default::default() };
        o.push_str(d.dump().trim_start_matches("0 p ").trim_end());
    }

    #[test]
    fn emphasis() {
        assert_eq!(dump("*a* _b_ **c** __d__"), "*a* *b* **c** **d**");
        assert_eq!(dump("***x***"), "***x***");
        assert_eq!(dump("*a **b** c*"), "*a ****b**** c*");
        assert_eq!(dump("**a *b* c**"), "**a *****b***** c**");
        assert_eq!(dump("snake_case_name"), "snake_case_name");
        assert_eq!(dump("* not em *"), "* not em *");
        assert_eq!(dump("~~gone~~ ~one~ ~~~x~~~"), "~~gone~~ ~~one~~ ~~~x~~~");
        assert_eq!(dump("*a"), "*a");
        assert_eq!(dump("a**b**c"), "a**b**c");
    }

    #[test]
    fn code_spans_escapes_entities() {
        assert_eq!(dump("`a` ``b ` c`` ` d `"), "`a` `b ` c` `d`");
        assert_eq!(dump("```x``"), "```x``");
        assert_eq!(dump("\\*no\\* \\a"), "*no* \\a");
        assert_eq!(dump("&amp; &lt; &#65; &#x42; &nope; &copy;"), "& < A B &nope; ©");
        assert_eq!(dump("`*no*`"), "`*no*`");
    }

    #[test]
    fn links_and_images() {
        assert_eq!(dump("[a](/u \"t\") [b][ref] [ref] [c][] [none]"), "[a](/u) [b](/r) [ref](/r) [c][] [none]");
        assert_eq!(dump("[*x*](y)"), "[*x*](y)");
        assert_eq!(dump("![alt *t*](i.png)"), "{img:alt t|i.png}");
        assert_eq!(dump("[![b](i)](l)"), "[{img:b|i}](l)");
        assert_eq!(dump("<https://x.org/a> <a@b.co>"), "[https://x.org/a](https://x.org/a) [a@b.co](a@b.co)");
        assert_eq!(dump("see https://x.org/p?q=1. ok"), "see [https://x.org/p?q=1](https://x.org/p?q=1). ok");
        assert_eq!(dump("(www.x.org)"), "([www.x.org](www.x.org))");
        assert_eq!(dump("[a [b](c) d](e)"), "[a [b](c) d](e)");
        assert_eq!(dump("[x](<a b> 'q')"), "[x](a b)");
        assert_eq!(dump("[x]( /p )"), "[x](/p)");
    }

    #[test]
    fn breaks_html_footnotes() {
        assert_eq!(dump("a  \nb\\\nc\nd"), "a\\nb\\nc d");
        assert_eq!(dump("x <br> y <!-- hid --> <span class=\"k\">z</span>"), "x\\ny {html:<span class=\"k\">}z{html:</span>}");
        assert_eq!(dump("note[^1] and [^9]"), "note^1 and [^9]");
        assert_eq!(dump("a < b"), "a < b");
    }
}
