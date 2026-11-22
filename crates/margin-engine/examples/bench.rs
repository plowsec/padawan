//! Engine bench. Read-only on the folder; the cache goes to a `TempDir`.
//!
//! `cargo run --release --offline -p margin-engine --example bench -- <dir> [--no-ignore] [--threads N]`
//! `... --example bench -- --synth <files> <bytes-per-file>`: generated corpus in a `TempDir`.

use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use margin_engine::index::{index_file, FileRec};
use margin_engine::{cache, fuzzy, search, walk, EngineConfig};
use margin_proto::{Event, IndexOpts, IndexPhase, IndexStatus, Out, SearchQuery, Site};
use margin_testutil::TempDir;
use rayon::prelude::*;

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1e3)
}

fn engine_ready(cfg: EngineConfig) -> (Duration, IndexStatus) {
    let t = Instant::now();
    let c = margin_engine::spawn_local(cfg, Arc::new(|| {}));
    loop {
        match c.poll() {
            Some(Out::Event(Event::Index(s))) if s.phase == IndexPhase::Ready => return (t.elapsed(), s),
            Some(Out::Event(Event::Fatal { msg })) => panic!("{msg}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_micros(200)),
        }
    }
}

/// `n` files of ~`size` bytes of pseudo-random code-like words, 100 per directory.
fn synth(t: &TempDir, n: usize, size: usize) {
    let words = ["fn", "let", "mut", "self", "return", "match", "struct", "impl", "parse_header", "buffer", "len", "Vec", "Option", "unwrap", "config", "{", "}", "(", ")", ";", "=", "->", "if", "else", "for", "in", "0x1f", "error", "handle_request", "value"];
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..n {
        let mut s = String::with_capacity(size + 64);
        let mut col = 0;
        while s.len() < size {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let w = words[(x % words.len() as u64) as usize];
            s.push_str(w);
            col += w.len() + 1;
            if col > 80 {
                s.push('\n');
                col = 0;
            } else {
                s.push(' ');
            }
        }
        if i % 97 == 0 {
            s.push_str("rare_marker_token\n");
        }
        t.write(&format!("d{}/f{i}.rs", i / 100), s);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let synth_dir;
    let dir: String = if args.get(1).map(String::as_str) == Some("--synth") {
        let n = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100_000);
        let size = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1000);
        synth_dir = TempDir::new("bench-synth");
        let t = Instant::now();
        synth(&synth_dir, n, size);
        eprintln!("synth: {n} files x {size} B in {}", ms(t.elapsed()));
        synth_dir.path().to_string_lossy().into_owned()
    } else {
        match args.get(1) {
            Some(d) => d.clone(),
            None => {
                eprintln!("usage: bench <dir> [--no-ignore] [--threads N] | bench --synth <files> <bytes>");
                return;
            }
        }
    };
    let dir = &dir;
    let no_ignore = args.iter().any(|a| a == "--no-ignore");
    let threads = args.iter().position(|a| a == "--threads").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(0);
    if let Some(n) = args.iter().position(|a| a == "--io-slots").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()) {
        margin_engine::jail::set_io_slots(n);
    }
    let root = std::fs::canonicalize(dir).expect("folder");
    let opts = IndexOpts { respect_gitignore: !no_ignore, ..Default::default() };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
    eprintln!("folder {} | threads {} | gitignore {}", root.display(), pool.current_num_threads(), !no_ignore);

    // Walk (twice: cold-ish, warm).
    let stop = AtomicBool::new(false);
    let mut w = walk::WalkOut::default();
    for i in 0..2 {
        let t = Instant::now();
        w = walk::walk(&pool, &root, &opts, &stop, &AtomicU32::new(0));
        eprintln!("walk #{i}: {} files, {} skipped, {}", w.files.len(), w.skipped, ms(t.elapsed()));
    }

    // Index.
    let t = Instant::now();
    let files: Vec<Arc<FileRec>> = pool.install(|| {
        w.files.par_iter().map_init(Vec::new, |buf, e| Arc::new(index_file(&root.join(&e.path), &e.path, e.size, e.mtime_ns, opts.max_file_bytes, buf))).collect()
    });
    let el = t.elapsed();
    let bytes: u64 = files.iter().map(|f| f.content_bytes()).sum();
    let bloom: usize = files.iter().map(|f| f.bloom.len()).sum();
    let indexed = files.iter().filter(|f| f.indexed()).count();
    eprintln!(
        "index: {indexed} files, {:.1} MB content, {} → {:.0} MB/s; bloom {:.2} MB ({:.1}% of content)",
        bytes as f64 / 1e6,
        ms(el),
        bytes as f64 / 1e6 / el.as_secs_f64(),
        bloom as f64 / 1e6,
        bloom as f64 * 100.0 / bytes.max(1) as f64
    );

    // Cache save / load.
    let tmp = TempDir::new("bench-cache");
    let cdir = tmp.path().join("cache");
    let rs = root.to_string_lossy().into_owned();
    let t = Instant::now();
    cache::save(&cdir, &rs, &files).expect("save");
    let sz = std::fs::metadata(cdir.join(cache::FILE_NAME)).map(|m| m.len()).unwrap_or(0);
    eprintln!("cache save: {:.1} MB, {}", sz as f64 / 1e6, ms(t.elapsed()));
    let t = Instant::now();
    let loaded = cache::load(&cdir, &rs).expect("load");
    eprintln!("cache load: {} entries, {}", loaded.len(), ms(t.elapsed()));

    // Whole engine: cold (empty cache) and warm start, to `Ready`.
    let etmp = TempDir::new("bench-engine");
    let ecfg = EngineConfig { root: root.clone(), cache_dir: Some(etmp.path().join("c")), opts: opts.clone(), threads };
    let (d, s) = engine_ready(ecfg.clone());
    eprintln!("engine cold start → Ready: {} ({} files, {} from cache, {} symbols)", ms(d), s.files_total, s.from_cache, s.symbols);
    std::thread::sleep(Duration::from_millis(300));
    let (d, s) = engine_ready(ecfg);
    eprintln!("engine warm start → Ready: {} ({} files, {} from cache, {} symbols)", ms(d), s.files_total, s.from_cache, s.symbols);

    // Fuzzy: real table, then the table replicated to ≥ 500k paths.
    for q in ["main", "libsrc", "mod.rs", "tstfix", "Cargo"] {
        let t = Instant::now();
        let r = fuzzy::find(&pool, &files, q, 50);
        eprintln!("fuzzy {q:?} on {}: {} hits, {} (best {:?})", files.len(), r.len(), ms(t.elapsed()), r.first().map(|m| m.path.as_str()));
    }
    let mut big: Vec<Arc<FileRec>> = Vec::new();
    let mut k = 0;
    while big.len() < 500_000 && !files.is_empty() {
        big.extend(files.iter().map(|f| Arc::new(FileRec::pending(format!("copy{k}/{}", f.path).into(), f.size, f.mtime_ns))));
        k += 1;
    }
    for q in ["main", "srcmodrs", "tstfix", ""] {
        let t = Instant::now();
        let r = fuzzy::find(&pool, &big, q, 50);
        eprintln!("fuzzy {q:?} on {}: {} hits, {}", big.len(), r.len(), ms(t.elapsed()));
    }
    for (q, inc) in [("*.rs", ""), ("src/**/*.{rs,toml}", ""), ("", "*.md, !copy1/**"), ("main", "!copy2/**")] {
        let f = margin_engine::glob::FileFilter::parse(inc, "");
        let t = Instant::now();
        let r = fuzzy::find_files(&pool, &big, q, &f, 100_000);
        eprintln!("find_files {q:?} include {inc:?} on {}: {} hits, {}", big.len(), r.len(), ms(t.elapsed()));
    }

    // Graph: synthetic outlines (margin-code may still return none), 200k files x 10 fns x 10 calls.
    {
        use margin_engine::graph::Graph;
        use margin_engine::index::{CallSum, FnSum};
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        let mut rnd = |m: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % m
        };
        let recs: Vec<Arc<FileRec>> = (0..200_000)
            .map(|i| {
                let fns: Vec<FnSum> = (0..10)
                    .map(|k| FnSum {
                        name: format!("f{}", rnd(500_000)).into(),
                        line: k * 10,
                        col: 3,
                        calls: (0..10).map(|j| CallSum { name: format!("f{}", rnd(500_000)).into(), line: k * 10 + j, col: 4, sink: u8::from(rnd(50) == 0) }).collect(),
                    })
                    .collect();
                Arc::new(FileRec { path: format!("g/{i}.c").into(), size: 1, mtime_ns: 1, flags: 0, bloom: Box::default(), fns: fns.into() })
            })
            .collect();
        let t = Instant::now();
        let g = Graph::build(&recs);
        eprintln!("graph build: {} defs, size {}, {}", g.symbols, g.size, ms(t.elapsed()));
        let names: Vec<String> = (0..1000).map(|i| format!("f{i}")).collect();
        let t = Instant::now();
        for n in &names {
            let _ = g.lens(n, &names[..10]);
        }
        eprintln!("lens: 1000 rows (10 callees each), cold cache: {}", ms(t.elapsed()));
        let t = Instant::now();
        for n in &names {
            let _ = g.lens(n, &names[..10]);
        }
        eprintln!("lens: 1000 rows, warm cache: {}", ms(t.elapsed()));
        let t = Instant::now();
        let r = g.reach_sites("f1", 100_000);
        eprintln!("reach sites from f1: {} sites, {}", r.len(), ms(t.elapsed()));
    }

    // Search (files warm after indexing).
    let cancel = AtomicBool::new(false);
    let queries = [
        SearchQuery { pattern: "fn ".into(), max_hits: 100_000, ..Default::default() },
        SearchQuery { pattern: "unwrap".into(), case_sensitive: true, max_hits: 100_000, ..Default::default() },
        SearchQuery { pattern: "xyzzy_never_there".into(), max_hits: 1000, ..Default::default() },
        SearchQuery { pattern: "SAFETY".into(), whole_word: true, max_hits: 1000, ..Default::default() },
        SearchQuery { pattern: "rare_marker_token".into(), max_hits: 100_000, ..Default::default() },
        SearchQuery { pattern: "self self self self".into(), max_hits: 100_000, ..Default::default() },
        SearchQuery { pattern: r"fn\s+\w+_test\w*".into(), regex: true, max_hits: 100_000, ..Default::default() },
        SearchQuery { pattern: r"(?i)todo|fixme".into(), regex: true, max_hits: 100_000, ..Default::default() },
    ];
    for q in &queries {
        let plan = search::plan(q).expect("plan");
        let n = Mutex::new(0usize);
        let emit = |v: Vec<Site>| *n.lock().unwrap() += v.len();
        for _ in 0..2 {
            let t = Instant::now();
            let st = search::run(&pool, &root, &files, &plan, opts.max_file_bytes, &cancel, &emit).expect("search");
            eprintln!(
                "search {:?}{}: {} hits in {} files, scanned {}, truncated {}, {}",
                q.pattern,
                if q.regex { " (regex)" } else { "" },
                st.hits,
                st.files_matched,
                st.files_scanned,
                st.truncated,
                ms(t.elapsed())
            );
        }
    }
}
