//! One Foxglove per user (#345): the shared guard (`hub_shell::single_instance`) under
//! Foxglove's name. A run with `FOXGLOVE_CONFIG_DIR` (tests, the Test lab) gets names of its own
//! for that folder, so it never hands off to, or blocks, the user's own Foxglove.

const APP: &str = "Foxglove";

fn log(line: &str) {
    crate::foxglove::app_log::early(line);
}

/// Takes the guard at startup. False when another Foxglove already holds it; that one has been
/// asked to show itself, and this process should exit 0.
pub fn claim() -> bool {
    use hub_shell::single_instance::claim;
    match super::store::test_sandbox() {
        // A test build gets names of its own folder, never the shared ones.
        Some(dir) => claim(APP, Some(dir.as_os_str()), log),
        None => claim(APP, std::env::var_os("FOXGLOVE_CONFIG_DIR").as_deref(), log),
    }
}

/// Lets go of the guard just before this process hands over to a fresh copy (#346).
pub fn release() {
    hub_shell::single_instance::release()
}

/// Calls `show` whenever a second launch asks this instance to show itself.
pub fn on_show(show: impl Fn() + Send + 'static) {
    hub_shell::single_instance::on_show(show)
}
