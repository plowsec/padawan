//! Walk rules and content search on real files (through `TempDir`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};

use margin_engine::index::{index_file, FileRec};
use margin_engine::{search, walk};
use margin_proto::limits::MAX_SNIPPET;
use margin_proto::{ErrCode, IndexOpts, SearchQuery, SearchStats, Site};
use margin_testutil::TempDir;

fn pool() -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap()
}

fn table(root: &std::path::Path, opts: &IndexOpts) -> (PathBuf, Vec<Arc<FileRec>>, u32) {
    let root = std::fs::canonicalize(root).unwrap();
    let w = walk::walk(&pool(), &root, opts, &AtomicBool::new(false), &AtomicU32::new(0));
    let mut buf = Vec::new();
    let files = w.files.iter().map(|e| Arc::new(index_file(&root.join(&e.path), &e.path, e.size, e.mtime_ns, opts.max_file_bytes, &mut buf))).collect();
    (root, files, w.skipped)
}

fn paths(files: &[Arc<FileRec>]) -> Vec<&str> {
    files.iter().map(|f| &*f.path).collect()
}

fn q(pat: &str) -> SearchQuery {
    SearchQuery { pattern: pat.into(), max_hits: 1000, ..Default::default() }
}

fn run(root: &std::path::Path, files: &[Arc<FileRec>], query: &SearchQuery) -> Result<(Vec<Site>, SearchStats), ErrCode> {
    let plan = search::plan(query).map_err(|e| e.0)?;
    let got = Mutex::new(Vec::new());
    let emit = |v: Vec<Site>| got.lock().unwrap().extend(v);
    let stats = search::run(&pool(), root, files, &plan, 2 << 20, &AtomicBool::new(false), &emit)?;
    let mut v = got.into_inner().unwrap();
    v.sort_by(|a, b| (a.path.as_str(), a.line, a.col).cmp(&(b.path.as_str(), b.line, b.col)));
    Ok((v, stats))
}

fn check_site(s: &Site) {
    assert!(s.text.len() <= MAX_SNIPPET);
    assert!(s.text.get(s.text_col as usize..(s.text_col + s.len) as usize).is_some(), "{s:?}");
}

#[test]
fn walk_rules() {
    let t = TempDir::new("walk");
    t.write(".gitignore", "*.log\n/build/\n!keep.log\nsub/only.txt\n");
    t.write("a.rs", "fn a() {}");
    t.write("x.log", "log");
    t.write("keep.log", "kept");
    t.write("build/out.o", "o");
    t.write("src/build/in.rs", "not root build");
    t.write("sub/.gitignore", "*.tmp\n!important.tmp\n");
    t.write("sub/a.tmp", "t");
    t.write("sub/important.tmp", "t");
    t.write("sub/only.txt", "o");
    t.write(".git/HEAD", "ref");
    t.write(".hidden/h.txt", "h");
    t.write("node/.DS_Store", "x");
    #[cfg(unix)]
    std::os::unix::fs::symlink(t.join("src"), t.join("srclink")).unwrap();
    let (_, files, skipped) = table(t.path(), &IndexOpts::default());
    assert_eq!(paths(&files), vec![".gitignore", ".hidden/h.txt", "a.rs", "keep.log", "src/build/in.rs", "sub/.gitignore", "sub/important.tmp"]);
    assert!(skipped >= 6, "{skipped}");
    let opts = IndexOpts { respect_gitignore: false, index_hidden: false, ..Default::default() };
    let (_, files, _) = table(t.path(), &opts);
    assert_eq!(paths(&files), vec!["a.rs", "build/out.o", "keep.log", "src/build/in.rs", "sub/a.tmp", "sub/important.tmp", "sub/only.txt", "x.log"]);
}

#[test]
fn big_and_binary_are_listed_not_searched() {
    let t = TempDir::new("big");
    t.write("big.txt", "needle ".repeat(100));
    t.write("bin.dat", b"needle\0needle");
    t.write("small.txt", "needle");
    let opts = IndexOpts { max_file_bytes: 100, ..Default::default() };
    let (root, files, _) = table(t.path(), &opts);
    assert_eq!(files.len(), 3);
    let plan = search::plan(&q("needle")).unwrap();
    let got = Mutex::new(Vec::new());
    let emit = |v: Vec<Site>| got.lock().unwrap().extend(v);
    let st = search::run(&pool(), &root, &files, &plan, 100, &AtomicBool::new(false), &emit).unwrap();
    assert_eq!(st.files_scanned, 1);
    assert_eq!(got.into_inner().unwrap().len(), 1);
}

#[test]
fn literal_regex_case_word_globs() {
    let t = TempDir::new("search");
    t.write("src/a.rs", "fn foo() {}\nlet FooBar = foo_bar;\r\nfoo foo\n");
    t.write("src/b.md", "Foo in docs\n");
    t.write("vendor/c.rs", "foo vendored\n");
    t.write("ünï.txt", "héllo wörld Héllo\n");
    let (root, files, _) = table(t.path(), &IndexOpts::default());

    let (v, st) = run(&root, &files, &q("foo")).unwrap();
    assert_eq!(v.len(), 7, "{v:#?}");
    assert_eq!(st.hits, 7);
    assert_eq!(st.files_matched, 3);
    for s in &v {
        check_site(s);
        assert_eq!(s.text[s.text_col as usize..][..s.len as usize].to_lowercase(), "foo");
    }
    let s = v.iter().find(|s| s.path.as_str() == "src/a.rs" && s.line == 1).unwrap();
    assert_eq!((s.col, s.text.as_str()), (4, "let FooBar = foo_bar;"));

    let cs = SearchQuery { case_sensitive: true, ..q("Foo") };
    assert_eq!(run(&root, &files, &cs).unwrap().0.len(), 2);

    let word = SearchQuery { whole_word: true, ..q("foo") };
    let (v, _) = run(&root, &files, &word).unwrap();
    assert_eq!(v.iter().map(|s| (s.path.as_str(), s.line, s.col)).collect::<Vec<_>>(), vec![("src/a.rs", 0, 3), ("src/a.rs", 2, 0), ("src/a.rs", 2, 4), ("src/b.md", 0, 0), ("vendor/c.rs", 0, 0)]);

    let inc = SearchQuery { include: "*.rs".into(), exclude: "vendor".into(), ..q("foo") };
    assert!(run(&root, &files, &inc).unwrap().0.iter().all(|s| s.path.as_str() == "src/a.rs"));
    let inc = SearchQuery { include: "src/**".into(), exclude: "*.md".into(), ..q("foo") };
    assert!(run(&root, &files, &inc).unwrap().0.iter().all(|s| s.path.as_str() == "src/a.rs"));

    let re = SearchQuery { regex: true, ..q(r"foo\w+") };
    let (v, _) = run(&root, &files, &re).unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].len, 6);
    let re = SearchQuery { regex: true, whole_word: true, case_sensitive: true, ..q("foo|Foo") };
    assert_eq!(run(&root, &files, &re).unwrap().0.len(), 5);
    // Multi-line match: len clipped to the first line. Empty matches skipped.
    let re = SearchQuery { regex: true, ..q(r"\{\}\nlet") };
    let (v, _) = run(&root, &files, &re).unwrap();
    assert_eq!((v.len(), v[0].len), (1, 2));
    let re = SearchQuery { regex: true, ..q("x*") };
    assert!(run(&root, &files, &re).unwrap().0.is_empty());

    // Unicode, case-insensitive (Unicode folding for non-ASCII literals).
    let (v, _) = run(&root, &files, &q("héllo")).unwrap();
    assert_eq!(v.len(), 2);
    v.iter().for_each(check_site);

    let bad = SearchQuery { regex: true, ..q("(unclosed") };
    assert_eq!(run(&root, &files, &bad).unwrap_err(), ErrCode::Invalid);
    let huge = SearchQuery { regex: true, ..q(r"\w{1000}{1000}") };
    assert_eq!(run(&root, &files, &huge).unwrap_err(), ErrCode::Invalid);
}

#[test]
fn snippets_on_long_and_multibyte_lines() {
    let t = TempDir::new("long");
    let mut line = "ж".repeat(3000);
    line.push_str("TARGET");
    line.push_str(&"€".repeat(3000));
    t.write("long.txt", format!("first\n{line}\n"));
    let (root, files, _) = table(t.path(), &IndexOpts::default());
    let (v, _) = run(&root, &files, &q("target")).unwrap();
    assert_eq!(v.len(), 1);
    let s = &v[0];
    check_site(s);
    assert_eq!((s.line, s.col, s.len), (1, 6000, 6));
    assert_eq!(&s.text[s.text_col as usize..][..6], "TARGET");
    let re = SearchQuery { regex: true, ..q("ж+") };
    let (v, _) = run(&root, &files, &re).unwrap();
    check_site(&v[0]);
    assert!(v[0].len as usize <= MAX_SNIPPET);
}

#[test]
fn max_hits_truncates_and_bloom_prunes() {
    let t = TempDir::new("trunc");
    for i in 0..40 {
        t.write(&format!("f{i}.txt"), "hit\n".repeat(10));
    }
    t.write("other.txt", "unrelated content only");
    let (root, files, _) = table(t.path(), &IndexOpts::default());
    let lim = SearchQuery { max_hits: 25, ..q("hit") };
    let (v, st) = run(&root, &files, &lim).unwrap();
    assert_eq!(v.len(), 25);
    assert!(st.truncated);
    let (_, st) = run(&root, &files, &q("unrelated")).unwrap();
    assert!(st.files_scanned < 5, "bloom should prune: {st:?}");
    assert_eq!(st.hits, 1);
    let (v, st) = run(&root, &files, &q("hit\nhit")).unwrap();
    assert!(!st.truncated && !v.is_empty());
}

#[test]
fn cancel_ends_the_stream() {
    let t = TempDir::new("cancel");
    for i in 0..50 {
        t.write(&format!("f{i}.txt"), "x".repeat(1000));
    }
    let (root, files, _) = table(t.path(), &IndexOpts::default());
    let plan = search::plan(&q("x")).unwrap();
    let r = search::run(&pool(), &root, &files, &plan, 2 << 20, &AtomicBool::new(true), &|_| {});
    assert_eq!(r.unwrap_err(), ErrCode::Cancelled);
}

#[test]
fn per_file_cap() {
    let t = TempDir::new("cap");
    t.write("many.txt", "a ".repeat(5000));
    let (root, files, _) = table(t.path(), &IndexOpts::default());
    let (v, st) = run(&root, &files, &SearchQuery { max_hits: 0, ..q("a") }).unwrap();
    assert_eq!(v.len(), search::PER_FILE);
    assert!(st.truncated);
}
