//! Jail: escapes, special files, atomic writes, deletes. Files only through `TempDir`.

use std::fs;
use std::time::Duration;

use margin_engine::jail::Jail;
use margin_proto::{EntryKind, ErrCode, RelPath};
use margin_testutil::TempDir;

fn p(s: &str) -> RelPath {
    RelPath::new(s).unwrap()
}

fn code<T: std::fmt::Debug>(r: Result<T, margin_engine::jail::JailErr>) -> ErrCode {
    r.unwrap_err().code
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    v.sort();
    v
}

#[cfg(unix)]
#[test]
fn symlinks_out_of_the_folder_are_denied() {
    use std::os::unix::fs::symlink;
    let outside = TempDir::new("outside");
    let secret = outside.write("secret.txt", "keep me");
    outside.write("d/inner.txt", "inner");
    let t = TempDir::new("jail");
    symlink(&secret, t.join("lf")).unwrap();
    symlink(outside.join("d"), t.join("ld")).unwrap();
    let j = Jail::new(t.path()).unwrap();

    assert_eq!(code(j.read_file(&p("lf"))), ErrCode::Denied);
    assert_eq!(code(j.write_file(&p("lf"), b"x", None, false)), ErrCode::Denied);
    assert_eq!(code(j.list_dir(&p("ld"))), ErrCode::Denied);
    assert_eq!(code(j.read_file(&p("ld/inner.txt"))), ErrCode::Denied);
    assert_eq!(code(j.write_file(&p("ld/new.txt"), b"x", None, false)), ErrCode::Denied);
    assert_eq!(code(j.create_dir(&p("ld/nd"))), ErrCode::Denied);
    assert_eq!(code(j.rename(&p("lf"), &p("lf2"))), ErrCode::Denied);
    assert_eq!(code(j.rename(&p("ld/inner.txt"), &p("x"))), ErrCode::Denied);
    assert_eq!(code(j.delete(&p("lf"))), ErrCode::Denied);
    assert_eq!(code(j.delete(&p("ld"))), ErrCode::Denied);
    assert_eq!(code(j.delete(&p("ld/inner.txt"))), ErrCode::Denied);
    assert_eq!(code(j.stat(&p("lf"))), ErrCode::Denied);

    assert_eq!(fs::read_to_string(&secret).unwrap(), "keep me");
    assert_eq!(names(outside.path()), vec!["d", "secret.txt"]);
    assert_eq!(names(&outside.join("d")), vec!["inner.txt"]);
    // Listed, not resolved.
    let (ents, _, _) = j.list_dir(&RelPath::root()).unwrap();
    assert!(ents.iter().all(|e| e.kind == EntryKind::Symlink));
}

#[cfg(unix)]
#[test]
fn symlinks_inside_chains_and_loops() {
    use std::os::unix::fs::symlink;
    let t = TempDir::new("chain");
    t.write("real.txt", "hello");
    t.mkdir("dir");
    symlink("real.txt", t.join("a")).unwrap();
    symlink("a", t.join("b")).unwrap();
    symlink("dir", t.join("dl")).unwrap();
    symlink("loop2", t.join("loop1")).unwrap();
    symlink("loop1", t.join("loop2")).unwrap();
    let j = Jail::new(t.path()).unwrap();
    assert_eq!(j.read_file(&p("b")).unwrap().0, b"hello");
    // Write through a link: the target changes, the link stays.
    j.write_file(&p("b"), b"new", None, false).unwrap();
    assert_eq!(fs::read_to_string(t.join("real.txt")).unwrap(), "new");
    assert!(fs::symlink_metadata(t.join("b")).unwrap().file_type().is_symlink());
    // Directory link inside: never listed through.
    assert_eq!(code(j.list_dir(&p("dl"))), ErrCode::Denied);
    assert!(j.read_file(&p("loop1")).is_err());
    assert!(j.write_file(&p("loop1"), b"x", None, false).is_err());
    assert!(j.delete(&p("loop1")).is_err());
    // A link inside the folder may be deleted (the link only).
    j.delete(&p("a")).unwrap();
    assert!(t.join("real.txt").is_file());
}

#[cfg(unix)]
fn mkfifo(path: &std::path::Path) -> bool {
    std::process::Command::new("mkfifo").arg(path).status().is_ok_and(|s| s.success())
}

#[cfg(unix)]
#[test]
fn fifo_never_blocks() {
    let t = TempDir::new("fifo");
    if !mkfifo(&t.join("pipe")) {
        return;
    }
    let root = t.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let j = Jail::new(&root).unwrap();
        let r = j.read_file(&p("pipe")).map(|_| ());
        let w = j.write_file(&p("pipe"), b"x", None, false).map(|_| ());
        let (ents, _, _) = j.list_dir(&RelPath::root()).unwrap();
        let _ = tx.send((r, w, ents));
    });
    let (r, w, ents) = rx.recv_timeout(Duration::from_secs(5)).expect("FIFO blocked the jail");
    assert_eq!(r.unwrap_err().code, ErrCode::Denied);
    assert_eq!(w.unwrap_err().code, ErrCode::Denied);
    assert_eq!(ents[0].kind, EntryKind::Other);
    // Walker skips it without blocking too.
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let seen = std::sync::atomic::AtomicU32::new(0);
    let w = margin_engine::walk::walk(&pool, &fs::canonicalize(t.path()).unwrap(), &Default::default(), &stop, &seen);
    assert!(w.files.is_empty());
    assert_eq!(w.skipped, 1);
}

#[test]
fn write_conflict_exists_atomic_and_mode() {
    let t = TempDir::new("write");
    t.write("a.txt", "one");
    let j = Jail::new(t.path()).unwrap();
    let m = j.stat(&p("a.txt")).unwrap();
    assert_eq!(code(j.write_file(&p("a.txt"), b"x", Some(m.mtime_ns.wrapping_add(1)), false)), ErrCode::Conflict);
    assert_eq!(code(j.write_file(&p("a.txt"), b"x", None, true)), ErrCode::Exists);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(t.join("a.txt"), fs::Permissions::from_mode(0o640)).unwrap();
    }
    let m2 = j.write_file(&p("a.txt"), b"two", Some(m.mtime_ns), false).unwrap();
    assert_eq!(m2.size, 3);
    assert_eq!(fs::read_to_string(t.join("a.txt")).unwrap(), "two");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(t.join("a.txt")).unwrap().permissions().mode() & 0o777, 0o640);
    }
    j.write_file(&p("new file é.txt"), b"", None, true).unwrap();
    assert_eq!(names(t.path()), vec!["a.txt", "new file é.txt"], "no temp files left");
    // Missing parent, directory target, bad names.
    assert_eq!(code(j.write_file(&p("nope/x"), b"", None, false)), ErrCode::NotFound);
    t.mkdir("d");
    assert_eq!(code(j.write_file(&p("d"), b"", None, false)), ErrCode::Denied);
    assert_eq!(code(j.write_file(&RelPath::root(), b"", None, false)), ErrCode::Invalid);
    assert_eq!(code(j.write_file(&p("gone"), b"", Some(1), false)), ErrCode::Conflict);
    assert_eq!(names(t.path()), vec!["a.txt", "d", "new file é.txt"]);
}

#[test]
fn delete_rename_create() {
    let t = TempDir::new("del");
    t.write("d/f.txt", "x");
    t.mkdir("empty");
    let j = Jail::new(t.path()).unwrap();
    assert_eq!(code(j.delete(&p("d"))), ErrCode::Invalid);
    assert!(t.join("d/f.txt").is_file());
    assert_eq!(j.delete(&p("empty")).unwrap(), EntryKind::Dir);
    assert_eq!(code(j.delete(&RelPath::root())), ErrCode::Invalid);
    assert_eq!(code(j.delete(&p("missing"))), ErrCode::NotFound);
    j.create_dir(&p("n")).unwrap();
    assert_eq!(code(j.create_dir(&p("n"))), ErrCode::Exists);
    assert_eq!(code(j.create_dir(&p("x/y"))), ErrCode::NotFound);
    t.write("g.txt", "g");
    assert_eq!(code(j.rename(&p("g.txt"), &p("d/f.txt"))), ErrCode::Exists);
    j.rename(&p("g.txt"), &p("n/g2.txt")).unwrap();
    assert_eq!(fs::read_to_string(t.join("n/g2.txt")).unwrap(), "g");
    j.delete(&p("d/f.txt")).unwrap();
    j.delete(&p("d")).unwrap();
    assert_eq!(names(t.path()), vec!["n"]);
}

#[test]
fn sizes_deep_paths_and_names() {
    let t = TempDir::new("sizes");
    t.write("zero", "");
    t.write("ten", "0123456789");
    t.write("eleven", "0123456789A");
    t.write("bin", b"ab\0cd");
    let deep: Vec<String> = (0..60).map(|i| format!("d{i}")).collect();
    let deep = deep.join("/");
    t.write(&format!("{deep}/f.txt"), "deep");
    t.write("sp ace/ünï cødé.txt", "u");
    let mut j = Jail::new(t.path()).unwrap();
    j.max_open = 10;
    let (d, m, bin) = j.read_file(&p("zero")).unwrap();
    assert!(d.is_empty() && m.size == 0 && !bin && m.kind == EntryKind::File);
    assert_eq!(j.read_file(&p("ten")).unwrap().0.len(), 10);
    assert_eq!(code(j.read_file(&p("eleven"))), ErrCode::TooLarge);
    let (d, _, bin) = j.read_file(&p("bin")).unwrap();
    assert!(bin && d.is_empty());
    assert_eq!(j.read_file(&p(&format!("{deep}/f.txt"))).unwrap().0, b"deep");
    assert_eq!(j.read_file(&p("sp ace/ünï cødé.txt")).unwrap().0, b"u");
    assert_eq!(code(j.read_file(&p("sp ace"))), ErrCode::Denied);
    assert_eq!(code(j.list_dir(&p("ten"))), ErrCode::Invalid);
}

#[cfg(unix)]
#[test]
fn bad_names_are_skipped() {
    let t = TempDir::new("names");
    t.write("ok.txt", "x");
    t.write("bad\\name", "x");
    t.write("ctl\u{1}x", "x");
    let j = Jail::new(t.path()).unwrap();
    let (ents, trunc, skipped) = j.list_dir(&RelPath::root()).unwrap();
    assert_eq!(ents.len(), 1);
    assert!(!trunc);
    assert_eq!(skipped, 2);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        fs::write(t.path().join(std::ffi::OsStr::from_bytes(b"\xff\xfe")), "x").unwrap();
        assert_eq!(j.list_dir(&RelPath::root()).unwrap().2, 3);
    }
}
