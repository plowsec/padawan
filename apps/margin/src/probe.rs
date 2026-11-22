//! `--role probe`: child of `--check-sandbox`. Runs inside one of the two sandbox profiles and tries
//! what that profile must refuse. One line per attempt on stdout: `check<TAB>name<TAB>pass|FAIL<TAB>detail`.

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use crate::args::Args;

pub fn run(args: &Args) -> Result<(), String> {
    let allowed = args.folder.clone().ok_or("probe: --root is required")?;
    let outside = args.outside.clone().ok_or("probe: --outside is required")?;
    let mut out = std::io::stdout().lock();
    let mut report = |name: &str, want_refused: bool, refused: bool, detail: String| {
        let verdict = if want_refused == refused { "pass" } else { "FAIL" };
        let _ = writeln!(out, "check\t{name}\t{verdict}\t{detail}");
    };
    let outcome = |r: &std::io::Result<()>| match r {
        Ok(()) => "allowed".to_owned(),
        Err(e) => format!("refused ({e})"),
    };

    let r = std::fs::read(outside.join("canary.txt")).map(drop);
    report("read a file outside", true, r.is_err(), outcome(&r));

    let r = std::fs::read_dir(&outside).map(drop);
    report("list a directory outside", true, r.is_err(), outcome(&r));

    let escape = outside.join("escape.txt");
    let r = std::fs::OpenOptions::new().write(true).create_new(true).open(&escape).map(drop);
    if r.is_ok() {
        let _ = std::fs::remove_file(&escape);
    }
    report("create a file outside", true, r.is_err(), outcome(&r));

    if let Some(home) = crate::dirs::home() {
        let r = std::fs::read_dir(&home).map(drop);
        report("list the home directory", true, r.is_err(), outcome(&r));
    }

    let r = std::process::Command::new("/bin/echo").arg("escaped").stdout(std::process::Stdio::null()).status().map(drop);
    report("start a program", true, r.is_err(), outcome(&r));

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], args.port));
    let r = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map(drop);
    report("open a network connection", true, r.is_err(), outcome(&r));

    let inside = allowed.join("probe.txt");
    let r = write_then_remove(&inside);
    report("write inside the allowed directory", false, r.is_err(), outcome(&r));
    Ok(())
}

fn write_then_remove(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)?.write_all(b"probe")?;
    std::fs::remove_file(path)
}
