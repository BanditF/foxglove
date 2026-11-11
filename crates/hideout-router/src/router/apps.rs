// SPDX-License-Identifier: MIT
//! The app watch's call (contract §8.12, v1.13, #1108): `apps.watch {exe}`, Foxglove API 1.3's
//! wire, answered by the router on top of the app watch's facts (§4.6). The reply is
//! `{exe, open}` with `open` 1 while `apps.running.<name>` is true and 0 otherwise; each flip of
//! that fact sends `app.window.appeared` or `app.window.closed` to the connections that watch
//! the app, nobody else. The permission is the facts' own, `apps.watch:<name>.exe`. Nothing
//! here waits or runs on a timer: the events ride the fact's publish ([`Router::publish`]),
//! which the app watch sends from process and window events. A connection's watches end with
//! it ([`Router::disconnect`]).

use hideout_contract::module::app_label;
use hideout_contract::services::{
    error_id, AppWindows, AppsWatch, APP_WINDOW_APPEARED, APP_WINDOW_CLOSED,
};
use serde_json::{json, Value};

use super::services::{refusal, ServiceError};
use super::{code, ok, ConnId, Out, Router, Who};
use crate::checks::{watches_app, Caps};

/// The app fact `apps.watch` follows.
const RUNNING: &str = "apps.running.";

/// The app label of an `exe` a module may watch (`obs64.exe` → `obs64`): a file name, not a
/// path, ending in `.exe`, as the permission spells it (§6.1).
fn label_of(exe: &str) -> Option<String> {
    let name = exe.strip_suffix(".exe")?;
    if name.is_empty() || name.contains(['\\', '/', ':']) {
        return None;
    }
    app_label(exe)
}

impl Router {
    /// `apps.watch {exe}` (§8.12): checks the caller and its grant, starts the watch and
    /// answers `{exe, open}`.
    pub(super) fn apps_watch(
        &mut self,
        conn: ConnId,
        who: &Who,
        caps: &Caps,
        id: &Value,
        params: Value,
    ) -> Value {
        let shown = match who {
            Who::Module { id, .. } => self
                .modules
                .get(id)
                .map_or_else(|| id.clone(), |m| m.name.clone()),
            Who::Tool { name, .. } => name.clone(),
            Who::New => String::new(),
        };
        if !matches!(who, Who::Module { .. }) {
            // The hub watches only the apps manifests name (§4.6), so a tool's watch could
            // never hear anything. Foxglove's API 1.3 serves plugins only, too.
            return refusal(
                id,
                &ServiceError::new(
                    code::DENIED,
                    error_id::APPS_DENIED,
                    format!("{shown} isn't a module: only modules watch apps"),
                ),
            );
        }
        let p: AppsWatch = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return refusal(
                    id,
                    &ServiceError::argument(error_id::APPS_ARGUMENT, format!("apps.watch: {e}")),
                )
            }
        };
        let exe = p.exe.trim().to_ascii_lowercase();
        let Some(label) = label_of(&exe) else {
            return refusal(
                id,
                &ServiceError::argument(
                    error_id::APPS_ARGUMENT,
                    format!("apps.watch: {exe:?} is not a program's file name such as obs64.exe"),
                ),
            );
        };
        let key = format!("{RUNNING}{label}");
        if !watches_app(&caps.kept(), &key) {
            return refusal(
                id,
                &ServiceError::new(
                    code::DENIED,
                    error_id::APPS_DENIED,
                    format!(
                        "{shown} needs permission apps.watch:{exe} to know when {exe} opens. Allow it in Hideout's permissions for {shown}."
                    ),
                ),
            );
        }
        self.app_watches
            .entry(conn)
            .or_default()
            .insert(label, exe.clone());
        let open = u32::from(self.facts.get(&key) == Some(&json!(true)));
        ok(
            id,
            serde_json::to_value(AppWindows { exe, open }).expect("serializable"),
        )
    }

    /// `apps.running.<name>` changed (`was_open` before, `value` now): the connections that
    /// watch that app hear `app.window.appeared` (`open` 1) or `app.window.closed` (`open` 0).
    /// Unset to `false` and `false` to unset say nothing: the app didn't close.
    pub(super) fn app_window_events(
        &self,
        key: &str,
        was_open: bool,
        value: &Value,
        out: &mut Vec<Out>,
    ) {
        let Some(label) = key.strip_prefix(RUNNING) else {
            return;
        };
        let open = value == &json!(true);
        if open == was_open {
            return;
        }
        let method = if open {
            APP_WINDOW_APPEARED
        } else {
            APP_WINDOW_CLOSED
        };
        for (conn, apps) in &self.app_watches {
            if let Some(exe) = apps.get(label) {
                let params = AppWindows {
                    exe: exe.clone(),
                    open: u32::from(open),
                };
                out.push(Out::Send(
                    *conn,
                    json!({"jsonrpc": "2.0", "method": method, "params": params}),
                ));
            }
        }
    }

    /// How many connections watch the app labelled `label` (`obs64`) with `apps.watch`.
    pub fn app_watchers(&self, label: &str) -> usize {
        self.app_watches
            .values()
            .filter(|a| a.contains_key(label))
            .count()
    }

    /// Whether `key` is an app fact `apps.watch` follows and is `true` now.
    pub(super) fn app_open_now(&self, key: &str) -> bool {
        key.starts_with(RUNNING) && self.facts.get(key) == Some(&json!(true))
    }
}

#[cfg(test)]
mod tests {
    use super::label_of;

    #[test]
    fn only_a_program_file_name_is_watched() {
        assert_eq!(label_of("obs64.exe").as_deref(), Some("obs64"));
        assert_eq!(label_of("my tool.exe").as_deref(), Some("my-tool"));
        assert_eq!(label_of("obs64"), None);
        assert_eq!(label_of(".exe"), None);
        assert_eq!(label_of(r"c:\obs\obs64.exe"), None);
        assert_eq!(label_of("a/obs64.exe"), None);
    }
}
