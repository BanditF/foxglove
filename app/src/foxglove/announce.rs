//! A small shared way to say something aloud to a screen reader (#1366): a polite live region.
//!
//! A page calls [`say`] with the words when something changed that a sighted user sees (a
//! stepper's new value), and draws [`region`] once, always, where the controls are. The region
//! has no size and no looks; its accessible name is the last thing said, and it is marked polite,
//! so the reader speaks each new text without taking the keyboard focus.
//!
//! Words are the caller's: say only what is known (never a missing reading as 0).
//!
//! The window has one more region, [`app`], drawn once by the shell for every page (#1376):
//! [`notice`] says a change of state through it, [`watch`] turns a changing value into notices.
//! Both are kept quiet: the same words twice inside [`GAP_S`] are said once, and two different
//! notices are at least [`GAP_S`] apart (later ones wait their turn, oldest first, so a failure is
//! never replaced by a later, happier notice). The same words again after the gap are said again.
//! Long or foreign text is cut by [`brief`] before it is spoken.

use egui::accesskit::Live;

/// The memory slot of what was last said, per region id.
fn slot(id: egui::Id) -> egui::Id {
    id.with("announce-said")
}

/// Says `text` through the region named `id` (the next [`region`] call with the same id).
pub fn say(ctx: &egui::Context, id: egui::Id, text: impl Into<String>) {
    let text: String = text.into();
    ctx.data_mut(|d| d.insert_temp(slot(id), text));
}

/// What the region holds now (the last thing said), if anything.
pub fn said(ctx: &egui::Context, id: egui::Id) -> Option<String> {
    ctx.data(|d| d.get_temp::<String>(slot(id)))
}

/// Draws the live region: nothing to see, a polite node with the last thing said as its name.
pub fn region(ui: &mut egui::Ui, id: egui::Id) {
    let text = said(ui.ctx(), id).unwrap_or_default();
    // The node's id is the region's own, not the next automatic one: an automatic id moves
    // when the widgets drawn before it change, and a reader then sees a new node holding the old
    // words (said twice, or a change missed).
    let (_, rect) = ui.allocate_space(egui::Vec2::ZERO);
    let r = ui.interact(rect, id.with("region"), egui::Sense::hover());
    let node = r.id;
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text.clone()));
    // The name is the words (a plain label node keeps its text as a value, which a live region
    // may not speak), as the hub's crash bar does.
    ui.ctx().accesskit_node_builder(node, |n| {
        n.set_live(Live::Polite);
        n.set_label(text.clone());
    });
}

/// The least time between two notices, in seconds; the same words inside it are one notice.
pub const GAP_S: f64 = 2.0;
/// The most notices kept waiting; past it the oldest is dropped.
const QUEUE_MAX: usize = 4;
/// The longest spoken copy of a message, in characters (see [`brief`]).
pub const BRIEF_MAX: usize = 120;

/// The window's region (one per window: drawn by the shell, said to by every page).
pub fn app_id() -> egui::Id {
    egui::Id::new("app-announce")
}

/// What the window's region remembers between frames.
#[derive(Clone, Debug, Default)]
struct Mem {
    /// The last words said.
    last: Option<String>,
    /// When they were said (the context's time).
    at: f64,
    /// Words waiting for their turn, oldest first.
    queue: Vec<String>,
    /// Words that repeat what the region holds: it is blanked for a frame, then these are set,
    /// so a reader sees a change.
    repeat: Option<String>,
}

fn mem(ctx: &egui::Context) -> Mem {
    ctx.data(|d| d.get_temp(app_id().with("mem")))
        .unwrap_or_default()
}

fn keep(ctx: &egui::Context, m: Mem) {
    ctx.data_mut(|d| d.insert_temp(app_id().with("mem"), m));
}

/// The first line of `text`, at most [`BRIEF_MAX`] characters, cut at a word: what is worth
/// speaking of a long or foreign message. When something is left out it says so.
pub fn brief(text: &str) -> String {
    let text = text.trim();
    let line = text.lines().next().unwrap_or("").trim();
    let mut cut = line.len() < text.len();
    let mut out = line.to_string();
    if line.chars().count() > BRIEF_MAX {
        cut = true;
        let end = line
            .char_indices()
            .nth(BRIEF_MAX)
            .map_or(line.len(), |(i, _)| i);
        let head = &line[..end];
        out = head
            .rsplit_once(char::is_whitespace)
            .map_or(head, |(w, _)| w)
            .trim_end()
            .to_string();
    }
    if cut {
        out.push('…');
        out.push_str(" Details on the screen.");
    }
    out
}

/// Sets `words` as the region's name now (blanking it for a frame first when it already holds
/// them) and remembers when.
fn emit(ctx: &egui::Context, m: &mut Mem, words: String, now: f64) {
    if said(ctx, app_id()).as_deref() == Some(words.as_str()) {
        say(ctx, app_id(), "");
        m.repeat = Some(words.clone());
        ctx.request_repaint();
    } else {
        say(ctx, app_id(), words.clone());
    }
    ctx.data_mut(|d| d.insert_temp(app_id().with(("recent", words.clone())), now));
    m.last = Some(words);
    m.at = now;
}

/// Whether `words` were said in the last `secs` seconds (for words that may flap).
pub fn recent(ctx: &egui::Context, words: &str, secs: f64) -> bool {
    let now = ctx.input(|i| i.time);
    ctx.data(|d| d.get_temp::<f64>(app_id().with(("recent", words.to_string()))))
        .is_some_and(|at| now - at < secs)
}

/// Says `words` through the window's region at time `now`. Returns whether they were said now
/// (not a repeat inside the gap, not held back).
pub fn notice_at(ctx: &egui::Context, words: impl Into<String>, now: f64) -> bool {
    let words: String = words.into();
    let mut m = mem(ctx);
    let said_now = if !m.queue.is_empty() {
        // Others wait: keep the order; one repeat after another is one notice.
        if m.queue.last() != Some(&words) {
            if m.queue.len() >= QUEUE_MAX {
                m.queue.remove(0);
            }
            m.queue.push(words);
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
            (GAP_S - (now - m.at)).max(0.05),
        ));
        false
    } else if m.last.is_some() && now - m.at < GAP_S {
        if m.last.as_deref() != Some(words.as_str()) {
            m.queue.push(words);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(GAP_S - (now - m.at)));
        }
        false
    } else {
        emit(ctx, &mut m, words, now);
        true
    };
    keep(ctx, m);
    said_now
}

/// [`notice_at`] at the context's current time.
pub fn notice(ctx: &egui::Context, words: impl Into<String>) -> bool {
    let now = ctx.input(|i| i.time);
    notice_at(ctx, words, now)
}

/// Says the oldest waiting notice once its time is up.
fn flush(ctx: &egui::Context, now: f64) {
    let mut m = mem(ctx);
    if m.queue.is_empty() || now - m.at < GAP_S {
        return;
    }
    let words = m.queue.remove(0);
    emit(ctx, &mut m, words, now);
    if !m.queue.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(GAP_S));
    }
    keep(ctx, m);
}

/// Draws the window's live region (the shell calls this once per frame, in a part of the window
/// whose size does not depend on its content) and says a waiting notice whose time has come.
/// A window that was not drawn in the frame before (opened again) starts with nothing said.
pub fn app(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    if !continuous(&ctx, "app-region") {
        keep(&ctx, Mem::default());
        say(&ctx, app_id(), "");
    }
    flush(&ctx, now);
    region(ui, app_id());
    let mut m = mem(&ctx);
    if let Some(words) = m.repeat.take() {
        say(&ctx, app_id(), words);
        ctx.request_repaint();
        keep(&ctx, m);
    }
}

/// Whether the thing called `key` was also seen in the frame before this one (of this window);
/// stamps it as seen now. False the first time, and after a gap (a page left, a window closed).
fn continuous(ctx: &egui::Context, key: &str) -> bool {
    let frame = ctx.cumulative_frame_nr_for(ctx.viewport_id());
    let id = app_id().with(("seen", key));
    let before: Option<u64> = ctx.data(|d| d.get_temp(id));
    ctx.data_mut(|d| d.insert_temp(id, frame));
    before.is_some_and(|b| b + 1 >= frame)
}

/// Watches `state` (named `key`, one per thing watched): the first time it is seen, or when it
/// was not seen in the frame before (its page was left, its window closed), nothing is said (a
/// state that changed while it was out of sight is not news); after that, `words(old, new)` is
/// said through [`notice`] whenever it changes and returns words.
pub fn watch<T>(
    ctx: &egui::Context,
    key: &str,
    state: T,
    words: impl FnOnce(&T, &T) -> Option<String>,
) where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let id = app_id().with(("watch", key));
    let old: Option<T> = ctx.data(|d| d.get_temp(id));
    if continuous(ctx, key) {
        if let Some(old) = old.as_ref().filter(|o| **o != state) {
            if let Some(w) = words(old, &state) {
                notice(ctx, w);
            }
        }
    }
    if old.as_ref() != Some(&state) {
        ctx.data_mut(|d| d.insert_temp(id, state));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeat_is_one_notice_inside_the_gap_and_new_ones_wait_in_order() {
        let ctx = egui::Context::default();
        assert!(notice_at(&ctx, "Keyboard connected", 10.0));
        assert!(!notice_at(&ctx, "Keyboard connected", 10.1));
        // A failure and then a happier notice: both wait, in order; neither is lost.
        assert!(!notice_at(&ctx, "Firmware update failed", 10.5));
        assert!(!notice_at(&ctx, "Keyboard connected again", 11.0));
        assert_eq!(said(&ctx, app_id()).as_deref(), Some("Keyboard connected"));
        flush(&ctx, 11.5);
        assert_eq!(said(&ctx, app_id()).as_deref(), Some("Keyboard connected"));
        flush(&ctx, 12.1);
        assert_eq!(
            said(&ctx, app_id()).as_deref(),
            Some("Firmware update failed")
        );
        flush(&ctx, 14.2);
        assert_eq!(
            said(&ctx, app_id()).as_deref(),
            Some("Keyboard connected again")
        );
    }

    #[test]
    fn the_same_words_after_the_gap_are_said_again() {
        let ctx = egui::Context::default();
        assert!(notice_at(&ctx, "Profile saved: a.foxprofile", 10.0));
        assert!(!notice_at(&ctx, "Profile saved: a.foxprofile", 11.0));
        // After the gap: said again, through a blank frame so a reader sees a change.
        assert!(notice_at(&ctx, "Profile saved: a.foxprofile", 20.0));
        assert_eq!(said(&ctx, app_id()).as_deref(), Some(""));
        assert_eq!(
            mem(&ctx).repeat.as_deref(),
            Some("Profile saved: a.foxprofile")
        );
    }

    #[test]
    fn the_region_keeps_its_node_when_the_widgets_before_it_change() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let id = egui::Id::new("card");
        say(&ctx, id, "Cool at 45 degrees Celsius");
        let node_with = |before: usize| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                for n in 0..before {
                    ui.label(format!("pill {n}"));
                }
                region(ui, id);
            });
            out.textures_delta.clear();
            let tree = out.platform_output.accesskit_update.expect("a tree");
            let live: Vec<_> = tree
                .nodes
                .iter()
                .filter(|(_, n)| n.label() == Some("Cool at 45 degrees Celsius"))
                .map(|(k, n)| (*k, n.live()))
                .collect();
            assert_eq!(live.len(), 1, "{live:?}");
            assert_eq!(live[0].1, Some(Live::Polite));
            live[0].0
        };
        let a = node_with(0);
        assert_eq!(node_with(3), a);
        assert_eq!(node_with(1), a);
    }

    #[test]
    fn long_text_is_cut_at_a_word_and_says_so() {
        assert_eq!(brief("Can't read it"), "Can't read it");
        let long = "word ".repeat(60);
        let b = brief(&long);
        assert!(b.chars().count() < BRIEF_MAX + 30, "{b}");
        assert!(b.ends_with("… Details on the screen."), "{b}");
        assert!(!b.contains("wor…"), "cut at a word: {b}");
        assert_eq!(
            brief("First line\nsecond line"),
            "First line… Details on the screen."
        );
    }

    #[test]
    fn a_watched_value_is_silent_when_first_seen_and_said_on_change() {
        let ctx = egui::Context::default();
        let words = |_: &bool, n: &bool| Some(if *n { "On" } else { "Off" }.to_string());
        watch(&ctx, "x", false, words);
        assert_eq!(said(&ctx, app_id()), None);
        watch(&ctx, "x", false, words);
        assert_eq!(said(&ctx, app_id()), None);
        watch(&ctx, "x", true, words);
        assert_eq!(said(&ctx, app_id()).as_deref(), Some("On"));
    }
}
