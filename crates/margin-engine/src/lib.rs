//! The engine: every access to the open folder happens here. Runs in the sandboxed worker process
//! (or on threads of the GUI process with `--no-sandbox` and in tests).
//!
//! PUBLIC API IS FROZEN: `EngineConfig`, `run`, `spawn_local`. The modules are public for the bench
//! and tests; the GUI uses only the frozen API.
//!
//! | Module | Does |
//! |---|---|
//! | `jail` | path resolution + file I/O confined to the canonical root |
//! | `glob`, `ignore` | gitignore globs, `.gitignore` stack |
//! | `walk` | parallel walk → file table |
//! | `index`, `bloom` | per-file record: flags, trigram bloom, outline summary |
//! | `fuzzy` | `FindFiles` |
//! | `search`, `snippet` | content search, `Site` windows |
//! | `graph` | call graph: lens, callers, defs, reach, sink paths |
//! | `paths` | `Paths`: call chains + argument flow |
//! | `replace` | `Replace` |
//! | `cache` | on-disk index cache |
//! | `service` | request loop, pools, events, incremental updates |

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use margin_proto::{Client, IndexOpts, Outbox, ReqId, Request, Waker};

pub mod bloom;
pub mod cache;
pub mod fuzzy;
pub mod glob;
pub mod graph;
pub mod ignore;
pub mod index;
pub mod jail;
pub mod paths;
pub mod replace;
pub mod search;
pub mod service;
pub mod snippet;
pub mod walk;

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
    service::run(cfg, requests, out);
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
