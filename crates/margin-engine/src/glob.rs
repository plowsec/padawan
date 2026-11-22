//! gitignore-style globs. Used by `.gitignore` rules and search include/exclude lists.
//!
//! | Syntax | Meaning |
//! |---|---|
//! | `*` `?` `[a-z]` `[!x]` `[^x]` | within one path component (`?`/class = one char) |
//! | `**` (whole component) | zero or more components; `a/**` = everything inside `a` |
//! | leading `/` or a `/` inside | anchored to the base dir; else matches the base name at any depth |
//! | trailing `/` | directories only |
//! | `\x` | literal `x` |
//!
//! Cost: component matcher and path matcher are the two-pointer "last star" algorithm,
//! O(len × pattern) worst case, never exponential.

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ch(char),
    Any,
    Class { neg: bool, ranges: Box<[(char, char)]> },
    Star,
}

impl Tok {
    fn matches(&self, c: char) -> bool {
        match self {
            Tok::Ch(x) => *x == c,
            Tok::Any => true,
            Tok::Class { neg, ranges } => ranges.iter().any(|&(a, b)| a <= c && c <= b) != *neg,
            Tok::Star => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    /// `**`: any number of components.
    DStar,
    Lit(Box<str>),
    Glob(Box<[Tok]>),
}

impl Seg {
    fn matches(&self, comp: &str) -> bool {
        match self {
            Seg::DStar => true,
            Seg::Lit(l) => **l == *comp,
            Seg::Glob(t) => seg_match(t, comp),
        }
    }
}

/// Longest pattern accepted (bytes). Longer lines are dropped.
pub const MAX_PATTERN: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glob {
    segs: Box<[Seg]>,
    /// No `/` in the pattern: matches the last component, at any depth.
    basename: bool,
    pub dir_only: bool,
}

impl Glob {
    /// `None` for an empty or oversize pattern.
    pub fn parse(pat: &str) -> Option<Glob> {
        if pat.is_empty() || pat.len() > MAX_PATTERN {
            return None;
        }
        let mut p = pat;
        let mut dir_only = false;
        if p.len() > 1 && p.ends_with('/') && !p.ends_with("\\/") {
            dir_only = true;
            p = &p[..p.len() - 1];
        }
        let anchored_lead = p.starts_with('/');
        if anchored_lead {
            p = &p[1..];
        }
        if p.is_empty() {
            return None;
        }
        let basename = !anchored_lead && !p.contains('/');
        let mut segs: Vec<Seg> = Vec::new();
        for s in p.split('/') {
            if s.is_empty() {
                continue;
            }
            if s == "**" {
                if segs.last() != Some(&Seg::DStar) {
                    segs.push(Seg::DStar);
                }
                continue;
            }
            segs.push(parse_seg(s));
        }
        if segs.is_empty() {
            return None;
        }
        if basename && segs.len() == 1 && segs[0] == Seg::DStar {
            // `**` alone: everything.
            segs[0] = Seg::Glob(vec![Tok::Star].into());
        } else if !basename && segs.last() == Some(&Seg::DStar) {
            // `a/**`: inside `a`, not `a` itself.
            let n = segs.len();
            segs.insert(n - 1, Seg::Glob(vec![Tok::Star].into()));
        }
        Some(Glob { segs: segs.into(), basename, dir_only })
    }

    /// Matching work per character of input (number of tokens).
    pub fn cost(&self) -> usize {
        self.segs.iter().map(|s| if let Seg::Glob(t) = s { t.len() } else { 1 }).sum()
    }

    /// A literal base-name pattern (fast path for big ignore files).
    pub fn literal_name(&self) -> Option<&str> {
        match (&*self.segs, self.basename) {
            ([Seg::Lit(l)], true) => Some(l),
            _ => None,
        }
    }

    /// `*<literal>` base-name pattern: the literal suffix.
    pub fn star_suffix(&self) -> Option<String> {
        match (&*self.segs, self.basename) {
            ([Seg::Glob(t)], true) if t.len() >= 2 && t[0] == Tok::Star && t[1..].iter().all(|x| matches!(x, Tok::Ch(_))) => {
                Some(t[1..].iter().filter_map(|x| if let Tok::Ch(c) = x { Some(*c) } else { None }).collect())
            }
            _ => None,
        }
    }

    /// Path given as components relative to the pattern's base dir.
    pub fn matches(&self, comps: &[&str], is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        if self.basename {
            return match comps.last() {
                Some(c) => self.segs.first().is_some_and(|s| s.matches(c)),
                None => false,
            };
        }
        path_match(&self.segs, comps)
    }
}

fn parse_seg(s: &str) -> Seg {
    let chars: Vec<char> = s.chars().collect();
    let mut toks: Vec<Tok> = Vec::with_capacity(chars.len());
    let mut special = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() => {
                toks.push(Tok::Ch(chars[i + 1]));
                i += 2;
                continue;
            }
            '*' => {
                special = true;
                if toks.last() != Some(&Tok::Star) {
                    toks.push(Tok::Star);
                }
            }
            '?' => {
                special = true;
                toks.push(Tok::Any);
            }
            '[' => {
                if let Some((tok, next)) = parse_class(&chars, i) {
                    special = true;
                    toks.push(tok);
                    i = next;
                    continue;
                }
                toks.push(Tok::Ch('['));
            }
            _ => toks.push(Tok::Ch(c)),
        }
        i += 1;
    }
    if special {
        Seg::Glob(toks.into())
    } else {
        Seg::Lit(toks.iter().filter_map(|t| if let Tok::Ch(c) = t { Some(*c) } else { None }).collect::<String>().into())
    }
}

/// `[...]` starting at `chars[start] == '['` → (token, index after `]`). `None`: no closing bracket.
fn parse_class(chars: &[char], start: usize) -> Option<(Tok, usize)> {
    let mut i = start + 1;
    let mut neg = false;
    if matches!(chars.get(i), Some('!') | Some('^')) {
        neg = true;
        i += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    loop {
        let c = *chars.get(i)?;
        if c == ']' && !first {
            return Some((Tok::Class { neg, ranges: ranges.into() }, i + 1));
        }
        first = false;
        let lo = if c == '\\' {
            i += 1;
            *chars.get(i)?
        } else {
            c
        };
        i += 1;
        if chars.get(i) == Some(&'-') && chars.get(i + 1).is_some_and(|&x| x != ']') {
            let mut hi = chars[i + 1];
            i += 2;
            if hi == '\\' {
                hi = *chars.get(i)?;
                i += 1;
            }
            if lo <= hi {
                ranges.push((lo, hi));
            }
        } else {
            ranges.push((lo, lo));
        }
    }
}

fn char_at(s: &str, i: usize) -> Option<char> {
    s.get(i..).and_then(|r| r.chars().next())
}

/// One component vs a token list. Two-pointer with backtrack to the last `*` only.
fn seg_match(toks: &[Tok], s: &str) -> bool {
    let (mut ti, mut si) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    loop {
        if let Some(t) = toks.get(ti) {
            if *t == Tok::Star {
                star = Some((ti + 1, si));
                ti += 1;
                continue;
            }
            if let Some(c) = char_at(s, si) {
                if t.matches(c) {
                    ti += 1;
                    si += c.len_utf8();
                    continue;
                }
            }
        } else if si >= s.len() {
            return true;
        }
        match star {
            Some((sti, ssi)) => match char_at(s, ssi) {
                Some(c) => {
                    let n = ssi + c.len_utf8();
                    star = Some((sti, n));
                    ti = sti;
                    si = n;
                }
                None => return false,
            },
            None => return false,
        }
    }
}

/// Components vs segments; `**` is the star. Same algorithm one level up.
fn path_match(segs: &[Seg], comps: &[&str]) -> bool {
    let (mut ti, mut si) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    loop {
        if let Some(t) = segs.get(ti) {
            if *t == Seg::DStar {
                star = Some((ti + 1, si));
                ti += 1;
                continue;
            }
            if let Some(c) = comps.get(si) {
                if t.matches(c) {
                    ti += 1;
                    si += 1;
                    continue;
                }
            }
        } else if si >= comps.len() {
            return true;
        }
        match star {
            Some((sti, ssi)) if ssi < comps.len() => {
                star = Some((sti, ssi + 1));
                ti = sti;
                si = ssi + 1;
            }
            _ => return false,
        }
    }
}

/// Comma-separated globs for search include/exclude. A path matches when it, or one of its parent
/// directories, matches a glob.
#[derive(Clone, Debug, Default)]
pub struct GlobList {
    globs: Vec<Glob>,
}

/// Patterns kept from one include/exclude string.
const MAX_LIST: usize = 256;

impl GlobList {
    pub fn parse(list: &str) -> GlobList {
        let globs = list.split(',').map(str::trim).filter_map(|p| Glob::parse(p.strip_prefix("./").unwrap_or(p))).take(MAX_LIST).collect();
        GlobList { globs }
    }

    pub fn is_empty(&self) -> bool {
        self.globs.is_empty()
    }

    pub fn matches_path(&self, path: &str) -> bool {
        if self.globs.is_empty() {
            return false;
        }
        let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        for k in (1..=comps.len()).rev() {
            let is_dir = k < comps.len();
            if self.globs.iter().any(|g| g.matches(&comps[..k], is_dir)) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pat: &str, path: &str) -> bool {
        let g = Glob::parse(pat).unwrap();
        let c: Vec<&str> = path.split('/').collect();
        g.matches(&c, false)
    }

    #[test]
    fn table() {
        let cases: &[(&str, &str, bool)] = &[
            ("*.rs", "main.rs", true),
            ("*.rs", "src/a/main.rs", true),
            ("*.rs", "main.rsx", false),
            ("main.?s", "main.rs", true),
            ("main.?s", "main.s", false),
            ("[a-c]x", "bx", true),
            ("[a-c]x", "dx", false),
            ("[!a-c]x", "dx", true),
            ("[^a-c]x", "ax", false),
            ("[]]x", "]x", true),
            ("[x", "[x", true),
            ("/a.txt", "a.txt", true),
            ("/a.txt", "d/a.txt", false),
            ("d/a.txt", "d/a.txt", true),
            ("d/a.txt", "e/d/a.txt", false),
            ("**/a.txt", "a.txt", true),
            ("**/a.txt", "x/y/a.txt", true),
            ("a/**/b", "a/b", true),
            ("a/**/b", "a/x/y/b", true),
            ("a/**/b", "a/x/y/c", false),
            ("a/**", "a/x", true),
            ("a/**", "a/x/y", true),
            ("a/**", "a", false),
            ("**", "anything/here", true),
            ("src/*.rs", "src/a.rs", true),
            ("src/*.rs", "src/x/a.rs", false),
            ("\\*x", "*x", true),
            ("\\*x", "ax", false),
            ("é?", "éß", true),
        ];
        for &(p, s, want) in cases {
            assert_eq!(m(p, s), want, "{p:?} vs {s:?}");
        }
    }

    #[test]
    fn dir_only() {
        let g = Glob::parse("build/").unwrap();
        assert!(g.matches(&["x", "build"], true));
        assert!(!g.matches(&["x", "build"], false));
    }

    #[test]
    fn pathological_is_fast() {
        let pat = format!("{}b", "a*".repeat(200));
        let g = Glob::parse(&pat).unwrap();
        let s = "a".repeat(4000);
        let t = std::time::Instant::now();
        assert!(!g.matches(&[&s], false));
        let deep = format!("{}x", "**/a/".repeat(100));
        let g2 = Glob::parse(&deep).unwrap();
        let comps: Vec<&str> = std::iter::repeat_n("a", 1000).collect();
        assert!(!g2.matches(&comps, false));
        assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
    }

    #[test]
    fn lists() {
        let l = GlobList::parse("*.rs, docs/**, node_modules");
        assert!(l.matches_path("src/a.rs"));
        assert!(l.matches_path("docs/x/y.md"));
        assert!(l.matches_path("a/node_modules/x.js"));
        assert!(!l.matches_path("src/a.md"));
        assert!(GlobList::parse(" , ").is_empty());
    }
}
