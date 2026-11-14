// SPDX-License-Identifier: MIT
//! The permission list a user is shown and what Allow, deny, revoke and the program binding do
//! with it (#1178, docs/hideout-permission-dialog-ux.md, contract §6.4).
//!
//! A grant is for the exact list that was shown: [`Listing::code`] is the SHA-256 of the list
//! (the module, its version, the hub's copy of the manifest, the program and every item with its
//! state), and [`Registry::approve_listed`] recomputes it and refuses unless it is the one the
//! user saw (`permission.list_changed`). Nothing here grants what is merely pending.
//!
//! Only the hub's own dialog and the control pipe's handler call `approve_listed`; no router
//! method does (a test in `hideout-router` keeps it so).

use hideout_contract::capability::{check_grantable, Capability};
use hideout_contract::{Error, ErrorId};
use serde::Serialize;

use crate::inventory::{Bound, ADOPTED};
use crate::keyboard::{canonical_path, VoidReason};
use crate::{adopted_exe, Failure, Registry};

/// The list format the code is computed over.
pub const LIST_FORMAT: &str = "hideout-permission-list-v1";

/// How an item stands in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemState {
    /// Can be allowed.
    Asked,
    /// On the never-list for this holder (§6.6) or not a permission this hub knows.
    Never,
    /// A `provide:` another adopted module already holds.
    Overlap,
}

/// Where an item is drawn (dialog order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Sensitive,
    MakeChanges,
    AnswerQuestions,
    ShareStatus,
    CantBeAllowed,
}

impl Group {
    /// The heading in the dialog (sentence case).
    pub fn heading(self) -> &'static str {
        match self {
            Group::Sensitive => "Sensitive",
            Group::MakeChanges => "Make changes when an app asks",
            Group::AnswerQuestions => "Answer questions from apps",
            Group::ShareStatus => "Share its status",
            Group::CantBeAllowed => "Can't be allowed",
        }
    }
}

/// One line of the list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Item {
    /// The canonical permission string.
    pub raw: String,
    pub state: ItemState,
    pub group: Group,
    /// The plain line.
    pub line: String,
    /// Why it can't be allowed, for `Never` and `Overlap`.
    pub reason: Option<&'static str>,
    /// Needs the extra "I understand" in the dialog.
    pub sensitive: bool,
}

/// The reason lines under "Can't be allowed".
pub const REASON_NEVER: &str = "Hideout never allows this for this module.";
pub const REASON_OVERLAP: &str = "Another program already answers these.";

/// The plain line for `provide:<method>`.
const PROVIDE_LINES: &[(&str, &str)] = &[
    ("health.read", "The keyboard's health report"),
    ("lights.page", "What the Lights page shows"),
    ("scenes.page", "Which scenes and wallpaper looks are listed"),
    ("overlay.window", "Where the layer overlay window is"),
    ("overlay.shown", "Whether the layer overlay is on"),
    ("tray.panel", "Whether the tray panel is open"),
    ("pc_lighting.list", "Your PC's lighting zones"),
    ("overlay.restore", "Bring the layer overlay back on screen"),
    ("overlay.set_shown", "Turn the layer overlay on or off"),
    ("tray.set_panel", "Open or close the tray panel"),
    ("pc_lighting.set_zone", "Change a PC lighting zone's colour"),
];

/// Other permission strings.
const OTHER_LINES: &[(&str, &str)] = &[
    (
        "facts.write:keyboard.*",
        "Share your keyboard's status in Hideout",
    ),
    (
        "facts.read:module.*",
        "See which modules are installed and running",
    ),
    ("key.events", "See every key you press"),
    ("hid.raw", "Talk to your devices directly"),
    ("net", "Use the network"),
    ("hub.notify", "Show notifications"),
    ("ui.window", "Open a window"),
    ("lighting.lease", "Take over lighting"),
];

/// Whether a `provide:` method name changes something rather than answering.
fn changes(method: &str) -> bool {
    let last = method.rsplit('.').next().unwrap_or(method);
    [
        "set", "restore", "toggle", "apply", "start", "stop", "write", "create", "delete",
    ]
    .iter()
    .any(|v| last == *v || last.starts_with(&format!("{v}_")))
}

fn is_sensitive(raw: &str) -> bool {
    raw == "net"
        || raw.starts_with("net.")
        || raw == "key.events"
        || raw == "hid.raw"
        || raw.starts_with("ai.ask:")
        || raw.starts_with("secrets.ref:")
        || raw.starts_with("device.hid:")
}

/// Group, plain line and sensitivity of one permission string. An unknown body falls back to
/// the raw string in the Sensitive group (§6.2's "Unknown: <raw>").
pub fn describe(raw: &str) -> (Group, String, bool) {
    if let Some(m) = raw.strip_prefix("provide:") {
        let group = if changes(m) {
            Group::MakeChanges
        } else {
            Group::AnswerQuestions
        };
        let line = PROVIDE_LINES
            .iter()
            .find(|(k, _)| *k == m)
            .map_or_else(|| format!("Answer {m}"), |(_, l)| (*l).to_string());
        return (group, line, false);
    }
    if let Some((_, l)) = OTHER_LINES.iter().find(|(k, _)| *k == raw) {
        let group = if raw.starts_with("facts.") {
            Group::ShareStatus
        } else if is_sensitive(raw) {
            Group::Sensitive
        } else {
            Group::MakeChanges
        };
        return (group, (*l).to_string(), is_sensitive(raw));
    }
    if raw.starts_with("facts.") {
        return (Group::ShareStatus, raw.to_string(), false);
    }
    if is_sensitive(raw) || Capability::parse(raw).is_err() {
        return (Group::Sensitive, format!("Unknown: {raw}"), true);
    }
    (Group::MakeChanges, raw.to_string(), false)
}

/// The list as it is now: what the dialog draws and the CLI prints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Listing {
    pub id: String,
    pub name: String,
    pub version: String,
    pub manifest_sha256: String,
    /// Canonical path of `run[0]` (adopted modules), else empty.
    pub exe_path: String,
    pub exe_sha256: String,
    /// `run[1..]` of an adopted module's manifest, else empty.
    pub run_args: Vec<String>,
    /// Items in dialog order (group, then raw).
    pub items: Vec<Item>,
    /// 64 hex digits.
    pub list_hash: String,
}

impl Listing {
    /// The 16 hex digits the dialog and the CLI show.
    pub fn code(&self) -> &str {
        &self.list_hash[..16]
    }

    /// The items that can be allowed.
    pub fn grantable(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| i.state == ItemState::Asked)
    }

    /// Whether `code` (16 digits or the full 64, any case) names this list.
    pub fn matches(&self, code: &str) -> bool {
        let c = code.trim().to_ascii_lowercase();
        (c.len() == 16 || c.len() == 64) && self.list_hash.starts_with(&c)
    }
}

/// A module's permissions as the Modules page reads them (#1178).
#[derive(Clone, Debug)]
pub struct View {
    /// Pending and declined items, with the list code.
    pub listing: Listing,
    /// Allowed now.
    pub granted: Vec<Item>,
    pub declined: Vec<String>,
    pub pending: Vec<String>,
    pub last_choice: Vec<String>,
    pub bound: Option<Bound>,
    /// The program changed or moved since the allow (or the binding was already voided).
    pub void: Option<VoidReason>,
    pub adopted: bool,
}

/// What an allow or deny did.
#[derive(Debug, Default)]
pub struct Outcome {
    pub granted: Vec<String>,
    pub declined: Vec<String>,
    /// Refused at grant time (never-list, overlap), with the error.
    pub refused: Vec<(String, Error)>,
    /// The program the grant is bound to.
    pub bound_path: String,
}

fn list_changed(shown: &str, now: &str) -> Failure {
    Failure::Check(Error::new(
        ErrorId::PermissionListChanged,
        format!("the list changed since it was shown (shown {shown}, now {now})"),
    ))
}

fn io_invalid(msg: &str) -> Failure {
    Failure::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, msg))
}

impl Registry {
    /// The program of a module now: canonical path and SHA-256 of the file for an adopted one.
    /// A module the hub installed itself has no external program: its path is empty and its
    /// hash is the hash of the signed file map.
    /// `run[1..]` of an adopted module now; empty for any other (or an unreadable manifest).
    fn run_args(&self, id: &str) -> Vec<String> {
        let adopted = self
            .inventory
            .modules
            .get(id)
            .is_some_and(|m| m.source == ADOPTED);
        if !adopted {
            return Vec::new();
        }
        self.manifest_checked(id)
            .ok()
            .and_then(|m| m.run)
            .map(|r| r.into_iter().skip(1).collect())
            .unwrap_or_default()
    }

    /// The program of `id` now (canonical path, SHA-256), as the binding and the updater's
    /// hash check read it.
    pub fn current_program(&self, id: &str) -> Result<(String, String), Failure> {
        self.program(id)
    }

    fn program(&self, id: &str) -> Result<(String, String), Failure> {
        let m = self
            .inventory
            .modules
            .get(id)
            .ok_or_else(|| Failure::NotInstalled(id.into()))?;
        if m.source == ADOPTED {
            let exe = adopted_exe(&self.manifest_checked(id)?)?;
            let path = canonical_path(&exe).map_err(|_| Failure::Io(crate::not_found(&exe)))?;
            let bytes = std::fs::read(&path)?;
            Ok((path, foxglove_plugin_trust::sha256_hex(&bytes)))
        } else {
            let text = serde_json::to_string(&m.files).map_err(std::io::Error::other)?;
            Ok((
                String::new(),
                foxglove_plugin_trust::sha256_hex(text.as_bytes()),
            ))
        }
    }

    /// The permission list of `id` as it is now (asked and not allowed: pending and declined).
    pub fn permission_list(&self, id: &str) -> Result<Listing, Failure> {
        let m = self
            .inventory
            .modules
            .get(id)
            .ok_or_else(|| Failure::NotInstalled(id.into()))?;
        let manifest = self.manifest_checked(id)?;
        let manifest_text = std::fs::read(self.module_dir(id, &m.version).join("module.toml"))?;
        let (exe_path, exe_sha256) = self.program(id)?;
        let run_args = self.run_args(id);
        let overlap = |cap: &Capability| -> bool {
            let Capability::Provide(p) = cap else {
                return false;
            };
            m.source == ADOPTED
                && self
                    .inventory
                    .modules
                    .iter()
                    .filter(|(k, o)| k.as_str() != id && o.source == ADOPTED)
                    .flat_map(|(_, o)| o.granted.iter())
                    .filter_map(|g| g.strip_prefix("provide:"))
                    .any(|o| crate::inventory::provide_overlap(o, p))
        };
        let holder = m.holder();
        let mut raws: Vec<String> = m
            .pending_permissions
            .iter()
            .chain(m.declined_permissions.iter())
            .cloned()
            .collect();
        raws.sort();
        raws.dedup();
        let mut items: Vec<Item> = raws
            .into_iter()
            .map(|raw| {
                let (group, line, sensitive) = describe(&raw);
                let (state, reason) = match Capability::parse(&raw) {
                    Err(_) => (ItemState::Never, Some(REASON_NEVER)),
                    Ok(c) if overlap(&c) => (ItemState::Overlap, Some(REASON_OVERLAP)),
                    Ok(c) => match check_grantable(&c, holder) {
                        Ok(()) => (ItemState::Asked, None),
                        Err(_) => (ItemState::Never, Some(REASON_NEVER)),
                    },
                };
                let group = if state == ItemState::Asked {
                    group
                } else {
                    Group::CantBeAllowed
                };
                Item {
                    raw,
                    state,
                    group,
                    line,
                    reason,
                    sensitive: sensitive && state == ItemState::Asked,
                }
            })
            .collect();
        items.sort_by(|a, b| (a.group, &a.raw).cmp(&(b.group, &b.raw)));
        let manifest_sha256 = foxglove_plugin_trust::sha256_hex(&manifest_text);
        let mut canon: Vec<(&str, ItemState)> =
            items.iter().map(|i| (i.raw.as_str(), i.state)).collect();
        canon.sort_by(|a, b| a.0.cmp(b.0));
        let doc = serde_json::json!({
            "format": LIST_FORMAT,
            "id": id,
            "version": m.version,
            "manifest_sha256": manifest_sha256,
            "exe_path": exe_path.to_lowercase(),
            "exe_sha256": exe_sha256,
            "items": canon
                .iter()
                .map(|(r, s)| serde_json::json!({"raw": r, "state": s}))
                .collect::<Vec<_>>(),
        });
        let list_hash = foxglove_plugin_trust::sha256_hex(doc.to_string().as_bytes());
        Ok(Listing {
            id: id.into(),
            name: manifest.name,
            version: m.version.clone(),
            manifest_sha256,
            exe_path,
            exe_sha256,
            run_args,
            items,
            list_hash,
        })
    }

    /// What the Modules page draws for `id` (#1178): the list as the dialog shows it, the items
    /// now allowed (with their plain lines), what was declined, the dialog's defaults after a
    /// void, and where the binding stands. Read-only.
    pub fn permission_view(&self, id: &str) -> Result<View, Failure> {
        let listing = self.permission_list(id)?;
        let m = &self.inventory.modules[id];
        let mut granted: Vec<Item> = m
            .granted
            .iter()
            .map(|raw| {
                let (group, line, sensitive) = describe(raw);
                Item {
                    raw: raw.clone(),
                    state: ItemState::Asked,
                    group,
                    line,
                    reason: None,
                    sensitive,
                }
            })
            .collect();
        granted.sort_by(|a, b| (a.group, &a.raw).cmp(&(b.group, &b.raw)));
        Ok(View {
            listing,
            granted,
            declined: m.declined_permissions.clone(),
            pending: m.pending_permissions.clone(),
            last_choice: m.last_choice.clone(),
            bound: m.bound.clone(),
            void: self.binding_void(id),
            adopted: m.source == ADOPTED,
        })
    }

    /// The user's Allow (§6.4): grants exactly `chosen` out of the list whose code is
    /// `list_code`, records `declined` as declined, and binds the grant to the manifest and the
    /// program. The inventory is read again from disk and the list recomputed first; if its code
    /// is not `list_code` nothing is written (`permission.list_changed`). `chosen` and
    /// `declined` must be items of the shown list and disjoint; items in neither stay pending.
    /// Items that can't be allowed (never-list, overlap) are recorded as declined and returned
    /// as refused.
    ///
    /// Called only from the hub's dialog and the control pipe's handler; `by` is `dialog` or
    /// `cli`.
    pub fn approve_listed(
        &mut self,
        id: &str,
        list_code: &str,
        chosen: &[String],
        declined: &[String],
        now: &str,
        by: &str,
    ) -> Result<Outcome, Failure> {
        self.reload_inventory()?;
        let list = self.permission_list(id)?;
        if !list.matches(list_code) {
            return Err(list_changed(list_code, list.code()));
        }
        let shown = |r: &String| list.items.iter().any(|i| &i.raw == r);
        if chosen.iter().chain(declined).any(|r| !shown(r)) {
            return Err(io_invalid("an item is not in the list that was shown"));
        }
        if chosen.iter().any(|c| declined.contains(c)) {
            return Err(io_invalid("an item is both allowed and declined"));
        }
        let asked: Vec<Capability> = chosen
            .iter()
            .map(|p| Capability::parse(p))
            .collect::<Result<_, _>>()?;
        let before = self.inventory.modules[id].granted.clone();
        let refused = self.inventory.grant(id, &asked).unwrap_or_default();
        let m = self.inventory.modules.get_mut(id).expect("listed");
        let got: Vec<String> = m
            .granted
            .iter()
            .filter(|g| !before.contains(g))
            .cloned()
            .collect();
        let refused_raw: Vec<String> = chosen
            .iter()
            .filter(|c| !m.granted.contains(c))
            .cloned()
            .collect();
        let mut dec: Vec<String> = declined.to_vec();
        dec.extend(refused_raw.iter().cloned());
        // Items that can never be allowed leave the pending list too: they are shown once.
        dec.extend(
            list.items
                .iter()
                .filter(|i| i.state != ItemState::Asked)
                .map(|i| i.raw.clone()),
        );
        m.pending_permissions.retain(|p| !dec.contains(p));
        m.declined_permissions.extend(dec);
        let granted_now = m.granted.clone();
        m.declined_permissions.retain(|d| !granted_now.contains(d));
        m.declined_permissions.sort();
        m.declined_permissions.dedup();
        m.last_choice.clear();
        m.bound = Some(Bound {
            manifest_sha256: list.manifest_sha256.clone(),
            exe_path: list.exe_path.clone(),
            exe_sha256: list.exe_sha256.clone(),
            list_hash: list.list_hash.clone(),
            at: now.into(),
            by: by.into(),
            run_args: list.run_args.clone(),
            void: false,
            attested: m.bound.take().map(|b| b.attested).unwrap_or_default(),
        });
        self.save_inventory()?;
        let refused: Vec<(String, Error)> = refused_raw.into_iter().zip(refused).collect();
        Ok(Outcome {
            granted: got,
            declined: declined.to_vec(),
            refused,
            bound_path: list.exe_path,
        })
    }

    /// Records `declined` as declined (`deny`) against the list whose code is `list_code`;
    /// nothing is granted.
    pub fn deny_listed(
        &mut self,
        id: &str,
        list_code: &str,
        declined: &[String],
        now: &str,
        by: &str,
    ) -> Result<Outcome, Failure> {
        self.approve_listed(id, list_code, &[], declined, now, by)
    }

    /// Narrows: `items` (or every grant for `None`) stop being granted and are recorded as
    /// declined. Never widens, needs no list code. Returns what remains granted. The caller (the
    /// hub) drops the router's grant first (§6.4).
    pub fn revoke_items(
        &mut self,
        id: &str,
        items: Option<&[String]>,
    ) -> Result<Vec<String>, Failure> {
        self.reload_inventory()?;
        let m = self
            .inventory
            .modules
            .get_mut(id)
            .ok_or_else(|| Failure::NotInstalled(id.into()))?;
        if let Some(wanted) = items {
            if let Some(x) = wanted.iter().find(|w| !m.granted.contains(w)) {
                return Err(io_invalid(&format!("{x} is not allowed")));
            }
        }
        let gone: Vec<String> = m
            .granted
            .iter()
            .filter(|g| items.is_none_or(|w| w.contains(g)))
            .cloned()
            .collect();
        m.granted.retain(|g| !gone.contains(g));
        m.declined_permissions.extend(gone);
        m.declined_permissions.sort();
        m.declined_permissions.dedup();
        let left = m.granted.clone();
        self.save_inventory()?;
        Ok(left)
    }

    /// Whether a grant of `id` is bound and the program no longer matches (or was already
    /// voided). Read-only; [`Registry::void_if_changed`] writes.
    pub fn binding_void(&self, id: &str) -> Option<VoidReason> {
        let m = self.inventory.modules.get(id)?;
        let b = m.bound.as_ref()?;
        if b.void {
            return Some(VoidReason::Changed);
        }
        if m.source != ADOPTED || b.exe_sha256.is_empty() {
            return None;
        }
        match self.program(id) {
            Ok((path, hash)) => {
                if !b.run_args.is_empty() && self.run_args(id) != b.run_args {
                    Some(VoidReason::Changed)
                } else if !crate::keyboard::same_path(&path, &b.exe_path) {
                    Some(VoidReason::Moved)
                } else if !hash.eq_ignore_ascii_case(&b.exe_sha256) {
                    Some(VoidReason::Changed)
                } else {
                    None
                }
            }
            // A program that is not there can't be the one that was allowed.
            Err(_) => Some(VoidReason::Moved),
        }
    }

    /// The check at every turn on and launch: if the program is not the one the grant is bound
    /// to, the grant is **void**: `granted` goes back to pending (the choices kept as
    /// `last_choice`), the binding's hash is cleared (the old file does not revive it) and the
    /// keyboard record goes void. Returns the reason when it voided now.
    pub fn void_if_changed(&mut self, id: &str, now: &str) -> Result<Option<VoidReason>, Failure> {
        let Some(reason) = self.binding_void(id) else {
            return Ok(None);
        };
        if self.inventory.modules[id]
            .bound
            .as_ref()
            .is_some_and(|b| b.void)
        {
            return Ok(None);
        }
        let path = self.home.join("inventory.json");
        let inv = crate::inventory::Inventory::update(&path, |inv| {
            if let Some(m) = inv.modules.get_mut(id) {
                let old = std::mem::take(&mut m.granted);
                m.pending_permissions.extend(old.iter().cloned());
                m.pending_permissions.sort();
                m.pending_permissions.dedup();
                m.last_choice = old;
                if let Some(b) = m.bound.as_mut() {
                    b.void = true;
                    b.exe_sha256.clear();
                    b.at = now.into();
                }
            }
        })?;
        if let Some(m) = inv.modules.get(id) {
            self.inventory.modules.insert(id.into(), m.clone());
        }
        let _ = self.void_keyboard(id, reason, now);
        Ok(Some(reason))
    }

    /// The install or update path vouches that the program file now at the module's `run[0]` is
    /// the next version of the one the grant is bound to (#1178). Hub-side only: no router
    /// method or module request reaches it. The caller (the updater or installer) has verified
    /// the signed `latest.json` / package for `version` before calling.
    ///
    /// It keeps the grant (no new Allow) only when the grant is bound and not void, and the new
    /// file is in the **same install folder** as the bound one. The old->new hashes are
    /// recorded in `bound.attested`. Anything else (void, another folder, no binding) is an
    /// error and changes nothing; the unattested change then voids at the next check.
    pub fn attest_update(
        &mut self,
        id: &str,
        version: &str,
        by: &str,
        now: &str,
    ) -> Result<(), Failure> {
        self.reload_inventory()?;
        let m = self
            .inventory
            .modules
            .get(id)
            .ok_or_else(|| Failure::NotInstalled(id.into()))?;
        let b = m
            .bound
            .as_ref()
            .filter(|b| !b.void && !b.exe_sha256.is_empty())
            .ok_or_else(|| io_invalid("nothing is bound to attest"))?;
        let (path, hash) = self.program(id)?;
        let folder = |p: &str| {
            std::path::Path::new(&crate::keyboard::strip_verbatim(p).replace('/', "\\"))
                .parent()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        if !crate::keyboard::same_path(&folder(&path), &folder(&b.exe_path)) {
            return Err(io_invalid(
                "the program is in another folder than the one allowed",
            ));
        }
        if hash.eq_ignore_ascii_case(&b.exe_sha256)
            && crate::keyboard::same_path(&path, &b.exe_path)
        {
            return Ok(());
        }
        let rec = crate::inventory::Attestation {
            from_sha256: b.exe_sha256.clone(),
            to_sha256: hash.clone(),
            from_path: b.exe_path.clone(),
            to_path: path.clone(),
            version: version.into(),
            at: now.into(),
            by: by.into(),
        };
        let inv = crate::inventory::Inventory::update(&self.home.join("inventory.json"), |inv| {
            if let Some(b) = inv.modules.get_mut(id).and_then(|m| m.bound.as_mut()) {
                b.exe_path = path.clone();
                b.exe_sha256 = hash.clone();
                b.attested.push(rec.clone());
                let n = b.attested.len();
                if n > 20 {
                    b.attested.drain(..n - 20);
                }
            }
        })?;
        if let Some(m) = inv.modules.get(id) {
            self.inventory.modules.insert(id.into(), m.clone());
        }
        Ok(())
    }
}
