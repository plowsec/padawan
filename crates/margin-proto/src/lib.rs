//! Contract between the GUI and the engine (worker). No dependency, no unsafe.
//!
//! | Module | What |
//! |---|---|
//! | `path` | `RelPath`: validated path relative to the open folder |
//! | `msg` | `Request`, `Reply`, `Event`, `Out` |
//! | `wire` | binary codec, bounds-checked (both sides treat the peer as hostile) |
//! | `frame` | length-prefixed frames over a byte stream |
//! | `link` | `Client` (GUI side), `Outbox` (engine side), in-process pair, pipe adapters |
//! | `mux` | GUI ⇄ launcher frames (spawn a worker, relay) |
//! | `limits` | size caps |

pub mod frame;
pub mod limits;
pub mod link;
pub mod msg;
pub mod mux;
pub mod path;
pub mod wire;

pub use link::{Client, Opener, Outbox, Waker};
pub use msg::*;
pub use path::RelPath;

/// Bumped on any wire change. Worker and GUI are the same binary; a mismatch means a stale process.
pub const PROTOCOL: u32 = 1;
