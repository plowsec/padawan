//! Raw blocks → final document: inline parsing, code/table chunking, footnotes moved to the end,
//! first-leaf links, gaps, layout hashes, headings and slugs.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use margin_code::{lex_line, Lang, LineState};

use crate::block::{parse_blocks, RawBody, RawLeaf};
use crate::doc::{f, Body, Class, CtrKind, Gap, Inner, Leaf, Span, TableInfo, TextKind, CODE_CHUNK, NONE, TABLE_CHUNK};
use crate::inline::{parse_inline, Cx};
use crate::text::{cap, clean, floor, slug};

/// Code line bytes shown (rest: `…`).
pub const CODE_LINE_CAP: usize = 4096;
/// Table rows sampled for column widths.
pub const TABLE_SAMPLE: usize = 200;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn lang_of(info: &str) -> Lang {
    let w = info.split_whitespace().next().unwrap_or("").trim_start_matches(['{', '.']).trim_end_matches('}');
    let w = w.split([',', '{']).next().unwrap_or("").to_ascii_lowercase();
    match w.as_str() {
        "" => Lang::Plain,
        "rust" | "rs" => Lang::Rust,
        "python" | "py" | "python3" | "py3" => Lang::Python,
        "javascript" | "js" | "jsx" | "node" | "mjs" => Lang::JavaScript,
        "typescript" | "ts" | "tsx" => Lang::TypeScript,
        "bash" | "sh" | "shell" | "zsh" | "console" | "shellsession" | "ksh" | "dockerfile" | "docker" => Lang::Shell,
        "c" | "h" => Lang::C,
        "cpp" | "c++" | "cxx" | "hpp" | "cc" => Lang::Cpp,
        "csharp" | "cs" | "c#" => Lang::CSharp,
        "go" | "golang" => Lang::Go,
        "java" => Lang::Java,
        "kotlin" | "kt" => Lang::Kotlin,
        "swift" => Lang::Swift,
        "ruby" | "rb" => Lang::Ruby,
        "php" => Lang::Php,
        "lua" => Lang::Lua,
        "json" | "jsonc" | "json5" => Lang::Json,
        "toml" => Lang::Toml,
        "yaml" | "yml" => Lang::Yaml,
        "html" | "xml" | "svg" | "xhtml" => Lang::Html,
        "css" | "scss" | "less" => Lang::Css,
        "sql" => Lang::Sql,
        "make" | "makefile" | "mk" => Lang::Makefile,
        "diff" | "patch" => Lang::Diff,
        "markdown" | "md" => Lang::Markdown,
        w if w.len() <= 16 && w.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-') => Lang::from_path(&format!("x.{w}")),
        _ => Lang::Plain,
    }
}

fn code_line(l: &str) -> String {
    let mut s = clean(cap(l, CODE_LINE_CAP));
    if l.len() > CODE_LINE_CAP {
        s.push('\u{2026}');
    }
    s
}

pub fn build(text: &str) -> Inner {
    let mut truncated = false;
    let mut text = text;
    if text.len() > crate::MAX_SOURCE {
        let cut = floor(text, crate::MAX_SOURCE);
        let head = text.get(..cut).unwrap_or("");
        let at = head.rfind('\n').map_or(cut, |i| i + 1);
        text = text.get(..at).unwrap_or("");
        truncated = true;
    }
    let blocks = parse_blocks(text);
    let mut d = Inner { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), ctrs: blocks.ctrs, truncated: truncated || blocks.truncated, ..Default::default() };

    // Footnote definitions go last, after a rule.
    let in_foot = |ctrs: &[crate::doc::Ctr], mut c: u32| {
        let mut n = 0;
        while c != NONE && n < 64 {
            let Some(x) = ctrs.get(c as usize) else { break };
            if matches!(x.kind, CtrKind::Foot { .. }) {
                return true;
            }
            c = x.parent;
            n += 1;
        }
        false
    };
    let (mut main, feet): (Vec<RawLeaf>, Vec<RawLeaf>) = blocks.leaves.into_iter().partition(|l| !in_foot(&d.ctrs, l.ctr));
    if let Some(first) = feet.first() {
        main.push(RawLeaf { line: first.line, ctr: NONE, body: RawBody::Rule });
        main.extend(feet);
    }

    let refs = blocks.refs;
    let feet_map = blocks.feet;
    let mut links = Vec::new();
    let mut seen: HashMap<String, u32> = HashMap::new();
    for raw in main {
        let mut cx = Cx { refs: &refs, feet: &feet_map, links: &mut links };
        let start = d.leaves.len() as u32;
        let ctr = raw.ctr;
        match raw.body {
            RawBody::Para(s) => {
                let spans = parse_inline(&s, &mut cx);
                push_text(&mut d, raw.line, ctr, TextKind::Para, spans);
            }
            RawBody::Heading(level, s) => {
                let spans = parse_inline(&s, &mut cx);
                let plain: String = spans.iter().map(|s| s.text.as_str()).collect::<String>().replace('\n', " ");
                let base = slug(&plain);
                let mut sl = base.clone();
                if seen.contains_key(&sl) {
                    let mut n = seen.get(&base).copied().unwrap_or(0);
                    loop {
                        n += 1;
                        sl = format!("{base}-{n}");
                        if !seen.contains_key(&sl) {
                            break;
                        }
                    }
                    seen.insert(base.clone(), n);
                }
                seen.insert(sl.clone(), 0);
                d.slugs.push((sl, raw.line));
                d.headings.push((level, plain.trim().to_owned(), raw.line));
                push_text(&mut d, raw.line, ctr, TextKind::Heading(level), spans);
            }
            RawBody::Title(a) => push_text(&mut d, raw.line, ctr, TextKind::Title(a), Vec::new()),
            RawBody::Empty => push_text(&mut d, raw.line, ctr, TextKind::Empty, Vec::new()),
            RawBody::Rule => push(&mut d, raw.line, ctr, 1, Body::Rule),
            RawBody::Code { info, lines, muted, lang } => {
                let lang = lang.unwrap_or_else(|| lang_of(&info));
                let label = if muted { info.clone() } else { clean(cap(info.split_whitespace().next().unwrap_or(""), 24)) };
                let lines: Vec<String> = lines.iter().map(|l| code_line(l)).collect();
                let n = lines.len().max(1);
                let chunks = n.div_ceil(CODE_CHUNK);
                let mut state = LineState::default();
                let mut toks = Vec::new();
                let mut it = lines.into_iter();
                for k in 0..chunks {
                    let part: Vec<String> = it.by_ref().take(CODE_CHUNK).collect();
                    let st = state;
                    if chunks > 1 && !muted && lang != Lang::Plain {
                        for l in &part {
                            toks.clear();
                            state = lex_line(lang, state, l, &mut toks);
                        }
                    }
                    let w = part.len() as u32;
                    let body = Body::Code { lang, label: label.clone(), lines: part, first: k == 0, last: k + 1 == chunks, state: st, muted };
                    push(&mut d, raw.line + (k * CODE_CHUNK) as u32, ctr, w, body);
                }
            }
            RawBody::Table { aligns, rows } => {
                let tid = d.tables.len() as u32;
                let ncols = aligns.len();
                d.tables.push(TableInfo { aligns, first_leaf: start, n_leaves: 0 });
                let total = rows.len();
                let mut it = rows.into_iter();
                let mut r0 = 0usize;
                while r0 < total {
                    let part: Vec<Vec<Vec<Span>>> = it
                        .by_ref()
                        .take(TABLE_CHUNK)
                        .map(|r| r.iter().take(ncols).map(|c| parse_inline(c, &mut Cx { refs: &refs, feet: &feet_map, links: &mut links })).collect())
                        .collect();
                    let n = part.len();
                    let line = raw.line + r0 as u32 + u32::from(r0 > 0);
                    let body = Body::Table { tid, row0: r0 as u32, rows: part, last: r0 + n >= total };
                    push(&mut d, line, ctr, n as u32, body);
                    r0 += n.max(1);
                }
                if let Some(t) = d.tables.last_mut() {
                    t.n_leaves = d.leaves.len() as u32 - start;
                }
            }
        }
        // First leaf of each container.
        if (d.leaves.len() as u32) > start {
            let mut c = ctr;
            let mut guard = 0;
            while c != NONE && guard < 64 {
                let Some(x) = d.ctrs.get_mut(c as usize) else { break };
                if x.first_leaf != NONE {
                    break;
                }
                x.first_leaf = start;
                c = x.parent;
                guard += 1;
            }
        }
    }
    d.links = links;
    finish(&mut d);
    d
}

fn push_text(d: &mut Inner, line: u32, ctr: u32, kind: TextKind, spans: Vec<Span>) {
    let w = spans.iter().map(|s| s.text.len() as u32 + if s.f & f::IMAGE != 0 { 24 } else { 0 }).sum::<u32>();
    push(d, line, ctr, w, Body::Text { kind, spans });
}

fn push(d: &mut Inner, line: u32, ctr: u32, weight: u32, body: Body) {
    d.leaves.push(Leaf { line, ctr, gap: Gap::Start, hash: 0, weight, body });
}

fn class(b: &Body) -> Class {
    match b {
        Body::Text { kind: TextKind::Heading(l), .. } => Class::H(*l),
        Body::Text { kind: TextKind::Title(_), .. } => Class::Title,
        Body::Text { .. } => Class::Para,
        Body::Code { .. } | Body::Table { .. } => Class::Block,
        Body::Rule => Class::Rule,
    }
}

fn texty(b: &Body) -> bool {
    matches!(b, Body::Text { kind: TextKind::Para | TextKind::Empty, .. })
}

fn digits(mut n: u64) -> u8 {
    let mut d = 1;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

/// Gaps, hashes, line index, footnote targets.
fn finish(d: &mut Inner) {
    let mut ch = Vec::new();
    let mut prev_ch: Vec<u32> = Vec::new();
    // Per-table hash of the sampled rows (column widths depend on them).
    let mut table_hash = vec![0u64; d.tables.len()];
    for (t, th) in d.tables.iter().zip(table_hash.iter_mut()) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        t.aligns.hash(&mut h);
        let mut rows = 0;
        for l in d.leaves.iter().skip(t.first_leaf as usize).take(t.n_leaves as usize) {
            if let Body::Table { rows: r, .. } = &l.body {
                for row in r {
                    if rows > TABLE_SAMPLE {
                        break;
                    }
                    row.hash(&mut h);
                    rows += 1;
                }
            }
        }
        *th = h.finish();
    }
    for i in 0..d.leaves.len() {
        d.chain(d.leaves[i].ctr, &mut ch);
        let gap = if i == 0 {
            Gap::Start
        } else {
            let (cur, prev) = (&d.leaves[i].body, &d.leaves[i - 1].body);
            let cont = matches!(cur, Body::Code { first: false, .. }) || matches!(cur, Body::Table { row0, .. } if *row0 > 0);
            if cont {
                Gap::Cont
            } else {
                let mut g = Gap::Normal { prev: class(prev), cur: class(cur) };
                if texty(cur) && texty(prev) {
                    for &c in &ch {
                        if let Some(CtrKind::List { loose, .. }) = d.ctr(c).map(|x| x.kind) {
                            if prev_ch.contains(&c) {
                                if !loose {
                                    g = Gap::Tight;
                                }
                                break;
                            }
                        }
                    }
                }
                g
            }
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        d.leaves[i].body.hash(&mut h);
        for &c in &ch {
            match d.ctr(c).map(|x| x.kind) {
                Some(CtrKind::Quote(a)) => (0u8, a.is_some()).hash(&mut h),
                Some(CtrKind::List { ordered, max_num, .. }) => (1u8, ordered, digits(max_num)).hash(&mut h),
                Some(CtrKind::Item { task, .. }) => (2u8, task.is_some()).hash(&mut h),
                Some(CtrKind::Foot { .. }) => 3u8.hash(&mut h),
                None => {}
            }
        }
        if let Body::Table { tid, .. } = d.leaves[i].body {
            table_hash.get(tid as usize).hash(&mut h);
        }
        let leaf = &mut d.leaves[i];
        leaf.gap = gap;
        leaf.hash = h.finish();
        std::mem::swap(&mut prev_ch, &mut ch);
    }
    d.by_line = d.leaves.iter().enumerate().map(|(i, l)| (l.line, i as u32)).collect();
    d.by_line.sort_by_key(|&(l, _)| l);
    for c in &d.ctrs {
        if let CtrKind::Foot { num } = c.kind {
            if c.first_leaf != NONE {
                d.foot_leaf.push((num, c.first_leaf));
            }
        }
    }
}
