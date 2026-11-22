//! GUI ⇄ launcher frames. The launcher is the only process that may start processes: the GUI asks it
//! for a worker on a folder, then engine frames are relayed both ways, tagged with the worker generation
//! (frames of a replaced worker are dropped).

use crate::limits::{MAX_FRAME, MAX_PATH, MAX_TEXT};
use crate::msg::{IndexOpts, Wire};
use crate::wire::{Dec, Enc, WireError, WireResult};

/// Largest mux frame: one engine frame plus the mux header.
pub const MAX_MUX_FRAME: usize = MAX_FRAME + 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToLauncher {
    /// Start a worker on `root` (absolute path) as generation `gen`; the previous worker is stopped.
    Spawn { gen: u32, root: String, opts: IndexOpts },
    /// An encoded `(ReqId, Request)` for the worker of `gen`.
    Engine { gen: u32, payload: Vec<u8> },
    /// Stop everything.
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FromLauncher {
    /// An encoded `Out` from the worker of `gen`.
    Engine { gen: u32, payload: Vec<u8> },
    /// The worker of `gen` could not start, or ended. `msg` is for the user.
    Ended { gen: u32, msg: String },
}

impl ToLauncher {
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Enc::new();
        match self {
            ToLauncher::Spawn { gen, root, opts } => {
                e.u8(1);
                e.u32(*gen);
                e.str(root);
                opts.enc(&mut e);
            }
            ToLauncher::Engine { gen, payload } => {
                e.u8(2);
                e.u32(*gen);
                e.bytes(payload);
            }
            ToLauncher::Quit => e.u8(3),
        }
        e.finish()
    }

    pub fn decode(buf: &[u8]) -> WireResult<Self> {
        let mut d = Dec::new(buf);
        let v = match d.u8()? {
            1 => ToLauncher::Spawn { gen: d.u32()?, root: d.string(MAX_PATH)?, opts: IndexOpts::dec(&mut d)? },
            2 => ToLauncher::Engine { gen: d.u32()?, payload: d.bytes(MAX_FRAME)?.to_vec() },
            3 => ToLauncher::Quit,
            t => return Err(WireError::BadTag(t)),
        };
        d.finish()?;
        Ok(v)
    }
}

impl FromLauncher {
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Enc::new();
        match self {
            FromLauncher::Engine { gen, payload } => {
                e.u8(1);
                e.u32(*gen);
                e.bytes(payload);
            }
            FromLauncher::Ended { gen, msg } => {
                e.u8(2);
                e.u32(*gen);
                e.str(msg);
            }
        }
        e.finish()
    }

    pub fn decode(buf: &[u8]) -> WireResult<Self> {
        let mut d = Dec::new(buf);
        let v = match d.u8()? {
            1 => FromLauncher::Engine { gen: d.u32()?, payload: d.bytes(MAX_FRAME)?.to_vec() },
            2 => FromLauncher::Ended { gen: d.u32()?, msg: d.string(MAX_TEXT)? },
            t => return Err(WireError::BadTag(t)),
        };
        d.finish()?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for m in [
            ToLauncher::Spawn { gen: 2, root: "/tmp/x".into(), opts: IndexOpts::default() },
            ToLauncher::Engine { gen: 2, payload: vec![1, 2, 3] },
            ToLauncher::Quit,
        ] {
            assert_eq!(ToLauncher::decode(&m.encode()).unwrap(), m);
        }
        for m in [FromLauncher::Engine { gen: 1, payload: vec![] }, FromLauncher::Ended { gen: 1, msg: "x".into() }] {
            assert_eq!(FromLauncher::decode(&m.encode()).unwrap(), m);
        }
        assert!(ToLauncher::decode(&[9]).is_err());
        assert!(FromLauncher::decode(&[1, 0]).is_err());
    }
}
