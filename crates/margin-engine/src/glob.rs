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

/// Alternatives produced by one `{a,b}` pattern.
pub const MAX_ALTS: usize = 64;

/// `{a,b}` alternatives → plain patterns (nested groups too). Unmatched or escaped braces stay literal.
/// At most `MAX_ALTS` results; past that, the rest is kept unexpanded.
pub fn expand_braces(pat: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut work = vec![pat.to_owned()];
    let mut steps = 0usize;
    while let Some(p) = work.pop() {
        steps += 1;
        let group = if steps > 4096 || out.len() + work.len() >= MAX_ALTS { None } else { innermost_group(&p) };
        match group {
            Some((open, close)) => {
                let (head, inner, tail) = (p.get(..open).unwrap_or(""), p.get(open + 1..close).unwrap_or(""), p.get(close + 1..).unwrap_or(""));
                let alts: Vec<&str> = inner.split(',').collect();
                let room = MAX_ALTS.saturating_sub(out.len() + work.len()).max(1);
                for a in alts.into_iter().take(room).rev() {
                    work.push(format!("{head}{a}{tail}"));
                }
            }
            None => {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Byte positions of the innermost `{ … }` (the first `}` and the last `{` before it), unescaped.
fn innermost_group(p: &str) -> Option<(usize, usize)> {
    let b = p.as_bytes();
    let esc = |i: usize| i > 0 && b.get(i - 1) == Some(&b'\\');
    let close = (0..b.len()).find(|&i| b.get(i) == Some(&b'}') && !esc(i))?;
    let open = (0..close).rev().find(|&i| b.get(i) == Some(&b'{') && !esc(i))?;
    Some((open, close))
}

/// Comma-separated globs for search include/exclude. A path matches when it, or one of its parent
/// directories, matches a glob. `{a,b}` alternatives expand.
#[derive(Clone, Debug, Default)]
pub struct GlobList {
    globs: Vec<Glob>,
}

/// Patterns kept from one include/exclude string.
const MAX_LIST: usize = 256;

impl GlobList {
    pub fn parse(list: &str) -> GlobList {
        Self::from_patterns(split_list(list).into_iter())
    }

    pub fn from_patterns<'p>(pats: impl Iterator<Item = &'p str>) -> GlobList {
        let mut globs = Vec::new();
        for p in pats {
            for alt in expand_braces(p.strip_prefix("./").unwrap_or(p)) {
                if globs.len() >= MAX_LIST {
                    return GlobList { globs };
                }
                if let Some(g) = Glob::parse(alt.strip_prefix("./").unwrap_or(&alt)) {
                    globs.push(g);
                }
            }
        }
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

/// Split a comma list at commas outside `{ }`, trimmed, empties dropped.
pub fn split_list(list: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0u32;
    let mut start = 0usize;
    for (i, c) in list.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(list.get(start..i).unwrap_or("").trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(list.get(start..).unwrap_or("").trim());
    out.retain(|s| !s.is_empty());
    out
}

/// Include + exclude lists of a query (`Search`, `Replace`, `FindFiles`).
///
/// | Rule | Meaning |
/// |---|---|
/// | include non-empty | the path (or a parent dir) matches one include |
/// | exclude | the path (or a parent dir) matches no exclude |
/// | `!glob` in include | an exclude |
/// | `name` / `*.rs` (no `/`) | the name at any depth; `dir/`, `dir/**` = everything below |
#[derive(Clone, Debug, Default)]
pub struct FileFilter {
    include: GlobList,
    exclude: GlobList,
}

impl FileFilter {
    pub fn parse(include: &str, exclude: &str) -> FileFilter {
        let inc = split_list(include);
        let (neg, pos): (Vec<&str>, Vec<&str>) = inc.into_iter().partition(|p| p.starts_with('!'));
        let negs = neg.into_iter().map(|p| p.get(1..).unwrap_or("").trim());
        FileFilter { include: GlobList::from_patterns(pos.into_iter()), exclude: GlobList::from_patterns(split_list(exclude).into_iter().chain(negs)) }
    }

    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    pub fn pass(&self, path: &str) -> bool {
        (self.include.is_empty() || self.include.matches_path(path)) && (self.exclude.is_empty() || !self.exclude.matches_path(path))
    }
}

/// `FindFiles` glob query: matches the relative path, or the name alone when the pattern has no `/`.
#[derive(Clone, Debug, Default)]
pub struct PathGlob {
    globs: Vec<Glob>,
}

impl PathGlob {
    /// Query has `*`, `?`, `[` or `{`.
    pub fn is_glob(q: &str) -> bool {
        q.contains(['*', '?', '[', '{'])
    }

    pub fn parse(q: &str) -> PathGlob {
        let q = q.trim();
        let globs = expand_braces(q.strip_prefix("./").unwrap_or(q)).iter().filter_map(|p| Glob::parse(p)).take(MAX_LIST).collect();
        PathGlob { globs }
    }

    /// `comps`: scratch buffer.
    pub fn matches<'p>(&self, path: &'p str, comps: &mut Vec<&'p str>) -> bool {
        comps.clear();
        comps.extend(path.split('/').filter(|c| !c.is_empty()));
        self.globs.iter().any(|g| g.matches(comps, false))
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
    fn braces() {
        let mut v = expand_braces("*.{rs,toml}");
        v.sort();
        assert_eq!(v, vec!["*.rs", "*.toml"]);
        let mut v = expand_braces("a{b,c{d,e}}f");
        v.sort();
        assert_eq!(v, vec!["abf", "acdf", "acef"]);
        assert_eq!(expand_braces("x{y"), vec!["x{y"]);
        assert_eq!(expand_braces("\\{a,b}"), vec!["\\{a,b}"]);
        assert!(expand_braces(&"{a,b}".repeat(40)).len() <= MAX_ALTS);
        let t = std::time::Instant::now();
        let _ = expand_braces(&"{".repeat(1000));
        let _ = expand_braces(&"{}".repeat(500));
        let _ = expand_braces(&"{a,b,c,d,e,f,g,h}".repeat(100));
        assert!(t.elapsed().as_millis() < 500);
    }

    #[test]
    fn filters() {
        let f = FileFilter::parse("*.rs, !gen/**, src/{a,b}/**", "target/");
        assert!(f.pass("x/main.rs"));
        assert!(!f.pass("gen/x.rs"));
        assert!(f.pass("src/a/notes.md"));
        assert!(!f.pass("src/c/notes.md"));
        assert!(!f.pass("target/debug/x.rs"));
        let f = FileFilter::parse("!*.md", "");
        assert!(f.pass("a.rs") && !f.pass("d/a.md"));
        assert!(FileFilter::parse(" , ", "").is_empty());
        let g = PathGlob::parse("src/*.{rs,md}");
        let mut c = Vec::new();
        assert!(g.matches("src/a.rs", &mut c) && g.matches("src/b.md", &mut c) && !g.matches("src/x/a.rs", &mut c));
        let g = PathGlob::parse("ma?n.*");
        assert!(g.matches("deep/dir/main.rs", &mut c) && !g.matches("main/x.rs", &mut c));
        assert!(PathGlob::is_glob("a*") && !PathGlob::is_glob("main"));
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
