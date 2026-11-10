// SPDX-License-Identifier: MIT
//! Live streams in panels (contract §5.4, §8.4; H2.9, UX: docs/hideout-streams-ux.md).
//!
//! A `canvas`, `image`, `text` or `markdown` node with a `stream` is one channel end of the hub's
//! panel host. [`Streams`] is the whole stream logic with no I/O and the clock passed in:
//!
//! - **Visible only.** [`Streams::sync`] opens a node's stream while its page panel is on screen
//!   and the module is running, and closes it otherwise ([`StreamCommand`]s for the hub). A node
//!   that loses its `stream` (patched to `null`, removed, replaced) closes cleanly: no state is
//!   kept or shown and nothing reopens.
//! - **Latest frame wins.** A canvas or image frame waits in a one-frame slot; [`Streams::present`]
//!   moves it on screen at most 30 times a second (2 with reduced motion). A frame that arrives
//!   while the slot is still full replaces it and counts as skipped: frames are never queued.
//! - **Reopen.** A stream its provider ends while the panel stays visible is reopened after 1, 2
//!   and 4 s (`STREAM_REOPEN_S`); when the third reopen ends too it is "Stopped updating" until
//!   Try again (one reopen). A stream over a cap (frame size, 60 frames a second) or whose grant
//!   failed is refused and never reopened.
//! - **Text.** Frames append, up to 1 MiB per node (`STREAM_TEXT_BYTES`), oldest whole lines
//!   dropped first. A patch of the node's `text` replaces what was shown, the appended text
//!   included, while text appended after the patch arrived stays.
//! - **What it looks like.** [`Streams::look`] is the hub-drawn state of a node ([`Look`]);
//!   frames never carry state marks, and nothing here is ever announced to a screen reader.
//!
//! The pipes themselves are [`crate::stream_io`]. Clean-room: written from the contract and the
//! UX plan; it knows no module ids.

use crate::model::{HostedPanel, PanelKey};
use hideout_contract::frame::{decode_payload, Frame};
use hideout_contract::messages::{PatchOp, PatchOpKind};
use hideout_contract::module::Slot;
use hideout_contract::panel::{
    Node, MAX_FRAME_BYTES, MAX_STREAM_FPS, STREAM_REOPEN_S, STREAM_TEXT_BYTES,
};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Most canvas or image frames drawn a second (§5.4).
pub const DRAW_FPS: u32 = 30;
/// With the system's animation effects off (§5.4, informative; UX decision 2).
pub const REDUCED_MOTION_FPS: u32 = 2;
/// "Connecting…" shows only when an open takes longer than this (UX: never a spinner flash).
pub const CONNECTING_AFTER: Duration = Duration::from_secs(1);
/// Back on screen: the last frame stays undimmed this long while the stream reopens.
pub const BACK_GRACE: Duration = Duration::from_secs(2);
/// The first line of a trimmed text node (UX wording).
pub const TRIMMED_LINE: &str = "Earlier text was trimmed.";

/// One stream node: its panel and node id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamKey {
    pub panel: PanelKey,
    pub node: String,
}

impl StreamKey {
    pub fn new(panel: PanelKey, node: impl Into<String>) -> Self {
        Self {
            panel,
            node: node.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Canvas,
    Image,
    /// `text` and `markdown`: appended text.
    Text,
}

impl StreamKind {
    fn of(node: &Node) -> Option<Self> {
        match node.kind.as_str() {
            "canvas" => Some(Self::Canvas),
            "image" => Some(Self::Image),
            "text" | "markdown" => Some(Self::Text),
            _ => None,
        }
    }
}

/// Where a stream's channel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Not open: the panel is hidden, or the module isn't running.
    Closed,
    /// `channel.open` sent or the pipe connecting, no frame yet on this channel.
    Connecting,
    /// Frames flowing.
    Open,
    /// The provider ended it; a reopen is due at `reopen_at`.
    Reopening,
    /// The user pressed Pause: closed as if hidden, the last frame at full colour.
    Paused,
    /// The third reopen ended too: "Stopped updating" with Try again.
    Stopped,
    /// Refused or closed for `ui.limits`, or the grant failed: "couldn't be shown", no reopen.
    Refused,
}

/// The last canvas colours or image drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Picture {
    Canvas(Vec<[u8; 4]>),
    Image {
        width: u16,
        height: u16,
        rgba: Vec<[u8; 4]>,
    },
}

/// What the hub draws for a node: the frame's state, never the module's words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// Opening, under a second: an empty box (or nothing for text).
    Empty,
    /// Opening for over a second: "Connecting…".
    Connecting,
    /// The frames, full colour; with reduced motion a small "Live" dot.
    Live,
    /// The user's Pause: full colour, "Paused" and Play.
    Paused,
    /// Dimmed, "Last seen <n> s ago": no new frames past `stale_after_ms`, back on screen past
    /// 2 s, or the module not running.
    LastSeen(Duration),
    /// Dimmed, "Stopped updating" and Try again.
    Stopped,
    /// "This preview couldn't be shown." and Open log.
    Refused,
}

/// What the hub does for the streams: open a channel through the router
/// (`Router::open_host_channel`), or close the pipe it reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamCommand {
    Open {
        key: StreamKey,
        module: String,
        channel: String,
        gen: u64,
    },
    Close {
        key: StreamKey,
        gen: u64,
    },
}

/// A control the hub draws on a stream node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamAct {
    Pause(StreamKey),
    Play(StreamKey),
    TryAgain(StreamKey),
    /// "Open log" on a refused stream: the hub opens its log view.
    OpenLog(StreamKey),
}

#[derive(Debug)]
pub struct Stream {
    pub kind: StreamKind,
    pub module: String,
    pub channel: String,
    pub phase: Phase,
    /// The channel generation frames must carry; each open makes a new one.
    gen: u64,
    pub picture: Option<Picture>,
    /// Bumped each time `picture` changes (the renderer's texture key).
    pub picture_seq: u64,
    /// A streamed image without `aspect` takes its first frame's, once (§5.2, v1.5).
    pub first_aspect: Option<f32>,
    pending: Option<Picture>,
    /// Appended text (text streams), after trimming.
    pub text: String,
    pub trimmed: bool,
    appended_total: u64,
    trimmed_total: u64,
    /// `appended_total` when a patch setting `text` arrived; it lands later (coalescing).
    replace_marks: VecDeque<u64>,
    pub last_frame: Option<Instant>,
    last_drawn: Option<Instant>,
    opened_at: Option<Instant>,
    /// Back on screen with an old frame: when.
    back_at: Option<Instant>,
    reopens: usize,
    reopen_at: Option<Instant>,
    stale_after: Option<Duration>,
    rate: (Option<Instant>, u32),
    /// Frames drawn, skipped by the cap, and dropped as malformed (developer mode, the log).
    pub drawn: u64,
    pub skipped: u64,
    pub dropped: u64,
}

impl Stream {
    fn new(kind: StreamKind, module: &str, channel: &str, stale_after: Option<Duration>) -> Self {
        Self {
            kind,
            module: module.into(),
            channel: channel.into(),
            phase: Phase::Closed,
            gen: 0,
            picture: None,
            picture_seq: 0,
            first_aspect: None,
            pending: None,
            text: String::new(),
            trimmed: false,
            appended_total: 0,
            trimmed_total: 0,
            replace_marks: VecDeque::new(),
            last_frame: None,
            last_drawn: None,
            opened_at: None,
            back_at: None,
            reopens: 0,
            reopen_at: None,
            stale_after,
            rate: (None, 0),
            drawn: 0,
            skipped: 0,
            dropped: 0,
        }
    }

    fn is_open(&self) -> bool {
        matches!(self.phase, Phase::Connecting | Phase::Open)
    }

    /// Appends `s`, then drops the oldest whole lines past the cap; a single line over the cap
    /// keeps its newest bytes, cut at a character boundary (§5.5, v1.5).
    fn append(&mut self, s: &str, cap: usize) {
        self.text.push_str(s);
        self.appended_total += s.len() as u64;
        if self.text.len() <= cap {
            return;
        }
        let len = self.text.len();
        let excess = len - cap;
        // The first line feed at or after byte `excess - 1` ends the last line that must go;
        // when that leaves nothing, the newest line alone is over the cap: keep its newest bytes.
        let from = (excess - 1..=len)
            .find(|&i| self.text.is_char_boundary(i))
            .unwrap_or(len);
        let cut = match self.text[from..].find('\n') {
            Some(i) if from + i + 1 < len => from + i + 1,
            _ => (excess..=len)
                .find(|&i| self.text.is_char_boundary(i))
                .unwrap_or(len),
        };
        self.text.drain(..cut);
        self.trimmed_total += cut as u64;
        self.trimmed = true;
    }
}

/// Every stream node the hub holds.
#[derive(Debug)]
pub struct Streams {
    map: BTreeMap<StreamKey, Stream>,
    out: Vec<StreamCommand>,
    /// Opens sent to the router, oldest first, to match its answers by module and channel.
    awaiting: VecDeque<(String, String, StreamKey, u64)>,
    next_gen: u64,
    pub reduced_motion: bool,
    text_cap: usize,
}

impl Default for Streams {
    fn default() -> Self {
        Self::with_text_cap(STREAM_TEXT_BYTES)
    }
}

/// Stream nodes of a tree, in tree order.
fn stream_nodes<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
    if n.stream_name().is_some() && StreamKind::of(n).is_some() {
        out.push(n);
    }
    for c in &n.children {
        stream_nodes(c, out);
    }
}

fn stale_after(n: &Node) -> Option<Duration> {
    n.props
        .get("stale_after_ms")
        .and_then(Value::as_u64)
        .filter(|&ms| ms > 0)
        .map(Duration::from_millis)
}

impl Streams {
    /// A host whose stream text cap is `cap` bytes (tests use a small one).
    pub fn with_text_cap(cap: usize) -> Self {
        Self {
            map: BTreeMap::new(),
            out: Vec::new(),
            awaiting: VecDeque::new(),
            next_gen: 1,
            reduced_motion: false,
            text_cap: cap,
        }
    }

    pub fn get(&self, key: &StreamKey) -> Option<&Stream> {
        self.map.get(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&StreamKey, &Stream)> {
        self.map.iter()
    }

    /// The commands since the last call, in order. The hub sends a panel's `ui.visibility`
    /// before its opens (§5.9): it takes these after the visibility notifications.
    pub fn take_commands(&mut self) -> Vec<StreamCommand> {
        std::mem::take(&mut self.out)
    }

    fn open(&mut self, key: &StreamKey, now: Instant) {
        let gen = self.next_gen;
        self.next_gen += 1;
        let s = self.map.get_mut(key).expect("known stream");
        s.gen = gen;
        s.phase = Phase::Connecting;
        s.opened_at = Some(now);
        s.reopen_at = None;
        s.rate = (None, 0);
        self.awaiting
            .push_back((s.module.clone(), s.channel.clone(), key.clone(), gen));
        self.out.push(StreamCommand::Open {
            key: key.clone(),
            module: s.module.clone(),
            channel: s.channel.clone(),
            gen,
        });
    }

    fn close(&mut self, key: &StreamKey, then: Phase) {
        let s = self.map.get_mut(key).expect("known stream");
        if s.is_open() {
            self.out.push(StreamCommand::Close {
                key: key.clone(),
                gen: s.gen,
            });
        }
        // Frames or answers for the old channel no longer apply.
        s.gen = 0;
        s.phase = then;
        s.reopen_at = None;
        s.pending = None;
    }

    /// Brings the streams in line with the panels: `shown` are the page panels on screen (the
    /// active tab, pop-outs that aren't minimised). Opens what is visible, closes the rest.
    pub fn sync(&mut self, panels: &[HostedPanel], shown: &[PanelKey], now: Instant) {
        let mut present: BTreeMap<StreamKey, (StreamKind, String, Option<Duration>, bool)> =
            BTreeMap::new();
        for p in panels.iter().filter(|p| p.slot == Slot::Page) {
            let mut nodes = Vec::new();
            stream_nodes(&p.tree, &mut nodes);
            let visible = p.live() && shown.contains(&p.key);
            for n in nodes {
                let kind = StreamKind::of(n).expect("a stream node");
                let channel = n.stream_name().expect("a stream").to_string();
                present.insert(
                    StreamKey::new(p.key.clone(), &n.id),
                    (kind, channel, stale_after(n), visible),
                );
            }
        }
        let running = |k: &StreamKey| panels.iter().any(|p| p.key == k.panel && p.live());
        // Gone, or bound to another channel: a clean close, nothing kept.
        let stale: Vec<StreamKey> = self
            .map
            .iter()
            .filter(|(k, s)| {
                present
                    .get(*k)
                    .is_none_or(|(kind, ch, _, _)| *kind != s.kind || *ch != s.channel)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            self.close(&k, Phase::Closed);
            self.map.remove(&k);
        }
        for (k, (kind, channel, stale_after, visible)) in present {
            let s = self
                .map
                .entry(k.clone())
                .or_insert_with(|| Stream::new(kind, &k.panel.module, &channel, stale_after));
            s.stale_after = stale_after;
            if visible {
                if s.phase == Phase::Closed {
                    // On screen again: the count starts afresh (§5.4).
                    s.reopens = 0;
                    s.back_at = (s.picture.is_some()).then_some(now);
                    self.open(&k, now);
                }
            } else if !running(&k) {
                // A module restart starts afresh: even Stopped or Refused may open again.
                if s.phase != Phase::Closed {
                    self.close(&k, Phase::Closed);
                }
            } else if matches!(
                s.phase,
                Phase::Connecting | Phase::Open | Phase::Reopening | Phase::Stopped
            ) {
                // Hidden: closed; back on screen starts the reopen count afresh (§5.4).
                self.close(&k, Phase::Closed);
            }
        }
    }

    /// The router's answer to an open of `module`'s `channel` (`Out::HostChannel`). Returns the
    /// stream, its generation and `{pipe, nonce}` to connect to; `None` when the stream is gone
    /// or was closed meanwhile (the provider's pipe then times out unused), or refused.
    pub fn granted(
        &mut self,
        module: &str,
        channel: &str,
        result: Result<Value, Value>,
    ) -> Option<(StreamKey, u64, String, String)> {
        let i = self
            .awaiting
            .iter()
            .position(|(m, c, _, _)| m == module && c == channel)?;
        let (_, _, key, gen) = self.awaiting.remove(i)?;
        let s = self.map.get_mut(&key).filter(|s| s.gen == gen)?;
        match result {
            Ok(v) => match (v["pipe"].as_str(), v["nonce"].as_str()) {
                (Some(p), Some(n)) => Some((key, gen, p.to_string(), n.to_string())),
                _ => {
                    s.phase = Phase::Refused;
                    None
                }
            },
            Err(e) => {
                log::warn!(
                    "{module}: stream {channel} for {}/{} not granted: {e}",
                    key.panel.panel,
                    key.node
                );
                s.phase = Phase::Refused;
                s.gen = 0;
                None
            }
        }
    }

    /// One payload read from `key`'s channel `gen`. Returns `Some(Close)` when the stream broke
    /// a cap and is refused (the hub closes the pipe), else `None`.
    pub fn frame(
        &mut self,
        key: &StreamKey,
        gen: u64,
        payload: &[u8],
        now: Instant,
    ) -> Option<StreamCommand> {
        let cap = self.text_cap;
        let s = self.map.get_mut(key).filter(|s| s.gen == gen && gen != 0)?;
        let over = |s: &mut Stream, why: &str| {
            log::warn!(
                "{}: stream {} closed: {why} (ui.limits)",
                s.module,
                s.channel
            );
            s.phase = Phase::Refused;
            s.pending = None;
            s.gen = 0;
            Some(StreamCommand::Close {
                key: key.clone(),
                gen,
            })
        };
        if payload.len() > MAX_FRAME_BYTES {
            return over(s, "a frame over 4 MiB");
        }
        match s.rate {
            (Some(start), n) if now.duration_since(start) < Duration::from_secs(1) => {
                s.rate.1 = n + 1;
                if s.rate.1 > MAX_STREAM_FPS {
                    return over(s, "over 60 frames a second");
                }
            }
            _ => s.rate = (Some(now), 1),
        }
        let picture = match (decode_payload(payload), s.kind) {
            (Ok(Frame::Canvas { colours, .. }), StreamKind::Canvas) => Picture::Canvas(colours),
            (
                Ok(Frame::Image {
                    width,
                    height,
                    rgba,
                    ..
                }),
                StreamKind::Image,
            ) => Picture::Image {
                width,
                height,
                rgba,
            },
            (Ok(Frame::Text { append, .. }), StreamKind::Text) => {
                s.append(&append, cap);
                s.phase = Phase::Open;
                s.last_frame = Some(now);
                s.back_at = None;
                return None;
            }
            (r, _) => {
                // Dropped silently on screen, counted for the log (UX: dropped frames).
                s.dropped += 1;
                log::debug!("{}: stream {} frame dropped: {r:?}", s.module, s.channel);
                return None;
            }
        };
        if s.pending.replace(picture).is_some() {
            s.skipped += 1;
        }
        s.phase = Phase::Open;
        s.last_frame = Some(now);
        None
    }

    /// The provider ended `key`'s channel `gen` (the pipe closed). Schedules the next reopen
    /// (1, 2, 4 s), or shows "Stopped updating" after the third.
    pub fn ended(&mut self, key: &StreamKey, gen: u64, now: Instant) {
        let Some(s) = self.map.get_mut(key).filter(|s| s.gen == gen && gen != 0) else {
            return;
        };
        s.gen = 0;
        s.pending = None;
        if let Some(wait) = STREAM_REOPEN_S.get(s.reopens) {
            s.reopens += 1;
            s.phase = Phase::Reopening;
            s.reopen_at = Some(now + Duration::from_secs(*wait));
        } else {
            s.phase = Phase::Stopped;
        }
    }

    /// Opens the reopens that are due.
    pub fn tick(&mut self, now: Instant) {
        let due: Vec<StreamKey> = self
            .map
            .iter()
            .filter(|(_, s)| s.phase == Phase::Reopening && s.reopen_at.is_some_and(|t| t <= now))
            .map(|(k, _)| k.clone())
            .collect();
        for k in due {
            self.open(&k, now);
        }
    }

    fn interval(&self) -> Duration {
        let fps = if self.reduced_motion {
            REDUCED_MOTION_FPS
        } else {
            DRAW_FPS
        };
        Duration::from_secs(1) / fps
    }

    /// Moves waiting frames on screen where the cap allows; `true` when something changed.
    /// Called on each repaint; [`Streams::next_wake`] says when the next one is due.
    pub fn present(&mut self, now: Instant) -> bool {
        let every = self.interval();
        let mut changed = false;
        for s in self.map.values_mut() {
            if s.pending.is_some() && s.last_drawn.is_none_or(|t| now.duration_since(t) >= every) {
                s.picture = s.pending.take();
                if let (None, Some(Picture::Image { width, height, .. })) =
                    (s.first_aspect, &s.picture)
                {
                    s.first_aspect = Some(f32::from(*width).max(1.0) / f32::from(*height).max(1.0));
                }
                s.picture_seq += 1;
                s.drawn += 1;
                s.last_drawn = Some(now);
                s.back_at = None;
                changed = true;
            }
        }
        changed
    }

    /// When the hub should look again (one repaint then, never a timer loop): the next frame
    /// slot, a reopen, the 1 s "Connecting…" mark, the 2 s back-on-screen grace, or a stale gap.
    pub fn next_wake(&self) -> Option<Instant> {
        let every = self.interval();
        let mut at: Option<Instant> = None;
        let mut want = |t: Option<Instant>| {
            if let Some(t) = t {
                at = Some(at.map_or(t, |a| a.min(t)));
            }
        };
        for s in self.map.values() {
            if s.pending.is_some() {
                want(Some(s.last_drawn.map_or_else(Instant::now, |t| t + every)));
            }
            want(s.reopen_at);
            if s.phase == Phase::Connecting && s.picture.is_none() {
                want(s.opened_at.map(|t| t + CONNECTING_AFTER));
            }
            want(s.back_at.map(|t| t + BACK_GRACE));
            if s.phase == Phase::Open && s.kind != StreamKind::Text {
                want(s.stale_after.zip(s.last_frame).map(|(d, t)| t + d));
            }
        }
        at
    }

    /// The node's state as the hub draws it. `running`: the module's state is `running`.
    pub fn look(&self, key: &StreamKey, running: bool, now: Instant) -> Look {
        let Some(s) = self.map.get(key) else {
            return Look::Empty;
        };
        let since = |t: Option<Instant>| t.map_or(Duration::ZERO, |t| now.duration_since(t));
        let has = s.picture.is_some() || !s.text.is_empty() || s.last_frame.is_some();
        match s.phase {
            Phase::Refused => Look::Refused,
            Phase::Paused => Look::Paused,
            _ if !running => {
                if has {
                    Look::LastSeen(since(s.last_frame))
                } else {
                    Look::Empty
                }
            }
            Phase::Stopped => Look::Stopped,
            _ if s.kind == StreamKind::Text => Look::Live,
            _ if s.picture.is_none() => match s.phase {
                Phase::Connecting | Phase::Reopening if since(s.opened_at) >= CONNECTING_AFTER => {
                    Look::Connecting
                }
                _ => Look::Empty,
            },
            _ => {
                if let Some(back) = s.back_at {
                    if now.duration_since(back) >= BACK_GRACE {
                        return Look::LastSeen(since(s.last_frame));
                    }
                    return Look::Live;
                }
                match (s.stale_after, s.last_frame) {
                    (Some(d), Some(t)) if now.duration_since(t) > d => {
                        Look::LastSeen(now.duration_since(t))
                    }
                    _ => Look::Live,
                }
            }
        }
    }

    /// A control pressed on a stream node.
    pub fn act(&mut self, act: &StreamAct, now: Instant) {
        match act {
            StreamAct::Pause(k) if self.map.contains_key(k) => self.close(k, Phase::Paused),
            StreamAct::Play(k) if self.map.get(k).is_some_and(|s| s.phase == Phase::Paused) => {
                self.map.get_mut(k).unwrap().reopens = 0;
                self.open(k, now);
            }
            // "Try again" reopens once; if that ends too, it is Stopped again.
            StreamAct::TryAgain(k)
                if self
                    .map
                    .get(k)
                    .is_some_and(|s| matches!(s.phase, Phase::Stopped)) =>
            {
                self.open(k, now)
            }
            _ => {}
        }
    }

    /// A `ui.patch` for `panel` arrived (before coalescing): marks where a `text` set will cut.
    pub fn patch_received(&mut self, panel: &PanelKey, ops: &[PatchOp]) {
        for k in self.text_sets(panel, ops) {
            if let Some(s) = self.map.get_mut(&k) {
                s.replace_marks.push_back(s.appended_total);
            }
        }
    }

    /// A `ui.patch` landed on `panel`'s tree: a `text` set replaces everything shown, so the
    /// text appended before the patch arrived goes; text appended since stays (§5.3, §5.4).
    pub fn patch_landed(&mut self, panel: &PanelKey, ops: &[PatchOp]) {
        for k in self.text_sets(panel, ops) {
            if let Some(s) = self.map.get_mut(&k) {
                let mark = s.replace_marks.pop_front().unwrap_or(s.appended_total);
                let cut = mark.saturating_sub(s.trimmed_total) as usize;
                let cut = cut.min(s.text.len());
                let cut = (cut..=s.text.len())
                    .find(|&i| s.text.is_char_boundary(i))
                    .unwrap_or(s.text.len());
                s.text.drain(..cut);
                s.trimmed_total += cut as u64;
                s.trimmed = false;
            }
        }
    }

    fn text_sets(&self, panel: &PanelKey, ops: &[PatchOp]) -> Vec<StreamKey> {
        ops.iter()
            .filter(|o| o.op == PatchOpKind::Set)
            .filter(|o| o.props.as_ref().is_some_and(|p| p.contains_key("text")))
            .map(|o| StreamKey::new(panel.clone(), &o.node))
            .filter(|k| self.map.get(k).is_some_and(|s| s.kind == StreamKind::Text))
            .collect()
    }
}

/// The words of a [`Look`] for the node's accessible description and its chip (UX wording).
/// Never a channel, pipe, nonce, frame, seq, fps or byte count.
pub fn look_words(look: Look) -> Option<String> {
    let ago = |d: Duration| {
        let s = d.as_secs();
        if s >= 60 {
            format!("Last seen {} min ago", s / 60)
        } else {
            format!("Last seen {s} s ago")
        }
    };
    match look {
        Look::Empty | Look::Live => None,
        Look::Connecting => Some("Connecting\u{2026}".into()),
        Look::Paused => Some("Paused".into()),
        Look::LastSeen(d) => Some(ago(d)),
        Look::Stopped => Some("Stopped updating".into()),
        Look::Refused => Some("This preview couldn't be shown.".into()),
    }
}

/// The screen reader's description of a stream node: "<label>, live preview" and so on. It is
/// a description, never a live region: frames are not announced.
pub fn accessible_description(label: &str, look: Look) -> String {
    let state = match look {
        Look::Paused => "paused".to_string(),
        Look::LastSeen(d) => format!("not updating, last seen {} seconds ago", d.as_secs()),
        Look::Stopped => "not updating".to_string(),
        Look::Refused => "couldn't be shown".to_string(),
        Look::Empty | Look::Connecting | Look::Live => "live preview".to_string(),
    };
    if label.is_empty() {
        state
    } else {
        format!("{label}, {state}")
    }
}
