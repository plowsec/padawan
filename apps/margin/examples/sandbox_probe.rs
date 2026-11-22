//! Does an egui/wgpu window come up under a sandbox profile? Renders some frames, reads a system font,
//! tries things the sandbox must refuse, reports on stderr, exits.
//!   sandbox-exec -p "$PROFILE" target/debug/examples/sandbox_probe

use std::io::Write as _;

struct Probe {
    frames: u32,
}

impl eframe::App for Probe {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        self.frames += 1;
        ui.label(format!("frame {}", self.frames));
        ui.ctx().request_repaint();
        if self.frames == 30 {
            eprintln!("probe: 30 frames rendered");
            ui.ctx().send_viewport_cmd(eframe::egui::ViewportCommand::Close);
        }
    }
}

fn check(name: &str, refused: bool) {
    eprintln!("probe: {name}: {}", if refused { "refused (good)" } else { "ALLOWED" });
}

fn main() {
    let home = std::env::var("HOME").unwrap_or_default();
    check("read ~/.zshrc", std::fs::read(format!("{home}/.zshrc")).is_err() && std::fs::read(format!("{home}/.zprofile")).is_err() && std::fs::read_dir(&home).is_err());
    let tmp = format!("{home}/margin-probe.tmp");
    let refused = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).is_err();
    if !refused {
        let _ = std::fs::remove_file(&tmp);
    }
    check("write ~/margin-probe.tmp", refused);
    check("exec /bin/echo", std::process::Command::new("/bin/echo").arg("x").output().is_err());
    check("tcp connect 1.1.1.1:443", std::net::TcpStream::connect_timeout(&"1.1.1.1:443".parse().unwrap(), std::time::Duration::from_secs(2)).is_err());
    check("udp bind+send", std::net::UdpSocket::bind("0.0.0.0:0").and_then(|s| s.send_to(b"x", "1.1.1.1:53")).is_err());
    eprintln!("probe: font SFNS readable: {}", std::fs::read("/System/Library/Fonts/SFNS.ttf").is_ok());
    if let Ok(dir) = std::env::var("PROBE_WRITE_OK") {
        let p = format!("{dir}/probe-write.tmp");
        let ok = std::fs::File::create(&p).and_then(|mut f| f.write_all(b"x")).is_ok();
        eprintln!("probe: write in allowed dir: {ok}");
        let _ = std::fs::remove_file(&p);
    }
    if std::env::var("PROBE_NO_WINDOW").is_ok() {
        return;
    }
    let r = eframe::run_native("probe", eframe::NativeOptions::default(), Box::new(|_cc| Ok(Box::new(Probe { frames: 0 }))));
    eprintln!("probe: eframe result: {r:?}");
}
