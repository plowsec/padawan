//! The real binary under the real sandbox profiles. macOS only (Seatbelt).

#[cfg(target_os = "macos")]
#[test]
fn both_sandboxes_refuse_every_escape() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_margin")).arg("--check-sandbox").output().expect("run margin");
    let text = String::from_utf8_lossy(&out.stdout);
    eprintln!("{text}{}", String::from_utf8_lossy(&out.stderr));
    if text.is_empty() && String::from_utf8_lossy(&out.stderr).contains("Operation not permitted") {
        // Already inside a sandbox (CI, an agent's shell): Seatbelt cannot nest. Nothing to check here.
        eprintln!("skip: cannot apply a sandbox from inside a sandbox");
        return;
    }
    assert!(out.status.success(), "sandbox check failed");
    assert!(text.contains("all checks passed"));
    assert_eq!(text.matches("  ok  ").count(), 13, "6 checks for the worker (no HOME in its environment), 7 for the gui");
    assert!(!text.contains("FAIL"));
}
