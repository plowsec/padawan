//! Throughput checks with generous bounds (debug builds). Prints MB/s.

use margin_code::{lex_line, outline, Lang, LineState};
use std::time::Instant;

fn synthetic_c(bytes: usize) -> String {
    let mut s = String::with_capacity(bytes + 512);
    let mut i = 0u32;
    while s.len() < bytes {
        s.push_str(&format!(
            "/* helper {i} */\nstatic int fn_{i}(const char *in, size_t n)\n{{\n    char buf[64];\n    if (n > sizeof(buf)) {{ return -1; }}\n    memcpy(buf, in, n); // copy\n    log_msg(\"fn_{i}: %s\\n\", buf);\n    return fn_{}(buf, strlen(buf)) + obj->vt->call(n);\n}}\n\n",
            i.wrapping_sub(1)
        ));
        i += 1;
    }
    s
}

fn synthetic_py(bytes: usize) -> String {
    let mut s = String::with_capacity(bytes + 512);
    let mut i = 0u32;
    while s.len() < bytes {
        s.push_str(&format!(
            "@decorator\ndef fn_{i}(a, b=None):\n    \"\"\"Doc {i}.\"\"\"\n    x = helper(a, 'str')  # c\n    return os.path.join(x, str(b))\n\n"
        ));
        i += 1;
    }
    s
}

#[test]
fn outline_c_2mb() {
    let src = synthetic_c(2 * 1024 * 1024);
    let t = Instant::now();
    let o = outline(Lang::C, &src);
    let s = t.elapsed().as_secs_f64();
    let mbs = src.len() as f64 / 1e6 / s;
    eprintln!("outline C: {:.1} MB in {s:.3}s = {mbs:.1} MB/s, {} fns", src.len() as f64 / 1e6, o.fns.len());
    assert!(o.fns.len() > 5000);
    assert!(o.fns.iter().all(|f| f.calls.len() == 5), "{:?}", o.fns.first());
    assert!(s < 2.0, "outline of 2 MB took {s:.2}s");
}

#[test]
fn outline_python_2mb() {
    let src = synthetic_py(2 * 1024 * 1024);
    let t = Instant::now();
    let o = outline(Lang::Python, &src);
    let s = t.elapsed().as_secs_f64();
    eprintln!("outline Python: {:.1} MB/s, {} fns", src.len() as f64 / 1e6 / s, o.fns.len());
    assert!(s < 2.0, "outline of 2 MB took {s:.2}s");
}

#[test]
fn lex_c_2mb() {
    let src = synthetic_c(2 * 1024 * 1024);
    let mut buf = Vec::new();
    let t = Instant::now();
    let mut st = LineState::default();
    let mut n = 0usize;
    for line in src.split('\n') {
        buf.clear();
        st = lex_line(Lang::C, st, line, &mut buf);
        n += buf.len();
    }
    let s = t.elapsed().as_secs_f64();
    eprintln!("lex C: {:.1} MB/s, {n} tokens", src.len() as f64 / 1e6 / s);
    assert!(s < 2.0, "lexing 2 MB took {s:.2}s");
}
