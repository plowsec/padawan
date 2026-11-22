//! The open folder: engine client, requests in flight (by id), index status.
//!
//! Every request goes through `Workspace::send` with a `Pending` tag; replies are matched by id.
//! Unknown ids (cancelled, stale) are dropped.

use std::collections::HashMap;
use std::path::PathBuf;

use margin_proto::{Client, IndexPhase, IndexStatus, Out, ReqId, RelPath, Request};

/// What a request was for.
#[derive(Clone, Debug)]
pub enum Pending {
    ListDir(RelPath),
    /// Open (or reload) the tab `tab`.
    Read { tab: u64, reload: bool },
    Write { tab: u64, rev: u64 },
    Stat { tab: u64 },
    FindFiles,
    Search,
    Lens { tab: u64, rev: u64 },
    Callers(String),
    Defs(String),
    Reach(String),
    /// New file to open once created.
    Create(RelPath),
    /// Rename / delete / mkdir: refresh this directory after.
    FileOp(RelPath),
    Rename { from: RelPath, to: RelPath },
    Deleted(RelPath),
    Ignore,
}

pub struct Workspace {
    pub client: Client,
    /// The path we asked to open (for state, recent list, reopen).
    pub path: PathBuf,
    /// From `Event::Hello` (sanitised at display time).
    pub root: Option<String>,
    pub name: Option<String>,
    pub index: Option<IndexStatus>,
    pub fatal: Option<String>,
    pub pending: HashMap<ReqId, Pending>,
}

impl Workspace {
    pub fn new(client: Client, path: PathBuf) -> Workspace {
        Workspace { client, path, root: None, name: None, index: None, fatal: None, pending: HashMap::new() }
    }

    pub fn send(&mut self, req: Request, p: Pending) -> ReqId {
        let id = self.client.request(req);
        self.pending.insert(id, p);
        id
    }

    pub fn cancel(&mut self, id: ReqId) {
        if self.pending.remove(&id).is_some() {
            self.client.cancel(id);
        }
    }

    pub fn poll(&self) -> Option<Out> {
        self.client.poll()
    }

    pub fn ready(&self) -> bool {
        self.index.is_some_and(|s| s.phase == IndexPhase::Ready)
    }

    /// Display name: from `Hello`, else the last path component.
    pub fn display_name(&self) -> String {
        match &self.name {
            Some(n) => crate::sanitize::clean(n),
            None => crate::sanitize::clean(&self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        }
    }

    /// Absolute path as a string, for the state file and Copy Path.
    pub fn root_str(&self) -> String {
        self.root.clone().unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }
}
