// SPDX-License-Identifier: MIT
//! The panel host's channel ends (§8.4): one reader thread per open stream, connected to the
//! provider's one-time pipe with the hub's nonce. Frames go to the hub's thread over an mpsc
//! queue and wake it; nothing polls. Reads wait in the kernel; closing a stream sets the
//! reader's stop flag and cancels its read, so the thread ends and drops the pipe at once.
//!
//! The hub drives it with [`StreamIo::pump`] on each repaint: router answers in, stream
//! commands out. The router never carries a frame (§5.4).

use crate::streams::{StreamCommand, StreamKey, Streams};
use foxglove_plugin::channel::{self, read_frame, ChannelConn};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Calls the hub back (a repaint) when a reader has something.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// From the reader threads.
#[derive(Debug)]
pub enum StreamEvent {
    Frame {
        key: StreamKey,
        gen: u64,
        payload: Vec<u8>,
    },
    /// The provider closed the channel, or the connect failed.
    Ended { key: StreamKey, gen: u64 },
}

struct Reader {
    stop: Arc<AtomicBool>,
    conn: Arc<Mutex<Option<ChannelConn>>>,
}

impl Reader {
    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(c) = self.conn.lock().unwrap().as_ref() {
            c.cancel();
        }
    }
}

/// The reader threads of the open streams.
pub struct StreamIo {
    tx: Sender<StreamEvent>,
    rx: Receiver<StreamEvent>,
    readers: HashMap<(StreamKey, u64), Reader>,
    waker: Waker,
}

impl StreamIo {
    pub fn new(waker: Waker) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            readers: HashMap::new(),
            waker,
        }
    }

    /// Streams with a reader thread right now (tests and the developer view).
    pub fn open_count(&self) -> usize {
        self.readers.len()
    }

    /// Connects to `pipe` with `nonce` on a thread of its own and reads frames until the
    /// provider closes or [`StreamIo::close`].
    pub fn connect(&mut self, key: StreamKey, gen: u64, pipe: String, nonce: String) {
        let stop = Arc::new(AtomicBool::new(false));
        let conn: Arc<Mutex<Option<ChannelConn>>> = Arc::new(Mutex::new(None));
        let (tx, waker) = (self.tx.clone(), self.waker.clone());
        let (t_stop, t_conn, t_key) = (stop.clone(), conn.clone(), key.clone());
        let spawned = std::thread::Builder::new()
            .name(format!("hideout-stream-{}", key.node))
            .spawn(move || {
                let send = |e: StreamEvent| {
                    if tx.send(e).is_ok() {
                        waker();
                    }
                };
                let mut c = match channel::connect(&pipe, &nonce) {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("stream {}: connect failed: {e}", t_key.node);
                        send(StreamEvent::Ended { key: t_key, gen });
                        return;
                    }
                };
                *t_conn.lock().unwrap() = Some(c.clone());
                while !t_stop.load(Ordering::SeqCst) {
                    match read_frame(&mut c) {
                        Ok(Some(payload)) if !t_stop.load(Ordering::SeqCst) => {
                            send(StreamEvent::Frame {
                                key: t_key.clone(),
                                gen,
                                payload,
                            })
                        }
                        Ok(Some(_)) => break,
                        Ok(None) | Err(_) => {
                            if !t_stop.load(Ordering::SeqCst) {
                                send(StreamEvent::Ended { key: t_key, gen });
                            }
                            break;
                        }
                    }
                }
                *t_conn.lock().unwrap() = None;
            });
        if spawned.is_ok() {
            self.readers.insert((key, gen), Reader { stop, conn });
        } else {
            log::warn!("stream {}: no reader thread", key.node);
        }
    }

    /// Stops the reader of `key`'s channel `gen`; the pipe closes as its thread ends.
    pub fn close(&mut self, key: &StreamKey, gen: u64) {
        if let Some(r) = self.readers.remove(&(key.clone(), gen)) {
            r.stop();
        }
    }

    /// Feeds what the readers sent into `streams`, runs its due reopens, closes what it closed,
    /// and returns the opens for the router (`Router::open_host_channel(module, channel)`;
    /// hand its `Out::HostChannel` answer to [`StreamIo::granted`]). Returns whether anything
    /// on screen changed.
    pub fn pump(&mut self, streams: &mut Streams, now: Instant) -> (bool, Vec<StreamCommand>) {
        let mut changed = false;
        let mut cmds = Vec::new();
        for e in self.rx.try_iter().collect::<Vec<_>>() {
            match e {
                StreamEvent::Frame { key, gen, payload } => {
                    if let Some(c) = streams.frame(&key, gen, &payload, now) {
                        cmds.push(c);
                    }
                    changed = true;
                }
                StreamEvent::Ended { key, gen } => {
                    self.readers.remove(&(key.clone(), gen));
                    streams.ended(&key, gen, now);
                    changed = true;
                }
            }
        }
        streams.tick(now);
        changed |= streams.present(now);
        cmds.extend(streams.take_commands());
        let mut opens = Vec::new();
        for c in cmds {
            match c {
                StreamCommand::Close { key, gen } => self.close(&key, gen),
                open => opens.push(open),
            }
        }
        (changed, opens)
    }

    /// The router's answer to an open: connects when the stream still wants it.
    pub fn granted(
        &mut self,
        streams: &mut Streams,
        module: &str,
        channel: &str,
        result: Result<Value, Value>,
    ) {
        if let Some((key, gen, pipe, nonce)) = streams.granted(module, channel, result) {
            self.connect(key, gen, pipe, nonce);
        }
    }
}

impl Drop for StreamIo {
    fn drop(&mut self) {
        for r in self.readers.values() {
            r.stop();
        }
    }
}

/// Windows' "Animation effects" setting is off (Settings > Accessibility > Visual effects):
/// the hub then draws canvas and image streams at 2 frames a second (§5.4, informative).
pub fn system_reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION,
    };
    let mut on: i32 = 1;
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut on as *mut i32).cast(),
            0,
        )
    };
    ok != 0 && on == 0
}
