//! `--no-sandbox`: engine on threads of the GUI process. Same root policy as the launcher.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use margin_engine::EngineConfig;
use margin_proto::{Client, DirListing, Event, IndexOpts, Opener, Out, SavedFile, Waker};

use crate::save::Saver;

pub struct LocalOpener {
    cache: Option<PathBuf>,
    config: Option<PathBuf>,
    browse_tx: Sender<DirListing>,
    browse_rx: Receiver<DirListing>,
    saver: Arc<Mutex<Saver>>,
    saved_tx: Sender<SavedFile>,
    saved_rx: Receiver<SavedFile>,
}

impl LocalOpener {
    pub fn new(cache: Option<PathBuf>, config: Option<PathBuf>) -> Self {
        let (browse_tx, browse_rx) = channel();
        let (saved_tx, saved_rx) = channel();
        let saver = Arc::new(Mutex::new(Saver::new(crate::dirs::home(), cache.iter().chain(config.iter()).cloned().collect())));
        Self { cache, config, browse_tx, browse_rx, saver, saved_tx, saved_rx }
    }
}

/// A client whose only message is `Fatal { msg }`.
pub fn failed_client(msg: String, waker: &Waker) -> Client {
    let (req_tx, _req_rx) = channel();
    let (out_tx, out_rx) = channel();
    let _ = out_tx.send(Out::Event(Event::Fatal { msg }));
    waker();
    Client::from_channels(req_tx, out_rx)
}

impl Opener for LocalOpener {
    fn open(&mut self, root: &Path, opts: &IndexOpts, waker: Waker) -> Client {
        let home = crate::dirs::home();
        let own: Vec<&Path> = self.cache.iter().chain(self.config.iter()).map(PathBuf::as_path).collect();
        let root = match crate::sandbox::check_root(root, home.as_deref(), &own) {
            Ok(r) => r,
            Err(e) => return failed_client(e, &waker),
        };
        let cache_dir = self.cache.as_ref().map(|c| crate::dirs::root_cache_dir(c, &root));
        margin_engine::spawn_local(EngineConfig { root, cache_dir, opts: opts.clone(), threads: 0 }, waker)
    }

    fn sandbox(&self) -> &str {
        "none"
    }

    fn browse(&mut self, path: Option<&Path>, waker: Waker) {
        let (tx, path) = (self.browse_tx.clone(), path.map(Path::to_path_buf));
        let _ = std::thread::Builder::new().name("browse".into()).spawn(move || {
            let _ = tx.send(crate::browse::list(path.as_deref()));
            waker();
        });
    }

    fn poll_browse(&mut self) -> Option<DirListing> {
        self.browse_rx.try_recv().ok()
    }

    fn save(&mut self, id: u32, path: &str, data: Vec<u8>, replace: bool, waker: Waker) {
        let (tx, saver, path) = (self.saved_tx.clone(), self.saver.clone(), path.to_owned());
        let _ = std::thread::Builder::new().name("save".into()).spawn(move || {
            let _ = tx.send(crate::save::answer(&saver, id, &path, &data, replace));
            waker();
        });
    }

    fn poll_saved(&mut self) -> Option<SavedFile> {
        self.saved_rx.try_recv().ok()
    }

    fn new_window(&mut self, folder: Option<&Path>) {
        if let Err(e) = crate::window::spawn(folder, true) {
            eprintln!("margin: {e}");
        }
    }
}
