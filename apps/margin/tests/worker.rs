//! The real worker process inside the real worker sandbox, driven over its pipes like the launcher does.
//! macOS only (Seatbelt).

#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use margin::sandbox::{worker_command, Sandbox, WorkerPaths};
use margin_proto::{Client, ErrCode, Event, IndexPhase, Out, RelPath, Reply, ReqId, Request, SearchQuery};
use margin_testutil::TempDir;

fn can_sandbox() -> bool {
    std::process::Command::new("/usr/bin/sandbox-exec")
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn next(c: &Client) -> Out {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(o) = c.poll() {
            return o;
        }
        assert!(Instant::now() < end, "no message from the worker");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Final reply to `id` (events and other ids are skipped; search batches are collected by the caller).
fn reply(c: &Client, id: ReqId) -> Reply {
    loop {
        match next(c) {
            Out::Reply(got, r) if got == id => return r,
            Out::Event(Event::Fatal { msg }) => panic!("worker died: {msg}"),
            _ => {}
        }
    }
}

fn p(s: &str) -> RelPath {
    RelPath::new(s).unwrap()
}

#[test]
fn worker_serves_a_folder_from_inside_the_sandbox() {
    if !can_sandbox() {
        eprintln!("skip: cannot apply a sandbox here (already sandboxed?)");
        return;
    }
    let outside = TempDir::new("outside");
    let secret = outside.write("secret.txt", "top secret");
    let root = TempDir::new("root");
    root.write("notes/todo.md", "# Todo\n\nbuy needle_in_haystack\n");
    root.write("src/main.c", "int main(void) {\n    return run(1);\n}\n");
    std::os::unix::fs::symlink(&secret, root.path().join("leak.txt")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("leakdir")).unwrap();
    let cache = TempDir::new("cache");

    let exe = Path::new(env!("CARGO_BIN_EXE_margin"));
    let mut cmd = worker_command(Sandbox::Seatbelt, &WorkerPaths { exe, root: root.path(), cache: cache.path() });
    cmd.args(["--role", "worker", "--root"]).arg(root.path()).arg("--cache").arg(cache.path());
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().expect("spawn worker");
    let client = margin_proto::link::client_over_io(child.stdout.take().unwrap(), child.stdin.take().unwrap(), Arc::new(|| {}));

    // Hello, then the index gets ready.
    match next(&client) {
        Out::Event(Event::Hello { protocol, root: shown, .. }) => {
            assert_eq!(protocol, margin_proto::PROTOCOL);
            assert_eq!(Path::new(&shown), root.path());
        }
        other => panic!("expected Hello, got {other:?}"),
    }
    loop {
        match next(&client) {
            Out::Event(Event::Index(s)) if s.phase == IndexPhase::Ready => {
                assert!(s.files_total >= 2, "{s:?}");
                break;
            }
            Out::Event(Event::Fatal { msg }) => panic!("worker died: {msg}"),
            _ => {}
        }
    }

    // Listing and reading inside the folder work.
    let id = client.request(Request::ListDir { path: RelPath::root() });
    match reply(&client, id) {
        Reply::Dir { entries, .. } => {
            let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
            for n in ["notes", "src", "leak.txt", "leakdir"] {
                assert!(names.contains(&n), "{names:?}");
            }
        }
        other => panic!("{other:?}"),
    }
    let id = client.request(Request::ReadFile { path: p("notes/todo.md") });
    let mtime = match reply(&client, id) {
        Reply::File { data, meta, binary } => {
            assert!(!binary);
            assert!(String::from_utf8(data).unwrap().contains("needle_in_haystack"));
            meta.mtime_ns
        }
        other => panic!("{other:?}"),
    };

    // Symlinks that leave the folder: refused, the secret stays unread and unchanged.
    for (req, what) in [
        (Request::ReadFile { path: p("leak.txt") }, "read through a file symlink"),
        (Request::ListDir { path: p("leakdir") }, "list through a directory symlink"),
        (Request::ReadFile { path: p("leakdir/secret.txt") }, "read below a directory symlink"),
        (Request::WriteFile { path: p("leak.txt"), data: b"pwned".to_vec(), expect_mtime_ns: None, create_new: false }, "write through a file symlink"),
        (Request::WriteFile { path: p("leakdir/new.txt"), data: b"pwned".to_vec(), expect_mtime_ns: None, create_new: true }, "create below a directory symlink"),
    ] {
        let id = client.request(req);
        match reply(&client, id) {
            Reply::Err { code: ErrCode::Denied, .. } => {}
            other => panic!("{what}: expected Denied, got {other:?}"),
        }
    }
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "top secret");
    assert!(!outside.path().join("new.txt").exists());

    // Write, then search sees the new text.
    let id = client.request(Request::WriteFile { path: p("notes/todo.md"), data: b"# Todo\n\nfind zebra_crossing\n".to_vec(), expect_mtime_ns: Some(mtime), create_new: false });
    assert!(matches!(reply(&client, id), Reply::Meta(_)));
    assert_eq!(std::fs::read_to_string(root.path().join("notes/todo.md")).unwrap(), "# Todo\n\nfind zebra_crossing\n");

    let id = client.request(Request::Search { query: SearchQuery { pattern: "zebra_crossing".into(), max_hits: 100, ..Default::default() } });
    let mut hits = Vec::new();
    loop {
        match next(&client) {
            Out::Reply(got, Reply::SearchBatch { hits: h }) if got == id => hits.extend(h),
            Out::Reply(got, Reply::SearchDone(stats)) if got == id => {
                assert_eq!(stats.hits, 1);
                break;
            }
            Out::Reply(got, other) if got == id => panic!("{other:?}"),
            _ => {}
        }
    }
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].path.as_str(), hits[0].line), ("notes/todo.md", 2));

    // Closing the pipe ends the worker.
    drop(client);
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < end, "worker did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}
