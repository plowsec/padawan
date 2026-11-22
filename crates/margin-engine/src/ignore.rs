//! `.gitignore` rules and the per-directory stack used by the walker and `ListDir`.
//!
//! | Rule | Detail |
//! |---|---|
//! | precedence | deeper `.gitignore` first; inside one file the last matching line wins |
//! | `!pat` | re-include (no effect under an ignored directory: the walker does not enter it) |
//! | caps | 1 MiB read per file, 10_000 rules, 2_000 wildcard rules / 20_000 wildcard tokens (rest dropped) |
//! | fast paths | literal names and `*.ext` go to hash maps; only wildcard rules are scanned |
//! | always ignored | `.git`, `.hg`, `.svn`, `.DS_Store` |

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::glob::Glob;
use crate::jail::open_regular;

pub const MAX_IGNORE_BYTES: u64 = 1 << 20;
pub const MAX_RULES: usize = 10_000;
pub const MAX_GENERAL: usize = 2_000;
/// Sum of tokens over the wildcard rules of one file: bounds the work per path.
pub const MAX_GENERAL_COST: usize = 20_000;

pub fn always_skip(name: &str) -> bool {
    matches!(name, ".git" | ".hg" | ".svn" | ".DS_Store")
}

#[derive(Debug)]
struct Rule {
    glob: Glob,
    neg: bool,
}

#[derive(Debug, Default)]
pub struct IgnoreFile {
    rules: Vec<Rule>,
    by_name: HashMap<Box<str>, Vec<u32>>,
    by_suffix: HashMap<Box<str>, Vec<u32>>,
    general: Vec<u32>,
    cost: usize,
}

impl IgnoreFile {
    pub fn parse(text: &[u8]) -> IgnoreFile {
        let mut f = IgnoreFile::default();
        for raw in text.split(|&b| b == b'\n') {
            if f.rules.len() >= MAX_RULES {
                break;
            }
            let Ok(line) = std::str::from_utf8(raw) else { continue };
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = trim_trailing_spaces(line);
            let (neg, pat) = match line.strip_prefix('!') {
                Some(r) => (true, r),
                None => (false, line.strip_prefix("\\!").map_or(line, |_| &line[1..])),
            };
            let pat = if pat.starts_with("\\#") { &pat[1..] } else { pat };
            let Some(glob) = Glob::parse(pat) else { continue };
            let idx = f.rules.len() as u32;
            if let Some(n) = glob.literal_name() {
                f.by_name.entry(n.into()).or_default().push(idx);
            } else if let Some(s) = glob.star_suffix().filter(|s| s.starts_with('.') && !s[1..].contains('.')) {
                f.by_suffix.entry(s.into()).or_default().push(idx);
            } else {
                let c = glob.cost();
                if f.general.len() >= MAX_GENERAL || f.cost.saturating_add(c) > MAX_GENERAL_COST {
                    continue;
                }
                f.cost += c;
                f.general.push(idx);
            }
            f.rules.push(Rule { glob, neg });
        }
        f
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// `Some(true)`: ignored, `Some(false)`: re-included, `None`: no rule matched.
    pub fn decide(&self, comps: &[&str], is_dir: bool) -> Option<bool> {
        let name = *comps.last()?;
        let mut best: Option<u32> = None;
        let consider = |list: Option<&Vec<u32>>, best: &mut Option<u32>| {
            for &i in list.into_iter().flatten().rev() {
                if best.is_some_and(|b| b >= i) {
                    break;
                }
                if self.rules.get(i as usize).is_some_and(|r| !r.glob.dir_only || is_dir) {
                    *best = Some(i);
                    break;
                }
            }
        };
        consider(self.by_name.get(name), &mut best);
        if !self.by_suffix.is_empty() {
            if let Some(dot) = name.rfind('.') {
                consider(self.by_suffix.get(&name[dot..]), &mut best);
            }
        }
        for &i in self.general.iter().rev() {
            if best.is_some_and(|b| b >= i) {
                break;
            }
            if self.rules.get(i as usize).is_some_and(|r| r.glob.matches(comps, is_dir)) {
                best = Some(i);
                break;
            }
        }
        best.and_then(|i| self.rules.get(i as usize)).map(|r| !r.neg)
    }
}

fn trim_trailing_spaces(line: &str) -> &str {
    let mut end = line.len();
    let b = line.as_bytes();
    while end > 0 && b[end - 1] == b' ' {
        if end >= 2 && b[end - 2] == b'\\' {
            break;
        }
        end -= 1;
    }
    &line[..end]
}

/// Read a `.gitignore` (regular file, not through a symlink), first 1 MiB only.
pub fn load(path: &Path) -> Option<IgnoreFile> {
    let (f, _) = open_regular(path).ok()?;
    let mut buf = Vec::new();
    f.take(MAX_IGNORE_BYTES).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 >= MAX_IGNORE_BYTES {
        // Drop the cut line.
        let end = memchr::memrchr(b'\n', &buf).unwrap_or(0);
        buf.truncate(end);
    }
    let f = IgnoreFile::parse(&buf);
    (!f.is_empty()).then_some(f)
}

/// One `.gitignore` on the stack: applies to paths below `depth` components.
#[derive(Debug)]
pub struct Node {
    parent: Option<Arc<Node>>,
    depth: usize,
    file: IgnoreFile,
}

pub type Stack = Option<Arc<Node>>;

pub fn push(parent: &Stack, depth: usize, file: IgnoreFile) -> Stack {
    Some(Arc::new(Node { parent: parent.clone(), depth, file }))
}

/// `comps`: the path from the root.
pub fn is_ignored(stack: &Stack, comps: &[&str], is_dir: bool) -> bool {
    let mut cur = stack.as_ref();
    while let Some(n) = cur {
        if let Some(rel) = comps.get(n.depth..) {
            if let Some(d) = n.file.decide(rel, is_dir) {
                return d;
            }
        }
        cur = n.parent.as_ref();
    }
    false
}

/// Stack for directory `dir_comps` (reads each `.gitignore` from the root down) and whether the
/// directory itself is ignored. Directories under an ignored one are ignored.
pub fn stack_for_dir(root: &Path, dir_comps: &[&str], respect: bool) -> (Stack, bool) {
    let mut stack: Stack = None;
    let mut dir = root.to_path_buf();
    for (i, c) in dir_comps.iter().enumerate() {
        if respect {
            if let Some(f) = load(&dir.join(".gitignore")) {
                stack = push(&stack, i, f);
            }
        }
        let comps = &dir_comps[..=i];
        if always_skip(c) || (respect && is_ignored(&stack, comps, true)) {
            return (stack, true);
        }
        dir.push(c);
    }
    if respect {
        if let Some(f) = load(&dir.join(".gitignore")) {
            stack = push(&stack, dir_comps.len(), f);
        }
    }
    (stack, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ign(stack: &Stack, p: &str, dir: bool) -> bool {
        let c: Vec<&str> = p.split('/').collect();
        is_ignored(stack, &c, dir)
    }

    #[test]
    fn rules_table() {
        let f = IgnoreFile::parse(b"# c\n*.log\n!keep.log\n/build\ntarget/\ndocs/*.tmp\n\\#hash\n\\!bang\nsp\\ \ntrail   \n**/deep/x\n");
        let s = push(&None, 0, f);
        let cases: &[(&str, bool, bool)] = &[
            ("a.log", false, true),
            ("x/y/a.log", false, true),
            ("keep.log", false, false),
            ("x/keep.log", false, false),
            ("build", true, true),
            ("x/build", true, false),
            ("target", true, true),
            ("target", false, false),
            ("x/target", true, true),
            ("docs/a.tmp", false, true),
            ("x/docs/a.tmp", false, false),
            ("#hash", false, true),
            ("!bang", false, true),
            ("sp ", false, true),
            ("trail", false, true),
            ("a/b/deep/x", false, true),
            ("main.rs", false, false),
        ];
        for &(p, d, want) in cases {
            assert_eq!(ign(&s, p, d), want, "{p}");
        }
    }

    #[test]
    fn nested_override() {
        let root = push(&None, 0, IgnoreFile::parse(b"*.txt\n"));
        let sub = push(&root, 1, IgnoreFile::parse(b"!a.txt\n/only\n"));
        assert!(ign(&root, "sub/a.txt", false));
        assert!(!ign(&sub, "sub/a.txt", false));
        assert!(ign(&sub, "sub/b.txt", false));
        assert!(ign(&sub, "sub/only", false));
        assert!(!ign(&sub, "sub/x/only", false));
    }

    #[test]
    fn hostile_file_is_fast() {
        let mut text = String::new();
        for i in 0..20_000 {
            text.push_str(&format!("{}b{i}\n", "a*".repeat(30)));
        }
        let f = IgnoreFile::parse(text.as_bytes());
        assert!(f.general.len() <= MAX_GENERAL && f.rules.len() <= MAX_RULES);
        let s = push(&None, 0, f);
        let long = "a".repeat(255);
        let t = std::time::Instant::now();
        for _ in 0..20 {
            assert!(!ign(&s, &long, false));
        }
        assert!(t.elapsed().as_secs_f64() < 2.0, "{:?}", t.elapsed());
    }
}
