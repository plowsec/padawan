//! The engine: every access to the open folder happens here. Runs in the sandboxed worker process
//! (or on threads of the GUI process with `--no-sandbox` and in tests).
//!
//! STUB. PUBLIC API IS FROZEN: `EngineConfig`, `run`, `spawn_local`.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use margin_proto::{Client, IndexOpts, Outbox, ReqId, Request, Waker};

#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// The folder. Canonicalised by the engine at start; everything is confined to it.
    pub root: PathBuf,
    /// Where the index cache lives (outside the folder). `None`: no cache on disk.
    pub cache_dir: Option<PathBuf>,
    pub opts: IndexOpts,
    /// Worker threads; 0 = one per core.
    pub threads: usize,
}

/// Serve requests until `Shutdown` or until `requests` closes. Sends `Event::Hello` first, then indexes
/// in the background (`Event::Index`). Blocks the calling thread; work runs on its own pool.
pub fn run(cfg: EngineConfig, requests: Receiver<(ReqId, Request)>, out: Outbox) {
    let _ = (cfg, requests, out);
}

/// Engine on threads of this process. Returns at once.
pub fn spawn_local(cfg: EngineConfig, waker: Waker) -> Client {
    let (client, rx, out) = margin_proto::link::local_pair(waker);
    std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || run(cfg, rx, out))
        .expect("spawn thread");
    client
}
