//! Plumbing between the GUI and an engine.
//!
//! | Side | Type | Does |
//! |---|---|---|
//! | GUI | `Client` | `request()` returns an id at once; `poll()` drains replies and events. Never blocks. |
//! | engine | `Receiver<(ReqId, Request)>` + `Outbox` | read requests, send `Out` |
//!
//! `local_pair`: both ends in one process (tests, `--no-sandbox`).
//! `client_over_io` / `server_over_io`: the same over a byte stream (pipes), with frames.
//! A bad frame ends the stream: the GUI gets `Event::Fatal`, the engine sees its request channel close.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::frame::{read_frame, write_frame};
use crate::limits::MAX_FRAME;
use crate::msg::{decode_out, decode_request, encode_out, encode_request, Event, IndexOpts, Out, Reply, ReqId, Request};

/// Called from any thread when something arrived for the GUI (`ctx.request_repaint()`).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// GUI end of an engine.
pub struct Client {
    tx: Sender<(ReqId, Request)>,
    rx: Receiver<Out>,
    next: AtomicU64,
}

impl Client {
    pub fn from_channels(tx: Sender<(ReqId, Request)>, rx: Receiver<Out>) -> Self {
        Self { tx, rx, next: AtomicU64::new(1) }
    }

    /// Queue a request. The id is never 0 and never reused. If the engine is gone the request is dropped
    /// (an `Event::Fatal` is already on its way).
    pub fn request(&self, req: Request) -> ReqId {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send((id, req));
        id
    }

    pub fn cancel(&self, target: ReqId) {
        self.request(Request::Cancel { target });
    }

    /// Next message, if any. Call until `None` each frame.
    pub fn poll(&self) -> Option<Out> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.tx.send((0, Request::Shutdown));
    }
}

/// Engine end: sends replies and events. Cheap to clone, usable from any thread.
#[derive(Clone)]
pub struct Outbox {
    f: Arc<dyn Fn(Out) + Send + Sync>,
}

impl Outbox {
    pub fn new(f: impl Fn(Out) + Send + Sync + 'static) -> Self {
        Self { f: Arc::new(f) }
    }

    pub fn send(&self, out: Out) {
        (self.f)(out);
    }

    pub fn reply(&self, id: ReqId, reply: Reply) {
        self.send(Out::Reply(id, reply));
    }

    pub fn event(&self, ev: Event) {
        self.send(Out::Event(ev));
    }
}

/// Opens a folder: starts an engine for it and returns the GUI end. Must not block
/// (failures arrive as `Event::Fatal`). The previous engine of this opener is stopped.
pub trait Opener: Send {
    fn open(&mut self, root: &Path, opts: &IndexOpts, waker: Waker) -> Client;
    /// What confines the engine, for the status bar: `"seatbelt"`, `"none"`…
    fn sandbox(&self) -> &str;
}

/// Both ends in one process.
pub fn local_pair(waker: Waker) -> (Client, Receiver<(ReqId, Request)>, Outbox) {
    let (req_tx, req_rx) = channel();
    let (out_tx, out_rx) = channel();
    let outbox = Outbox::new(move |o| {
        let _ = out_tx.send(o);
        waker();
    });
    (Client::from_channels(req_tx, out_rx), req_rx, outbox)
}

/// GUI end over a byte stream. Two threads: encoder/writer, reader/decoder.
pub fn client_over_io(mut r: impl Read + Send + 'static, mut w: impl Write + Send + 'static, waker: Waker) -> Client {
    let (req_tx, req_rx) = channel::<(ReqId, Request)>();
    let (out_tx, out_rx) = channel();
    std::thread::Builder::new()
        .name("link-write".into())
        .spawn(move || {
            for (id, req) in req_rx {
                let last = req == Request::Shutdown;
                if write_frame(&mut w, &encode_request(id, &req)).is_err() || last {
                    break;
                }
            }
        })
        .expect("spawn thread");
    std::thread::Builder::new()
        .name("link-read".into())
        .spawn(move || {
            let end = loop {
                match read_frame(&mut r, MAX_FRAME) {
                    Ok(Some(buf)) => match decode_out(&buf) {
                        Ok(out) => {
                            if out_tx.send(out).is_err() {
                                return;
                            }
                            waker();
                        }
                        Err(e) => break format!("bad message from the engine: {e}"),
                    },
                    Ok(None) => break "the engine exited".to_owned(),
                    Err(e) => break format!("engine link: {e}"),
                }
            };
            let _ = out_tx.send(Out::Event(Event::Fatal { msg: end }));
            waker();
        })
        .expect("spawn thread");
    Client::from_channels(req_tx, out_rx)
}

/// Engine end over a byte stream. The request channel closes when the stream ends or sends garbage.
pub fn server_over_io(mut r: impl Read + Send + 'static, mut w: impl Write + Send + 'static) -> (Receiver<(ReqId, Request)>, Outbox) {
    let (req_tx, req_rx) = channel();
    let (out_tx, out_rx) = channel::<Out>();
    std::thread::Builder::new()
        .name("link-read".into())
        .spawn(move || {
            while let Ok(Some(buf)) = read_frame(&mut r, MAX_FRAME) {
                match decode_request(&buf) {
                    Ok(m) => {
                        if req_tx.send(m).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .expect("spawn thread");
    std::thread::Builder::new()
        .name("link-write".into())
        .spawn(move || {
            for out in out_rx {
                if write_frame(&mut w, &encode_out(&out)).is_err() {
                    break;
                }
            }
        })
        .expect("spawn thread");
    // `Sender` is not `Sync` on older toolchains: guard it.
    let out_tx = std::sync::Mutex::new(out_tx);
    let outbox = Outbox::new(move |o| {
        if let Ok(tx) = out_tx.lock() {
            let _ = tx.send(o);
        }
    });
    (req_rx, outbox)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::RelPath;
    use std::time::Duration;

    fn noop() -> Waker {
        Arc::new(|| {})
    }

    fn wait(c: &Client) -> Out {
        for _ in 0..2000 {
            if let Some(o) = c.poll() {
                return o;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("no message");
    }

    #[test]
    fn local_round_trip() {
        let (client, rx, out) = local_pair(noop());
        let id = client.request(Request::Stat { path: RelPath::root() });
        let (got, req) = rx.recv().unwrap();
        assert_eq!((got, &req), (id, &Request::Stat { path: RelPath::root() }));
        out.reply(got, Reply::Done);
        assert_eq!(wait(&client), Out::Reply(id, Reply::Done));
        assert_ne!(client.request(Request::Reindex { opts: IndexOpts::default() }), id);
    }

    /// Two in-memory pipes built from channels.
    struct PipeR(Receiver<Vec<u8>>, Vec<u8>);
    struct PipeW(Sender<Vec<u8>>);

    impl Read for PipeR {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(v) => self.1 = v,
                    Err(_) => return Ok(0),
                }
            }
            let n = buf.len().min(self.1.len());
            buf[..n].copy_from_slice(&self.1[..n]);
            self.1.drain(..n);
            Ok(n)
        }
    }

    impl Write for PipeW {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.send(buf.to_vec()).map_err(|_| std::io::ErrorKind::BrokenPipe)?;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn pipe() -> (PipeR, PipeW) {
        let (tx, rx) = channel();
        (PipeR(rx, Vec::new()), PipeW(tx))
    }

    #[test]
    fn io_round_trip_and_fatal_on_close() {
        let (c2s_r, c2s_w) = pipe();
        let (s2c_r, s2c_w) = pipe();
        let client = client_over_io(s2c_r, c2s_w, noop());
        let (rx, out) = server_over_io(c2s_r, s2c_w);
        let id = client.request(Request::FindFiles { query: "x".into(), limit: 3 });
        let (got, req) = rx.recv().unwrap();
        assert_eq!(got, id);
        assert_eq!(req, Request::FindFiles { query: "x".into(), limit: 3 });
        out.reply(got, Reply::Done);
        out.event(Event::Fatal { msg: "x".into() });
        assert_eq!(wait(&client), Out::Reply(id, Reply::Done));
        assert_eq!(wait(&client), Out::Event(Event::Fatal { msg: "x".into() }));
        drop(out);
        drop(rx);
        // Server writer gone → the client reports the end of the link.
        assert!(matches!(wait(&client), Out::Event(Event::Fatal { .. })));
    }

    #[test]
    fn garbage_from_the_engine_is_fatal_not_a_panic() {
        let (s2c_r, mut s2c_w) = pipe();
        let (_c2s_r, c2s_w) = pipe();
        let client = client_over_io(s2c_r, c2s_w, noop());
        write_frame(&mut s2c_w, &[9, 9, 9]).unwrap();
        assert!(matches!(wait(&client), Out::Event(Event::Fatal { .. })));
    }
}
