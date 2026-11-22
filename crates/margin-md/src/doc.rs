//! Parsed document: a flat list of leaf blocks, each pointing at its innermost container.
//!
//! | Item | What |
//! |---|---|
//! | `Leaf` | one drawable block (paragraph, heading, code chunk, table chunk, rule) + source line |
//! | `Ctr` | container (quote, list, item, footnote); `parent` chain gives the nesting |
//! | `Span` | run of inline text with one style; `link`/`img` index `links` |
//!
//! Long code blocks and tables are split into chunks (`CODE_CHUNK` lines, `TABLE_CHUNK` rows) so the
//! preview lays out only what is visible.

use margin_code::{Lang, LineState};

pub const NONE: u32 = u32::MAX;

/// Code lines per leaf.
pub const CODE_CHUNK: usize = 64;
/// Table rows per leaf (the header counts as a row of the first chunk).
pub const TABLE_CHUNK: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl Alert {
    pub fn from_tag(s: &str) -> Option<Alert> {
        let s = s.trim().strip_prefix("[!")?.strip_suffix(']')?;
        Some(match s.to_ascii_uppercase().as_str() {
            "NOTE" => Alert::Note,
            "TIP" => Alert::Tip,
            "IMPORTANT" => Alert::Important,
            "WARNING" => Alert::Warning,
            "CAUTION" => Alert::Caution,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Alert::Note => "Note",
            Alert::Tip => "Tip",
            Alert::Important => "Important",
            Alert::Warning => "Warning",
            Alert::Caution => "Caution",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CtrKind {
    Quote(Option<Alert>),
    List { ordered: bool, start: u64, loose: bool, max_num: u64 },
    /// `line`: source line of the marker (task toggles report it).
    Item { task: Option<bool>, num: u64, line: u32 },
    Foot { num: u32 },
}

#[derive(Clone, Copy, Debug)]
pub struct Ctr {
    pub kind: CtrKind,
    pub parent: u32,
    /// First leaf inside (markers are drawn next to it).
    pub first_leaf: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

pub mod f {
    pub const EM: u8 = 1;
    pub const STRONG: u8 = 2;
    pub const STRIKE: u8 = 4;
    pub const CODE: u8 = 8;
    pub const IMAGE: u8 = 16;
    pub const HTML: u8 = 32;
    pub const FOOT: u8 = 64;
}

/// Text already masked (`text::clean`). Hard break = `"\n"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub text: String,
    pub f: u8,
    /// Index into `links`, or `NONE`.
    pub link: u32,
    /// For `f::IMAGE`: index into `links` of the image source.
    pub img: u32,
}

#[derive(Clone, Debug)]
pub struct Link {
    /// As written (after escapes/entities). Hostile: returned to the app, never opened.
    pub dest: String,
    /// Masked, for tooltips and image placeholders.
    pub shown: String,
    pub title: String,
    /// Footnote reference: its number.
    pub foot: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextKind {
    Para,
    Heading(u8),
    Title(Alert),
    /// Empty list item / quote: keeps the marker visible.
    Empty,
}

#[derive(Clone, Debug, Hash)]
pub enum Body {
    Text { kind: TextKind, spans: Vec<Span> },
    Code { lang: Lang, label: String, lines: Vec<String>, first: bool, last: bool, state: LineState, muted: bool },
    /// `row0`: index of the first row in the table (0 = the header row).
    Table { tid: u32, row0: u32, rows: Vec<Vec<Vec<Span>>>, last: bool },
    Rule,
}

/// Vertical gap above a leaf, resolved to pixels by the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gap {
    /// First leaf: top padding of the page.
    Start,
    /// Next chunk of the same code block / table.
    Cont,
    /// Items of a tight list.
    Tight,
    /// Collapsed margins: max(prev bottom, this top).
    Normal { prev: Class, cur: Class },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Para,
    H(u8),
    Block,
    Rule,
    Title,
}

#[derive(Clone, Debug)]
pub struct Leaf {
    pub line: u32,
    pub ctr: u32,
    pub gap: Gap,
    /// Content + layout-relevant context. Layout cache key.
    pub hash: u64,
    /// Rough size for height estimates: chars (text), lines (code), rows (table).
    pub weight: u32,
    pub body: Body,
}

#[derive(Clone, Debug)]
pub struct TableInfo {
    pub aligns: Vec<Align>,
    pub first_leaf: u32,
    pub n_leaves: u32,
}

#[derive(Debug, Default)]
pub struct Inner {
    pub id: u64,
    pub leaves: Vec<Leaf>,
    pub ctrs: Vec<Ctr>,
    pub links: Vec<Link>,
    pub tables: Vec<TableInfo>,
    pub headings: Vec<(u8, String, u32)>,
    /// `(slug, line)` in heading order, de-duplicated GitHub-style.
    pub slugs: Vec<(String, u32)>,
    /// `(line, leaf)` sorted by line.
    pub by_line: Vec<(u32, u32)>,
    /// Footnote number → its first leaf.
    pub foot_leaf: Vec<(u32, u32)>,
    pub truncated: bool,
}

impl Inner {
    /// Ancestors of `ctr`, innermost first (bounded).
    pub fn chain(&self, ctr: u32, out: &mut Vec<u32>) {
        out.clear();
        let mut c = ctr;
        while c != NONE && out.len() < 64 {
            out.push(c);
            c = self.ctrs.get(c as usize).map_or(NONE, |x| x.parent);
        }
    }

    pub fn ctr(&self, c: u32) -> Option<&Ctr> {
        self.ctrs.get(c as usize)
    }

    /// Leaf holding source line `line`: the last leaf starting at or before it.
    pub fn leaf_at_line(&self, line: u32) -> usize {
        let i = self.by_line.partition_point(|&(l, _)| l <= line);
        if i == 0 {
            return self.by_line.first().map_or(0, |&(_, x)| x as usize);
        }
        self.by_line.get(i - 1).map_or(0, |&(_, x)| x as usize)
    }

    /// Compact dump, one leaf per line: `line [containers] kind content`. For tests.
    pub fn dump(&self) -> String {
        let mut s = String::new();
        let mut ch = Vec::new();
        for l in &self.leaves {
            s.push_str(&l.line.to_string());
            self.chain(l.ctr, &mut ch);
            if !ch.is_empty() {
                s.push_str(" [");
                for (k, &c) in ch.iter().rev().enumerate() {
                    if k > 0 {
                        s.push(' ');
                    }
                    let Some(c) = self.ctr(c) else { continue };
                    match c.kind {
                        CtrKind::Quote(None) => s.push('>'),
                        CtrKind::Quote(Some(a)) => {
                            s.push_str(">!");
                            s.push_str(a.label());
                        }
                        CtrKind::List { ordered, start, loose, .. } => {
                            if ordered {
                                s.push_str(&format!("ol{start}"));
                            } else {
                                s.push_str("ul");
                            }
                            s.push_str(if loose { ":l" } else { ":t" });
                        }
                        CtrKind::Item { task, .. } => {
                            s.push_str("li");
                            match task {
                                Some(true) => s.push_str("[x]"),
                                Some(false) => s.push_str("[ ]"),
                                None => {}
                            }
                        }
                        CtrKind::Foot { num } => s.push_str(&format!("fn{num}")),
                    }
                }
                s.push(']');
            }
            s.push(' ');
            match &l.body {
                Body::Text { kind, spans } => {
                    match kind {
                        TextKind::Para => s.push('p'),
                        TextKind::Heading(n) => s.push_str(&format!("h{n}")),
                        TextKind::Title(a) => {
                            s.push_str("title ");
                            s.push_str(a.label());
                        }
                        TextKind::Empty => s.push_str("empty"),
                    }
                    if !spans.is_empty() {
                        s.push(' ');
                        self.dump_spans(&mut s, spans);
                    }
                }
                Body::Code { label, lines, muted, .. } => {
                    s.push_str(if *muted { "raw" } else { "code" });
                    if !label.is_empty() {
                        s.push(':');
                        s.push_str(label);
                    }
                    s.push(' ');
                    s.push_str(&format!("{:?}", lines.join("\n")));
                }
                Body::Table { tid, row0, rows, .. } => {
                    let al = self.tables.get(*tid as usize).map(|t| &t.aligns[..]).unwrap_or(&[]);
                    s.push_str("table");
                    if *row0 == 0 {
                        s.push(' ');
                        for a in al {
                            s.push(match a {
                                Align::None => '-',
                                Align::Left => 'l',
                                Align::Center => 'c',
                                Align::Right => 'r',
                            });
                        }
                    }
                    for r in rows {
                        s.push_str(" |");
                        for c in r {
                            self.dump_spans(&mut s, c);
                            s.push('|');
                        }
                    }
                }
                Body::Rule => s.push_str("hr"),
            }
            s.push('\n');
        }
        s
    }

    fn dump_spans(&self, s: &mut String, spans: &[Span]) {
        let mut inner = String::new();
        for sp in spans {
            inner.clear();
            if sp.f & f::IMAGE != 0 {
                let src = self.links.get(sp.img as usize).map_or("", |l| l.dest.as_str());
                inner.push_str(&format!("{{img:{}|{}}}", sp.text, src));
            } else if sp.f & f::CODE != 0 {
                inner.push('`');
                inner.push_str(&sp.text);
                inner.push('`');
            } else if sp.f & f::FOOT != 0 {
                inner.push_str(&format!("^{}", sp.text));
            } else if sp.f & f::HTML != 0 {
                inner.push_str(&format!("{{html:{}}}", sp.text));
            } else {
                inner.push_str(&sp.text.replace('\n', "\\n"));
            }
            if sp.f & f::EM != 0 {
                inner = format!("*{inner}*");
            }
            if sp.f & f::STRONG != 0 {
                inner = format!("**{inner}**");
            }
            if sp.f & f::STRIKE != 0 {
                inner = format!("~~{inner}~~");
            }
            if sp.link != NONE && sp.f & f::FOOT == 0 {
                let d = self.links.get(sp.link as usize).map_or("", |l| l.dest.as_str());
                inner = format!("[{inner}]({d})");
            }
            s.push_str(&inner);
        }
    }
}
