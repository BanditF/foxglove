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
//! Both are kept quiet: the same words twice in a row are said once, and two different notices
//! are at least [`GAP_S`] apart (the later one waits and is said when the time is up; if several
//! wait, only the newest is said, as the older ones are already stale).

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
    let (_, r) = ui.allocate_exact_size(egui::Vec2::ZERO, egui::Sense::hover());
    let node = r.id;
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text.clone()));
    // The name is the words (a plain label node keeps its text as a value, which a live region
    // may not speak), as the hub's crash bar does.
    ui.ctx().accesskit_node_builder(node, |n| {
        n.set_live(Live::Polite);
        n.set_label(text.clone());
    });
}

/// The least time between two notices, in seconds.
pub const GAP_S: f64 = 2.0;

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
    /// Words held back by the gap.
    pending: Option<String>,
}

/// Says `words` through the window's region at time `now`. Returns whether they were said now
/// (not a repeat, not held back).
pub fn notice_at(ctx: &egui::Context, words: impl Into<String>, now: f64) -> bool {
    let words: String = words.into();
    let key = app_id().with("mem");
    let mut m: Mem = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
    let said = if m.last.as_deref() == Some(words.as_str()) {
        // A repeat of what was just said: nothing, and nothing waits for it.
        m.pending = None;
        false
    } else if m.last.is_some() && now - m.at < GAP_S {
        m.pending = Some(words);
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(GAP_S - (now - m.at)));
        false
    } else {
        say(ctx, app_id(), words.clone());
        m.last = Some(words);
        m.at = now;
        m.pending = None;
        true
    };
    ctx.data_mut(|d| d.insert_temp(key, m));
    said
}

/// [`notice_at`] at the context's current time.
pub fn notice(ctx: &egui::Context, words: impl Into<String>) -> bool {
    let now = ctx.input(|i| i.time);
    notice_at(ctx, words, now)
}

/// Says the notice held back by the gap once its time is up.
fn flush(ctx: &egui::Context, now: f64) {
    let key = app_id().with("mem");
    let Some(m) = ctx.data(|d| d.get_temp::<Mem>(key)) else {
        return;
    };
    if let Some(words) = m.pending.clone() {
        if now - m.at >= GAP_S {
            notice_at(ctx, words, now);
        }
    }
}

/// Draws the window's live region (the shell calls this once per frame) and says a held notice
/// whose time has come.
pub fn app(ui: &mut egui::Ui) {
    let now = ui.ctx().input(|i| i.time);
    flush(ui.ctx(), now);
    region(ui, app_id());
}

/// Watches `state` (named `key`, one per thing watched): the first time it is seen nothing is
/// said (opening a page is not news); after that, `words(old, new)` is said through [`notice`]
/// whenever it changes and returns words.
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
    if let Some(old) = old.as_ref().filter(|o| **o != state) {
        if let Some(w) = words(old, &state) {
            notice(ctx, w);
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
    fn a_repeat_is_said_once_and_a_quick_change_waits_its_turn() {
        let ctx = egui::Context::default();
        assert!(notice_at(&ctx, "Keyboard connected", 10.0));
        assert!(!notice_at(&ctx, "Keyboard connected", 10.1));
        assert!(!notice_at(&ctx, "Keyboard disconnected", 10.5));
        assert_eq!(said(&ctx, app_id()).as_deref(), Some("Keyboard connected"));
        // Newer words replace the held ones.
        assert!(!notice_at(&ctx, "Update available", 11.0));
        flush(&ctx, 11.5);
        assert_eq!(said(&ctx, app_id()).as_deref(), Some("Keyboard connected"));
        flush(&ctx, 12.1);
        assert_eq!(said(&ctx, app_id()).as_deref(), Some("Update available"));
        // After the gap a new notice is said at once.
        assert!(notice_at(&ctx, "Saved", 20.0));
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
