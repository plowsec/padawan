//! `--role gui`: the window process. stdin/stdout are the frame pipes to the launcher; `MuxOpener` turns
//! them into engines (`Client`) and folder listings for the GUI. Nothing here blocks the UI thread:
//! one thread writes frames, one reads them.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use margin_proto::frame::{read_frame, write_frame};
use margin_proto::mux::{FromLauncher, ToLauncher, MAX_MUX_FRAME};
use margin_proto::{decode_out, encode_request, Client, DirListing, Event, IndexOpts, Opener, Out, ReqId, Request, Waker};

use crate::args::Args;

pub fn run_gui(args: &Args) -> Result<(), String> {
    let opener = MuxOpener::start(std::io::stdin(), std::io::stdout(), args.sandbox.clone());
    margin_gui::run(margin_gui::RunOpts {
        opener: Box::new(opener),
        folder: args.folder.clone(),
        file: args.file.clone(),
        config_dir: args.dir.clone(),
        gui_sandbox: args.sandbox.clone(),
    })
}

/// Where engine messages of the current generation go.
struct Route {
    gen: u32,
    out: Option<Sender<Out>>,
    waker: Option<Waker>,
    browse_waker: Option<Waker>,
}

pub struct MuxOpener {
    to_launcher: Sender<ToLauncher>,
    route: Arc<Mutex<Route>>,
    browse_rx: Receiver<DirListing>,
    sandbox: String,
    gen: u32,
}

impl MuxOpener {
    pub fn start(mut input: impl std::io::Read + Send + 'static, mut output: impl std::io::Write + Send + 'static, sandbox: String) -> Self {
        let (to_launcher, outgoing) = channel::<ToLauncher>();
        let (browse_tx, browse_rx) = channel();
        let route = Arc::new(Mutex::new(Route { gen: 0, out: None, waker: None, browse_waker: None }));

        std::thread::Builder::new()
            .name("mux-write".into())
            .spawn(move || {
                for msg in outgoing {
                    if write_frame(&mut output, &msg.encode()).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn thread");

        let r = route.clone();
        std::thread::Builder::new()
            .name("mux-read".into())
            .spawn(move || {
                let deliver = |gen: u32, out: Out| {
                    let Ok(route) = r.lock() else { return };
                    if route.gen != gen {
                        return;
                    }
                    if let Some(tx) = &route.out {
                        let _ = tx.send(out);
                    }
                    if let Some(w) = &route.waker {
                        w();
                    }
                };
                let fatal = |msg: String| {
                    let gen = r.lock().map(|g| g.gen).unwrap_or(0);
                    deliver(gen, Out::Event(Event::Fatal { msg }));
                };
                loop {
                    let msg = match read_frame(&mut input, MAX_MUX_FRAME) {
                        Ok(Some(buf)) => FromLauncher::decode(&buf),
                        Ok(None) => return fatal("the launcher is gone".into()),
                        Err(e) => return fatal(format!("launcher link: {e}")),
                    };
                    match msg {
                        Ok(FromLauncher::Engine { gen, payload }) => match decode_out(&payload) {
                            Ok(out) => deliver(gen, out),
                            // The worker reads hostile files: a bad message means it is broken or subverted. Drop it.
                            Err(e) => deliver(gen, Out::Event(Event::Fatal { msg: format!("bad message from the worker: {e}") })),
                        },
                        Ok(FromLauncher::Ended { gen, msg }) => deliver(gen, Out::Event(Event::Fatal { msg })),
                        Ok(FromLauncher::Dirs(listing)) => {
                            let _ = browse_tx.send(listing);
                            if let Some(w) = r.lock().ok().and_then(|g| g.browse_waker.clone()) {
                                w();
                            }
                        }
                        Err(e) => return fatal(format!("bad message from the launcher: {e}")),
                    }
                }
            })
            .expect("spawn thread");

        Self { to_launcher, route, browse_rx, sandbox, gen: 0 }
    }
}

impl Opener for MuxOpener {
    fn open(&mut self, root: &Path, opts: &IndexOpts, waker: Waker) -> Client {
        self.gen = self.gen.wrapping_add(1);
        let gen = self.gen;
        let (req_tx, req_rx) = channel::<(ReqId, Request)>();
        let (out_tx, out_rx) = channel();
        if let Ok(mut route) = self.route.lock() {
            route.gen = gen;
            route.out = Some(out_tx);
            route.waker = Some(waker);
        }
        let _ = self.to_launcher.send(ToLauncher::Spawn { gen, root: root.to_string_lossy().into_owned(), opts: opts.clone() });
        let to_launcher = self.to_launcher.clone();
        std::thread::Builder::new()
            .name(format!("mux-req-{gen}"))
            .spawn(move || {
                for (id, req) in req_rx {
                    if to_launcher.send(ToLauncher::Engine { gen, payload: encode_request(id, &req) }).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn thread");
        Client::from_channels(req_tx, out_rx)
    }

    fn sandbox(&self) -> &str {
        &self.sandbox
    }

    fn browse(&mut self, path: Option<&Path>, waker: Waker) {
        if let Ok(mut route) = self.route.lock() {
            route.browse_waker = Some(waker);
        }
        let path = path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = self.to_launcher.send(ToLauncher::Browse { path });
    }

    fn poll_browse(&mut self) -> Option<DirListing> {
        self.browse_rx.try_recv().ok()
    }

    fn new_window(&mut self, folder: Option<&Path>) {
        let folder = folder.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = self.to_launcher.send(ToLauncher::NewWindow { folder });
    }
}

impl Drop for MuxOpener {
    fn drop(&mut self) {
        let _ = self.to_launcher.send(ToLauncher::Quit);
    }
}
