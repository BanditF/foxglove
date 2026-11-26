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
use hideout_contract::permission::{fact_words, Permission};
use hideout_contract::{Error, ErrorId};
use serde::Serialize;

use crate::inventory::{Bound, FollowFolder, RunOk, RunState, ADOPTED};
use crate::keyboard::{canonical_path, VoidReason};
use crate::one_run::OneRun;
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
pub const REASON_NEVER: &str = "Hideout never allows this for this app.";
pub const REASON_OVERLAP: &str = "Another app already answers these.";

/// The plain line for `provide:<method>`.
const PROVIDE_LINES: &[(&str, &str)] = &[
    ("health.read", "The keyboard's health report"),
    ("lights.page", "What the Lights page shows"),
    ("scenes.page", "Which scenes and wallpaper looks are listed"),
    ("scene.status", "Whether a lighting scene is running"),
    ("lights.status", "What your keyboard lights are showing"),
    ("scene.start", "Start a lighting scene on your keyboard"),
    ("overlay.window", "Where the layer overlay window is"),
    ("overlay.shown", "Whether the layer overlay is on"),
    ("tray.panel", "Whether the tray panel is open"),
    ("pc_lighting.list", "Your PC's lighting zones"),
    ("overlay.restore", "Bring the layer overlay back on screen"),
    ("overlay.set_shown", "Turn the layer overlay on or off"),
    ("tray.set_panel", "Open or close the tray panel"),
    ("pc_lighting.set_zone", "Change a PC lighting zone's colour"),
    // #1120: Wallpaper Engine's pause, for the programs you let pause it.
    (
        "wallpaper-engine.status",
        "Whether your live wallpaper is paused",
    ),
    ("wallpaper-engine.pause", "Pause your live wallpaper"),
    (
        "wallpaper-engine.release",
        "Let your live wallpaper play again",
    ),
];

/// #1134: the line for a `provide:gpu.*` (or any `provide:gpu.` name). The module that holds it
/// is the GPU manager: every app's AI request goes to it, so it is drawn as Sensitive and the
/// dialog adds the module's name and what it sees (`confirm_line`).
pub const GPU_BROKER_LINE: &str = "Act as the GPU manager for all apps";

/// Whether a `provide:` method name makes its holder the GPU manager.
pub fn is_gpu_broker(method: &str) -> bool {
    method == "gpu.*" || method.starts_with("gpu.")
}

/// Other permission strings.
const OTHER_LINES: &[(&str, &str)] = &[
    ("key.events", "See every key you press"),
    ("hid.raw", "Talk to your devices directly"),
    ("net", "Go online"),
    ("files", "Read and write your files"),
    ("device.hid:keyboard", "Talk to your keyboard (USB)"),
    ("device.hid:lighting", "Talk to your PC's lighting devices"),
    (
        "device.hid:vendor",
        "Talk to other devices from the same maker",
    ),
    ("hub.notify", "Show notifications"),
    ("ui.window", "Open a window"),
    ("lighting.lease", "Take over lighting"),
];

/// Whether a `provide:` method name changes something rather than answering.
fn changes(method: &str) -> bool {
    let last = method.rsplit('.').next().unwrap_or(method);
    [
        "set", "restore", "toggle", "apply", "start", "stop", "write", "create", "delete", "pause",
        "release",
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

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// Whether a grant waits for the user's OK again when a follow-folder grant meets new bytes
/// (#1465): every Sensitive item, plus `files`, `secrets`, every `channel.open:*` and every
/// offer (`channel.offer:*`, `provide:*`). The registry cannot know a channel's catalog risk, so
/// all channel opens are held (fail closed). Offering a channel or answering calls also waits
/// for the user's OK on changed bytes.
pub fn held_on_change(raw: &str) -> bool {
    describe(raw).2
        || raw == "files"
        || raw == "secrets"
        || raw.starts_with("channel.open:")
        || raw.starts_with("channel.offer:")
        || raw.starts_with("provide:")
}

/// Group, plain line and sensitivity of one permission string. An unknown body falls back to
/// the raw string in the Sensitive group (§6.2's "Unknown: <raw>").
pub fn describe(raw: &str) -> (Group, String, bool) {
    if let Some(m) = raw.strip_prefix("provide:") {
        if is_gpu_broker(m) {
            return (Group::Sensitive, GPU_BROKER_LINE.to_string(), true);
        }
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
        // Plain words for the pattern (display only; the grant is the raw string). A pattern
        // that does not parse is never grantable and keeps its raw text.
        let line = match Permission::try_from(raw.to_string()) {
            Ok(Permission::FactsRead(p)) => capitalized(&fact_words(true, &p)),
            Ok(Permission::FactsWrite(p)) => capitalized(&fact_words(false, &p)),
            _ => raw.to_string(),
        };
        return (Group::ShareStatus, line, false);
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
    /// The program runs from exactly the path the grant was bound to (same `run[1..]`), on this
    /// PC's own fixed drive. Only then may a re-ask show the earlier answer as pre-checked
    /// (display only). Moved, network, removable or unbound: false.
    pub same_place: bool,
    /// For an app with a one-run OK ([`View::one_run`]) nothing is bound: this is then that
    /// run's record in a binding's shape ([`crate::inventory::RunOk::as_bound`]), so the page
    /// can say when it was allowed. It keeps and follows nothing.
    pub bound: Option<Bound>,
    /// The program changed or moved since the allow (or the binding was already voided).
    pub void: Option<VoidReason>,
    pub adopted: bool,
    /// The app's program runs whatever it is handed (#1452, [`crate::one_run`]): an Allow holds
    /// for one run, nothing is remembered and "Keep my OK" is not offered.
    pub one_run: Option<OneRun>,
    /// A one-run app whose OK was used up by a start (or was a remembered OK from before the
    /// rule): the ask is the ordinary one for a start, not "the app changed".
    pub asks_again: bool,
    /// Whether a cloud service keeps the app's folder in step (adopted apps with a path; else
    /// `Local`). `Unknown` when it could not be read: nothing is kept for such a folder.
    pub folder_sync: crate::sync_root::SyncAnswer,
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
    /// That program's SHA-256 as the grant records it.
    pub bound_sha256: String,
    /// The OK was stored for one run only, and why (#1452); nothing was bound.
    pub one_run: Option<OneRun>,
}

fn list_changed(shown: &str, now: &str) -> Failure {
    Failure::Check(Error::new(
        ErrorId::PermissionListChanged,
        format!("the list changed since it was shown (shown {shown}, now {now})"),
    ))
}

/// The end of a one-run OK (#1452): everything allowed waits for the user again, the choice
/// kept for the next ask, and a dev-folder enrolment ends with it.
fn end_grants(m: &mut crate::inventory::Installed) {
    let old = std::mem::take(&mut m.granted);
    m.pending_permissions.extend(old.iter().cloned());
    m.pending_permissions.sort();
    m.pending_permissions.dedup();
    if !old.is_empty() {
        m.last_choice = old;
    }
    if m.identity
        .as_ref()
        .is_some_and(|i| matches!(i.class, crate::inventory::IdentityClass::DevFolder { .. }))
    {
        m.identity = None;
    }
}

pub(crate) fn io_invalid(msg: &str) -> Failure {
    Failure::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, msg))
}

/// What a folder step does to a module's grants (#1465): the held items go from `granted` to
/// `pending_permissions` and `last_choice`, and the keyboard record `voids` picks goes void
/// (reason `changed`). Nothing else changes.
/// What the binding check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    /// Nothing bound, or the program is the one the grant is bound to.
    Fine,
    /// Not the program that was allowed (or already void).
    Void(VoidReason),
    /// The app file is there but could not be read now.
    Unreadable,
}

/// What a check made at the launch, against the pinned program, knows about that start.
#[derive(Debug, Clone, Copy)]
struct Launch<'a> {
    /// The arguments the process is about to be started with; `None` reads the manifest's.
    args: Option<&'a [String]>,
    /// The start was counted as the one a one-run OK is for ([`Registry::start_counted`]).
    counted: bool,
}

fn hold_back(
    m: &mut crate::inventory::Installed,
    now: &str,
    voids: impl Fn(&crate::keyboard::KeyboardGrant) -> bool,
) {
    let moved: Vec<String> = m
        .granted
        .iter()
        .filter(|g| held_on_change(g))
        .cloned()
        .collect();
    m.granted.retain(|g| !moved.contains(g));
    m.pending_permissions.extend(moved.iter().cloned());
    m.pending_permissions.sort();
    m.pending_permissions.dedup();
    m.last_choice.extend(moved);
    m.last_choice.sort();
    m.last_choice.dedup();
    if let Some(k) = m.keyboard.as_mut().filter(|k| voids(k)) {
        k.state = crate::keyboard::GrantState::Void;
        k.sha256.clear();
        k.reason = Some(crate::keyboard::VoidReason::Changed);
        k.at = now.into();
    }
}

/// Whether the binding's newest step is a folder step that has not held anything back: one
/// recorded before #1465.
fn old_folder_step(m: &crate::inventory::Installed) -> bool {
    m.bound
        .as_ref()
        .and_then(|b| b.attested.last())
        .is_some_and(|a| a.by == FOLLOW_ATTESTED_BY && !a.held_back)
}

/// [`Registry::reask_old_folder_steps`] for one module; false when there is nothing to do.
fn reask_old_folder_step(m: &mut crate::inventory::Installed, now: &str) -> bool {
    if !old_folder_step(m) {
        return false;
    }
    // Any `on` keyboard record: the old step moved it to the changed bytes.
    hold_back(m, now, |k| k.state == crate::keyboard::GrantState::On);
    if let Some(a) = m.bound.as_mut().and_then(|b| b.attested.last_mut()) {
        a.held_back = true;
    }
    true
}

/// The only `by` that may set a follow-folder grant: the hub's own dialog.
pub const FOLLOW_SET_BY: &str = "dialog";
/// `attested[].by` of a program change kept by a follow-folder grant.
pub const FOLLOW_ATTESTED_BY: &str = "folder";

/// Whether a re-ask may show the earlier answer as pre-checked: the program is at exactly the
/// path the grant was bound to, and that path is on this PC's own fixed drive (never a network,
/// removable or other-host path). Display only; it never grants anything.
pub fn reask_same_place(current: &str, bound: &str) -> bool {
    !bound.is_empty()
        && crate::keyboard::same_path(current, bound)
        && on_local_fixed_drive(current)
        && on_local_fixed_drive(bound)
}

/// Whether a canonical path is on one of this PC's internal fixed drives (not USB, see [`crate::drive`]): a drive letter (not UNC, not a
/// `\\.\` device path) whose drive Windows reports as fixed (not network, removable, optical or
/// RAM disk). A mapped network drive canonicalizes to its UNC form and fails too.
pub fn on_local_fixed_drive(path: &str) -> bool {
    on_local_fixed_drive_with(path, &crate::drive::SystemDrives)
}

/// [`on_local_fixed_drive`] with the disk questions asked of `probe` (tests pass a fake): the
/// drive must be fixed *and* on an internal bus ([`crate::drive::BusType::is_internal`]), so a
/// USB disk that reports `DRIVE_FIXED` is refused, and so is a bus that can't be read.
pub fn on_local_fixed_drive_with(path: &str, probe: &dyn crate::drive::DriveProbe) -> bool {
    crate::drive::not_local_kind(path, probe).is_none()
}

/// Folder and file name of a canonical path (`\\?\` stripped, backslashes).
fn split_program(path: &str) -> (String, String) {
    let p = std::path::PathBuf::from(crate::keyboard::strip_verbatim(path).replace('/', "\\"));
    (
        p.parent()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default(),
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

impl Registry {
    /// The program of a module now: canonical path and SHA-256 of the file for an adopted one.
    /// A module the hub installed itself has no external program: its path is empty and its
    /// hash is the hash of the signed file map.
    /// `run[1..]` of an adopted module now; empty for any other (or an unreadable manifest).
    pub(crate) fn run_args(&self, id: &str) -> Vec<String> {
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
            // "Not there" keeps its own answer; any other failure (the file is busy, access
            // was refused) is passed on as it is, so the binding check can tell the two apart.
            let path = canonical_path(&exe).map_err(|e| {
                Failure::Io(if e.kind() == std::io::ErrorKind::NotFound {
                    crate::not_found(&exe)
                } else {
                    e
                })
            })?;
            // Something at that path that is not a file (a folder put in its place) is not the
            // app file: "not there", so it is never taken for a file that is only busy.
            if std::fs::metadata(&path).is_ok_and(|d| !d.is_file()) {
                return Err(Failure::Io(crate::not_found(&exe)));
            }
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
        let mut doc = serde_json::json!({
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
        // The code is for the whole command (#1452): an Allow for one argument list is refused
        // for another. Only a list that has arguments adds them, so the code of an app without
        // any is what it was before, and an OK stored for it still matches.
        if !run_args.is_empty() {
            doc["run_args"] = serde_json::json!(run_args);
        }
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
        let listing_path = listing.exe_path.clone();
        let one_run = self.one_run(id);
        let bound = m
            .bound
            .clone()
            .or_else(|| m.one_run.as_ref().map(RunOk::as_bound));
        let asks_again = one_run.is_some()
            && match m.one_run.as_ref() {
                Some(r) => r.state == RunState::Ended,
                None => m.bound.as_ref().is_some_and(|b| !b.void),
            };
        let same_place = m.source == ADOPTED
            && bound.as_ref().is_some_and(|b| {
                // Args compared exactly (fail closed): a grant bound with no args does not
                // match a manifest that now has some.
                reask_same_place(&listing.exe_path, &b.exe_path) && self.run_args(id) == b.run_args
            });
        Ok(View {
            listing,
            granted,
            declined: m.declined_permissions.clone(),
            pending: m.pending_permissions.clone(),
            last_choice: m.last_choice.clone(),
            same_place,
            bound,
            void: self.binding_void(id),
            adopted: m.source == ADOPTED,
            one_run,
            asks_again,
            folder_sync: if m.source == ADOPTED && !listing_path.is_empty() {
                self.probes.sync.check(&listing_path)
            } else {
                crate::sync_root::SyncAnswer::Local
            },
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
    ///
    /// `follow` is the dialog's "Keep my OK after updates in this folder": the binding then
    /// keeps a new hash of the same file ([`FollowFolder`]). Only `by == "dialog"` may set it,
    /// and only for an adopted program on a local fixed drive; anything else is refused and
    /// nothing is written. Every Allow states it again: `false` leaves no follow.
    ///
    /// An app whose program runs whatever it is handed ([`crate::one_run`], #1452) gets no
    /// binding from any caller: the Allow is stored as one run's OK
    /// ([`crate::inventory::RunOk`]), `follow` is refused, and no dev folder enrols it. This is
    /// the only place an Allow is stored, so the rule holds for the window and the control pipe
    /// alike.
    ///
    /// [`FollowFolder`]: crate::inventory::FollowFolder
    #[allow(clippy::too_many_arguments)]
    pub fn approve_listed(
        &mut self,
        id: &str,
        list_code: &str,
        chosen: &[String],
        declined: &[String],
        now: &str,
        by: &str,
        follow: bool,
    ) -> Result<Outcome, Failure> {
        self.approve_or_deny(id, list_code, chosen, declined, now, by, follow, false)
    }

    /// [`Registry::approve_listed`], or with `deny` a Deny ([`Registry::deny_listed`]). The two
    /// differ only for a one-run app: an Allow is the OK for its next start even when nothing
    /// is chosen, a Deny is no OK at all.
    #[allow(clippy::too_many_arguments)]
    fn approve_or_deny(
        &mut self,
        id: &str,
        list_code: &str,
        chosen: &[String],
        declined: &[String],
        now: &str,
        by: &str,
        follow: bool,
        deny: bool,
    ) -> Result<Outcome, Failure> {
        self.reload_inventory()?;
        // A new build the folder grant keeps is recorded first, so the Sensitive items it
        // withholds are pending in the list this Allow is checked against (#1465).
        self.keep_followed_inner(id, now)?;
        // An Allow (or a Deny) writes a binding. Grants that are void (legacy, no binding, or
        // the program changed) must not ride into it unseen: move them back to waiting first,
        // so the list code changes and the dialog reopens with everything the binding covers.
        if self.binding_void(id).is_some() && self.void_if_changed(id, now)?.is_some() {
            return Err(list_changed(list_code, self.permission_list(id)?.code()));
        }
        let list = self.permission_list(id)?;
        if !list.matches(list_code) {
            return Err(list_changed(list_code, list.code()));
        }
        let one_run = if self.inventory.modules[id].source == ADOPTED {
            crate::one_run::one_run_rule(&list.exe_path, self.windows_folder.as_deref())
        } else {
            None
        };
        if follow && one_run.is_some() {
            return Err(io_invalid(
                "the OK for this app is for one run only and can't be kept; to be asked only \
                 once, use a version that has its own program file",
            ));
        }
        let follow = if follow {
            if by != FOLLOW_SET_BY {
                return Err(Failure::Check(Error::new(
                    ErrorId::PermissionCaller,
                    "only the Hideout window can keep the OK for a folder",
                )));
            }
            if list.exe_path.is_empty() || !on_local_fixed_drive(&list.exe_path) {
                return Err(io_invalid(
                    "the OK can be kept only for an app in a folder on this PC's own drive",
                ));
            }
            let (folder, file_name) = split_program(&list.exe_path);
            Some(FollowFolder {
                folder,
                file_name,
                at: now.into(),
                by: FOLLOW_SET_BY.into(),
            })
        } else {
            None
        };
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
        // Dev mode: an Allow from the window for an app inside a dev folder enrols it there
        // (identity `dev-folder`, nothing inherited: `sensitive_confirmed` starts empty). Any
        // other Allow (and a Deny, which allows nothing) drops a dev-folder identity the app had.
        let dev_folder = if by == FOLLOW_SET_BY
            && one_run.is_none()
            && !chosen.is_empty()
            && self.inventory.modules[id].source == ADOPTED
        {
            self.dev_folder_for(&list.exe_path)
        } else {
            None
        };
        let before = self.inventory.modules[id].granted.clone();
        let refused = self.inventory.grant(id, &asked).unwrap_or_default();
        let m = self.inventory.modules.get_mut(id).expect("listed");
        m.identity = match dev_folder {
            Some(folder) => Some(crate::inventory::Identity {
                class: crate::inventory::IdentityClass::DevFolder {
                    folder: Some(folder),
                },
                kind: Some(m.kind),
                visibility: Some(m.visibility),
                established_at: Some(now.into()),
                established_by: Some(FOLLOW_SET_BY.into()),
                sensitive_confirmed: Vec::new(),
            }),
            None => m
                .identity
                .take()
                .filter(|i| !matches!(i.class, crate::inventory::IdentityClass::DevFolder { .. })),
        };
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
        if one_run.is_some() && deny {
            // A Deny allows nothing, so it is not an OK for a run: an OK that is still live
            // stays as it was, one that is over is dropped, and none is written. A binding
            // from before the rule is dropped, and a keyboard record that is `on` goes with it.
            if m.bound.take().is_some() {
                if let Some(k) = m
                    .keyboard
                    .as_mut()
                    .filter(|k| k.state == crate::keyboard::GrantState::On)
                {
                    k.state = crate::keyboard::GrantState::Void;
                    k.sha256.clear();
                    k.reason = Some(VoidReason::Other);
                    k.at = now.into();
                }
            }
            if !m.one_run.as_ref().is_some_and(RunOk::live) {
                m.one_run = None;
            }
        } else if one_run.is_some() {
            // Nothing is remembered: this OK is for the next start, and that start uses it up.
            m.bound = None;
            m.one_run = Some(RunOk {
                manifest_sha256: list.manifest_sha256.clone(),
                exe_path: list.exe_path.clone(),
                exe_sha256: list.exe_sha256.clone(),
                list_hash: list.list_hash.clone(),
                at: now.into(),
                by: by.into(),
                run_args: list.run_args.clone(),
                state: RunState::Allowed,
            });
        } else {
            m.one_run = None;
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
                follow,
            });
        }
        self.save_inventory()?;
        let refused: Vec<(String, Error)> = refused_raw.into_iter().zip(refused).collect();
        Ok(Outcome {
            granted: got,
            declined: declined.to_vec(),
            refused,
            bound_sha256: list.exe_sha256,
            bound_path: list.exe_path,
            one_run,
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
        self.approve_or_deny(id, list_code, &[], declined, now, by, false, true)
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

    /// Turn off "Keep my OK after updates in this folder": only `bound.follow` is cleared. The
    /// binding's hash, path, list, history and every grant stay as they are, so the next new
    /// build voids as it does without a follow, and a build that landed but was not yet checked
    /// is not recorded as kept. Narrows (like [`Registry::revoke_items`]), so any hub caller may
    /// use it. Returns whether a follow was cleared.
    pub fn stop_keeping(&mut self, id: &str) -> Result<bool, Failure> {
        self.reload_inventory()?;
        let m = self
            .inventory
            .modules
            .get_mut(id)
            .ok_or_else(|| Failure::NotInstalled(id.into()))?;
        let Some(b) = m.bound.as_mut().filter(|b| b.follow.is_some()) else {
            return Ok(false);
        };
        b.follow = None;
        self.save_inventory()?;
        Ok(true)
    }

    /// Whether a grant of `id` is bound and the program no longer matches (or was already
    /// voided). Read-only; [`Registry::void_if_changed`] writes.
    ///
    /// An app file that is there but can't be read now also answers `Moved` here, so every
    /// caller that reads this refuses (fails closed); [`Registry::app_file_unreadable`] tells
    /// that case apart, and it is never written as a void.
    pub fn binding_void(&self, id: &str) -> Option<VoidReason> {
        self.binding_void_with(id, || self.program(id), None)
    }

    /// Whether the grant of `id` is bound and not void, and its app file is there but could not
    /// be read just now (another program has it open, access was refused). Nothing may start
    /// from such a file, but it says nothing about which program it is, so the user's Allow is
    /// kept and the next check reads it again. A file that is not there is not this: that one
    /// is `Moved`. Read-only.
    pub fn app_file_unreadable(&self, id: &str) -> bool {
        self.binding_check(id, || self.program(id), None) == Check::Unreadable
    }

    /// [`Registry::binding_void`] against a program the caller already holds open and hashed
    /// (S7, F4): `path` and `sha256` come from the pinned handle, so the check is about the
    /// bytes that are about to run, not a second read of the name. A changed hash is `Changed`
    /// here even under a follow-folder grant: only [`Registry::keep_followed`] may carry a
    /// follow across new bytes (and hold the held-back items), so bytes that arrived after it
    /// ran are not excused (#1465). A dev enrolment still excuses.
    ///
    /// This form knows nothing of the start it is asked about, so for a one-run app (#1452) it
    /// always answers that the OK is needed: only [`Registry::binding_void_launch`] can pass
    /// one.
    pub fn binding_void_pinned(&self, id: &str, path: &str, sha256: &str) -> Option<VoidReason> {
        let launch = Launch {
            args: None,
            counted: false,
        };
        self.binding_void_with(
            id,
            || Ok((path.to_string(), sha256.to_string())),
            Some(launch),
        )
    }

    /// [`Registry::binding_void_pinned`] for the launch itself: `args` are the arguments the
    /// process is about to be started with (not a second read of the manifest), and `counted`
    /// is [`Registry::start_counted`] of the registry that ran [`Registry::keep_followed`] for
    /// this start. A one-run app passes only when `counted` is true: a start whose count could
    /// not be read or written down is refused, so one OK never covers more than one start.
    pub fn binding_void_launch(
        &self,
        id: &str,
        path: &str,
        sha256: &str,
        args: &[String],
        counted: bool,
    ) -> Option<VoidReason> {
        let launch = Launch {
            args: Some(args),
            counted,
        };
        self.binding_void_with(
            id,
            || Ok((path.to_string(), sha256.to_string())),
            Some(launch),
        )
    }

    /// Whether the last [`Registry::keep_followed`] of this registry counted a start of `id` as
    /// the one its one-run OK is for, and wrote that down. False for a registry that did not run
    /// the step itself, whatever the file says.
    pub fn start_counted(&self, id: &str) -> bool {
        self.counted_start.as_deref() == Some(id)
    }

    fn binding_void_with(
        &self,
        id: &str,
        program: impl FnOnce() -> Result<(String, String), Failure>,
        pinned: Option<Launch<'_>>,
    ) -> Option<VoidReason> {
        match self.binding_check(id, program, pinned) {
            Check::Fine => None,
            Check::Void(why) => Some(why),
            // Refused like a moved file; only the write ([`Registry::void_if_changed`]) and
            // the wording treat it differently.
            Check::Unreadable => Some(VoidReason::Moved),
        }
    }

    fn binding_check(
        &self,
        id: &str,
        program: impl FnOnce() -> Result<(String, String), Failure>,
        launch: Option<Launch<'_>>,
    ) -> Check {
        let pinned = launch.is_some();
        let Some(m) = self.inventory.modules.get(id) else {
            return Check::Fine;
        };
        // A one-run app is held to its run's record, never to a binding: one from before the
        // rule (or written into the file) is not an OK for it.
        if m.source == ADOPTED
            && (self.one_run(id).is_some() || (m.bound.is_none() && m.one_run.is_some()))
        {
            return self.one_run_check(id, m, program, launch);
        }
        let Some(b) = m.bound.as_ref() else {
            // Old data: an adopted record with grants and no binding would carry them to any
            // program. Every Allow writes a binding, so this has no legitimate source.
            return if m.source == ADOPTED && !m.granted.is_empty() {
                Check::Void(VoidReason::Changed)
            } else {
                Check::Fine
            };
        };
        if b.void {
            return Check::Void(VoidReason::Changed);
        }
        // A binding with no program hash that still holds grants binds them to nothing.
        if m.source == ADOPTED && b.exe_sha256.is_empty() && !m.granted.is_empty() {
            return Check::Void(VoidReason::Changed);
        }
        if m.source != ADOPTED || b.exe_sha256.is_empty() {
            return Check::Fine;
        }
        match program() {
            Ok((path, hash)) => {
                if self.args_at(id, launch) != b.run_args {
                    Check::Void(VoidReason::Changed)
                } else if !crate::keyboard::same_path(&path, &b.exe_path)
                    && self.dev_enrolment(id, m, b, &path).is_none()
                {
                    Check::Void(VoidReason::Moved)
                } else if !hash.eq_ignore_ascii_case(&b.exe_sha256) {
                    if (!pinned && self.follows(id, b, &path))
                        || self.dev_enrolment(id, m, b, &path).is_some()
                    {
                        Check::Fine
                    } else {
                        Check::Void(VoidReason::Changed)
                    }
                } else {
                    Check::Fine
                }
            }
            // The file is there and this read of it failed: not known to be another program.
            Err(Failure::Io(e)) if e.kind() != std::io::ErrorKind::NotFound => Check::Unreadable,
            // A program that is not there can't be the one that was allowed.
            Err(_) => Check::Void(VoidReason::Moved),
        }
    }

    /// The check for an app with a one-run OK (#1452). Without a record there is no OK: grants
    /// that are there anyway (a remembered OK from before the rule) are void. A record that is
    /// over allows nothing. A live one holds the program to the path, the bytes and the
    /// arguments of the Allow; nothing excuses a difference (no follow, no dev folder). At the
    /// launch the start must also be the counted one ([`Launch::counted`], and the record says
    /// `started`), so a start whose count could not be read or written down does not run: the
    /// record alone, which stays `started` while the app runs and after it ends, is not enough.
    fn one_run_check(
        &self,
        id: &str,
        m: &crate::inventory::Installed,
        program: impl FnOnce() -> Result<(String, String), Failure>,
        launch: Option<Launch<'_>>,
    ) -> Check {
        let Some(r) = m.one_run.as_ref() else {
            return if m.granted.is_empty() && m.bound.as_ref().is_none_or(|b| !b.void) {
                Check::Fine
            } else {
                Check::Void(VoidReason::Changed)
            };
        };
        if !r.live() {
            return Check::Void(VoidReason::Changed);
        }
        match program() {
            Ok((path, hash)) => {
                if self.args_at(id, launch) != r.run_args {
                    Check::Void(VoidReason::Changed)
                } else if !crate::keyboard::same_path(&path, &r.exe_path) {
                    Check::Void(VoidReason::Moved)
                } else if !hash.eq_ignore_ascii_case(&r.exe_sha256)
                    || launch.is_some_and(|l| !l.counted || r.state != RunState::Started)
                {
                    Check::Void(VoidReason::Changed)
                } else {
                    Check::Fine
                }
            }
            Err(Failure::Io(e)) if e.kind() != std::io::ErrorKind::NotFound => Check::Unreadable,
            Err(_) => Check::Void(VoidReason::Moved),
        }
    }

    /// The arguments a check compares with the ones allowed: at the launch, the ones the
    /// process is started with; otherwise the manifest's as it reads now.
    fn args_at(&self, id: &str, launch: Option<Launch<'_>>) -> Vec<String> {
        match launch.and_then(|l| l.args) {
            Some(a) => a.to_vec(),
            None => self.run_args(id),
        }
    }

    /// Why `id` gets a one-run OK (#1452), or `None`: the rule of [`crate::one_run`] on the
    /// canonical path of an adopted app's program as it is now (the path as written when the
    /// file is not there, so a missing runner is still a runner).
    pub fn one_run(&self, id: &str) -> Option<OneRun> {
        self.inventory
            .modules
            .get(id)
            .filter(|m| m.source == ADOPTED)?;
        let exe = adopted_exe(&self.manifest_checked(id).ok()?).ok()?;
        let path = canonical_path(&exe).unwrap_or_else(|_| exe.to_string_lossy().into_owned());
        crate::one_run::one_run_rule(&path, self.windows_folder.as_deref())
    }

    /// Counts a start of a one-run app (#1452), before the launch checks: the first start after
    /// an Allow marks the OK as used (`started`); any start after that ends it, so everything
    /// allowed waits for the user again (the choice kept in `last_choice`) and the launch is
    /// refused until the next Allow. A remembered OK such an app still has from before the rule
    /// ends the same way. Written under the inventory's lock, against the file as it is then.
    ///
    /// Only a start that this call itself moved from `allowed` to `started`, with the write
    /// done, is the counted one ([`Registry::start_counted`]); every other outcome (an error
    /// included) leaves no start counted.
    fn one_run_step(&mut self, id: &str, now: &str) -> Result<(), Failure> {
        self.counted_start = None;
        let Some(m) = self.inventory.modules.get(id) else {
            return Ok(());
        };
        if m.source != ADOPTED {
            return Ok(());
        }
        let rule = self.one_run(id).is_some();
        let old_binding = rule && m.bound.is_some();
        if !old_binding && !m.one_run.as_ref().is_some_and(RunOk::live) {
            if rule {
                // No OK to count this start against: nothing remembered goes with it.
                self.void_keyboard_uncounted(id, now)?;
            }
            return Ok(());
        }
        let mut ended = false;
        let mut counted = false;
        let inv = crate::inventory::Inventory::update(&self.home.join("inventory.json"), |inv| {
            let Some(m) = inv.modules.get_mut(id) else {
                return;
            };
            if rule {
                if let Some(b) = m.bound.take() {
                    m.one_run = Some(RunOk {
                        manifest_sha256: b.manifest_sha256,
                        exe_path: b.exe_path,
                        exe_sha256: b.exe_sha256,
                        list_hash: b.list_hash,
                        at: b.at,
                        by: b.by,
                        run_args: b.run_args,
                        state: RunState::Started,
                    });
                }
            }
            let Some(r) = m.one_run.as_mut().filter(|r| r.live()) else {
                return;
            };
            if r.state == RunState::Allowed {
                r.state = RunState::Started;
                counted = true;
                return;
            }
            r.state = RunState::Ended;
            r.exe_sha256.clear();
            end_grants(m);
            ended = true;
        })?;
        if let Some(m) = inv.modules.get(id) {
            self.inventory.modules.insert(id.into(), m.clone());
        }
        if ended {
            let _ = self.void_keyboard(id, VoidReason::Other, now);
        }
        if counted {
            self.counted_start = Some(id.into());
        } else if rule {
            self.void_keyboard_uncounted(id, now)?;
        }
        Ok(())
    }

    /// A one-run app's start that is not the counted one gets no keyboard grant: a record that
    /// is `on` (kept from before the rule) goes void. Does nothing without such a record.
    fn void_keyboard_uncounted(&mut self, id: &str, now: &str) -> Result<(), Failure> {
        let on = self.inventory.modules.get(id).is_some_and(|m| {
            m.keyboard
                .as_ref()
                .is_some_and(|k| k.state == crate::keyboard::GrantState::On)
        });
        if on {
            self.void_keyboard(id, VoidReason::Other, now)?;
        }
        Ok(())
    }

    /// Whether `b`'s follow-folder grant keeps a new hash of the program now at `path`
    /// (canonical, as [`Registry::program`] read it): a follow set by the dialog, the binding
    /// not void, `path` exactly the bound path and exactly the follow's folder and file name, on
    /// a local fixed drive, and `run[1..]` exactly as at the Allow (an empty list included).
    /// The path is canonical, so a junction, symlink or 8.3 name that leads elsewhere is
    /// another path and never follows.
    fn follows(&self, id: &str, b: &Bound, path: &str) -> bool {
        let Some(f) = b.follow.as_ref() else {
            return false;
        };
        let (folder, file_name) = split_program(path);
        f.by == FOLLOW_SET_BY
            && !b.void
            && !b.exe_sha256.is_empty()
            && crate::keyboard::same_path(path, &b.exe_path)
            && crate::keyboard::same_path(&folder, &f.folder)
            && file_name.eq_ignore_ascii_case(&f.file_name)
            && on_local_fixed_drive(path)
            // A folder a cloud service keeps in step can be rewritten from elsewhere: nothing
            // is kept there, and a reading that can't be made counts the same (fails closed).
            && self.probes.sync.check(path) == crate::sync_root::SyncAnswer::Local
            && self.run_args(id) == b.run_args
    }

    /// Records a program change a follow-folder grant keeps (#1178 follow-up): the binding's
    /// hash moves to the new file and `attested` gets an entry `by: "folder"`. A kept change
    /// carries only what is not Sensitive (#1465): Sensitive grants go back to pending (the
    /// choice kept in `last_choice`, so the dialog shows them as allowed before) and an `on`
    /// keyboard record for the old file goes void. A signed update never gets here: the
    /// updater or installer attests it first (`attest_update`), so the binding already names
    /// the new file. A dev-folder step is unchanged: its record moves with it. Does nothing (false) without a follow, with no
    /// change, or with a change the follow does not keep (that one voids as before). Hub-side
    /// only; the launch calls it before it checks the binding.
    ///
    /// It is called once for every start, so it is also where a start of a one-run app is
    /// counted (#1452): the start after an Allow uses that OK, and every later one ends it.
    pub fn keep_followed(&mut self, id: &str, now: &str) -> Result<bool, Failure> {
        self.one_run_step(id, now)?;
        self.keep_followed_inner(id, now)
    }

    /// [`Registry::keep_followed`] without counting a start: what an Allow and the turn-on
    /// check use.
    fn keep_followed_inner(&mut self, id: &str, now: &str) -> Result<bool, Failure> {
        let Some(m) = self.inventory.modules.get(id) else {
            return Ok(false);
        };
        let Some(b) = m.bound.as_ref() else {
            return Ok(false);
        };
        if m.source != ADOPTED {
            return Ok(false);
        }
        let Ok((path, hash)) = self.program(id) else {
            return Ok(false);
        };
        let same = crate::keyboard::same_path(&path, &b.exe_path);
        let by = if same && b.follow.is_some() && self.follows(id, b, &path) {
            FOLLOW_ATTESTED_BY
        } else if self.dev_enrolment(id, m, b, &path).is_some() {
            crate::dev_folder::DEV_ATTESTED_BY
        } else {
            return Ok(false);
        };
        if same && hash.eq_ignore_ascii_case(&b.exe_sha256) {
            return Ok(false);
        }
        let (old, old_path) = (b.exe_sha256.clone(), b.exe_path.clone());
        let (follow, version) = (b.follow.clone(), m.version.clone());
        let dev_step = by == crate::dev_folder::DEV_ATTESTED_BY;
        let enrolled = m.identity.clone();
        let rec = crate::inventory::Attestation {
            from_sha256: old.clone(),
            to_sha256: hash.clone(),
            from_path: b.exe_path.clone(),
            to_path: path.clone(),
            version,
            at: now.into(),
            by: by.into(),
            held_back: !dev_step,
        };
        let mut kept = false;
        let inv = crate::inventory::Inventory::update(&self.home.join("inventory.json"), |inv| {
            let Some(m) = inv.modules.get_mut(id) else {
                return;
            };
            // Only the binding that was checked: another writer meanwhile (a void, a new
            // Allow, an attested update) wins.
            let Some(b) = m.bound.as_mut().filter(|b| {
                !b.void
                    && b.exe_sha256.eq_ignore_ascii_case(&old)
                    && b.follow == follow
                    && crate::keyboard::same_path(&b.exe_path, &old_path)
                    // A dev-folder step also needs the enrolment as it is on disk now: a
                    // CLI Allow or a void from another process wins over this carry.
                    && (!dev_step
                        || (m.identity == enrolled && m.source == ADOPTED && b.by == FOLLOW_SET_BY))
            }) else {
                return;
            };
            b.exe_sha256 = hash.clone();
            b.exe_path = path.clone();
            b.attested.push(rec.clone());
            let n = b.attested.len();
            if n > 20 {
                b.attested.drain(..n - 20);
            }
            if dev_step {
                // The record moves with a dev-folder step. It is the router's chain
                // (`attested_keyboard_move`) that decides whether memory follows: it checks
                // the enrolment again at that time, and a break there ends in a re-ask.
                if let Some(k) = m.keyboard.as_mut().filter(|k| k.allows(&old_path, &old)) {
                    k.sha256 = hash.clone();
                    k.path = path.clone();
                }
            } else {
                // Bytes changed under the folder's grant: Sensitive items and the keyboard
                // wait for the user's OK again; the rest carries.
                hold_back(m, now, |k| k.allows(&old_path, &old));
            }
            kept = true;
        })?;
        if let Some(m) = inv.modules.get(id) {
            self.inventory.modules.insert(id.into(), m.clone());
        }
        Ok(kept)
    }

    /// Applies the #1465 rule once to folder steps recorded before it. Such a step carried
    /// every grant and the keyboard record to changed bytes; a binding whose newest step is one
    /// gets what a step made now gets: the held items ([`held_on_change`]) wait for the user's
    /// OK again (kept in `last_choice`), an `on` keyboard record goes void (reason `changed`),
    /// everything else stays, and the row reads as changed since the user allowed it. The step
    /// is then marked `held_back`, so a later open changes nothing.
    ///
    /// A step without the mark cannot be told from one a build with the rule but without the
    /// mark wrote, nor is an Allow made after the old step proof that the user saw the held
    /// items for the new bytes: both read as "not held back yet" (fail closed; the cost is one
    /// more ask). A binding whose newest step is an updater's, installer's or dev folder's is
    /// not touched. Returns the ids changed. Runs when the registry opens.
    pub fn reask_old_folder_steps(&mut self, now: &str) -> Result<Vec<String>, Failure> {
        let ids: Vec<String> = self
            .inventory
            .modules
            .iter()
            .filter(|(_, m)| old_folder_step(m))
            .map(|(id, _)| id.clone())
            .collect();
        if ids.is_empty() {
            return Ok(ids);
        }
        // In memory first, so a failed write still leaves this process without the carry (and
        // the next open tries again).
        for m in self.inventory.modules.values_mut() {
            reask_old_folder_step(m, now);
        }
        let path = self.home.join("inventory.json");
        crate::inventory::Inventory::update(&path, |inv| {
            for m in inv.modules.values_mut() {
                reask_old_folder_step(m, now);
            }
        })?;
        Ok(ids)
    }

    /// The check at every turn on and launch: if the app is not the one the grant is bound
    /// to, the grant is **void**: `granted` goes back to pending (the choices kept as
    /// `last_choice`), the binding's hash is cleared (the old file does not revive it) and the
    /// keyboard record goes void. Returns the reason when it voided now.
    ///
    /// A change a follow-folder grant keeps is recorded first ([`Registry::keep_followed`]) and
    /// does not void.
    ///
    /// An app file that is there but can't be read at this moment voids nothing (`None`): one
    /// failed read is not a changed program. Nothing starts meanwhile, since every read-only
    /// check ([`Registry::binding_void`], the launch) still refuses, and the next check that
    /// can read the file decides.
    pub fn void_if_changed(&mut self, id: &str, now: &str) -> Result<Option<VoidReason>, Failure> {
        self.keep_followed_inner(id, now)?;
        let Check::Void(reason) = self.binding_check(id, || self.program(id), None) else {
            return Ok(None);
        };
        let m = &self.inventory.modules[id];
        if m.bound.as_ref().is_some_and(|b| b.void) {
            return Ok(None);
        }
        // A one-run OK that is already over, with nothing left to take back.
        if m.bound.is_none()
            && m.granted.is_empty()
            && m.one_run.as_ref().is_some_and(|r| !r.live())
        {
            return Ok(None);
        }
        // A one-run app keeps no binding, void or not: what is left is its run's record.
        let one_run = m.bound.is_none() || self.one_run(id).is_some();
        let path = self.home.join("inventory.json");
        let inv = crate::inventory::Inventory::update(&path, |inv| {
            if let Some(m) = inv.modules.get_mut(id) {
                let old = std::mem::take(&mut m.granted);
                m.pending_permissions.extend(old.iter().cloned());
                m.pending_permissions.sort();
                m.pending_permissions.dedup();
                m.last_choice = old;
                // A void is an identity change: a dev-folder enrolment ends with it.
                if m.identity.as_ref().is_some_and(|i| {
                    matches!(i.class, crate::inventory::IdentityClass::DevFolder { .. })
                }) {
                    m.identity = None;
                }
                if one_run {
                    if let Some(b) = m.bound.take() {
                        // A remembered OK from before the rule: over, as a used one is.
                        m.one_run = Some(RunOk {
                            manifest_sha256: b.manifest_sha256,
                            exe_path: b.exe_path,
                            exe_sha256: String::new(),
                            list_hash: b.list_hash,
                            at: b.at,
                            by: b.by,
                            run_args: b.run_args,
                            state: RunState::Ended,
                        });
                    }
                    if let Some(r) = m.one_run.as_mut() {
                        if r.live() {
                            r.state = RunState::Void;
                        }
                        r.exe_sha256.clear();
                    }
                } else if let Some(b) = m.bound.as_mut() {
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

    /// The install or update path vouches that the app file now at the module's `run[0]` is
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
                "the app is in another folder than the one allowed",
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
            held_back: false,
        };
        let inv = crate::inventory::Inventory::update(&self.home.join("inventory.json"), |inv| {
            if let Some(m) = inv.modules.get_mut(id) {
                // The keyboard record follows the same file, but only a record that is `on`
                // **and** pinned to exactly the bound old file (never one for another file),
                // and only to the file the grant itself moved to.
                if let (Some(g), Some(b)) = (m.keyboard.as_mut(), m.bound.as_ref()) {
                    if g.state == crate::keyboard::GrantState::On
                        && g.sha256.eq_ignore_ascii_case(&b.exe_sha256)
                        && crate::keyboard::same_path(&g.path, &b.exe_path)
                    {
                        g.path = path.clone();
                        g.sha256 = hash.to_ascii_lowercase();
                        g.at = now.into();
                    }
                }
            }
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

    /// The keyboard-record moves in the file the router may follow (#1267): for an adopted
    /// module, an `on` record that sits on a binding that is not void, was set by the
    /// dialog, is pinned to the same canonical local-fixed-drive file with the same `run[1..]`,
    /// and was reached from earlier hashes of that same file only by attested steps
    /// (`updater`, `installer`, or `folder` while the follow still holds, or `dev-folder` while
    /// `dev_enrolment` holds now, with both paths of the step inside the enrolled folder). A step
    /// that changed the path (a dev-folder step only inside its folder), broke the hash chain,
    /// or has another `by` ends the chain there, so a moved, void or unattested record yields
    /// nothing. Ancestors' old paths are checked by name only (inside the folder, local, not
    /// UNC): an old file is never run. Read-only; hub-side only.
    pub fn attested_keyboard_moves(&self) -> Vec<crate::keyboard::KeyboardMove> {
        self.inventory
            .modules
            .keys()
            .filter_map(|id| self.attested_keyboard_move(id))
            .collect()
    }

    /// A dev-folder step moved the keyboard record on disk (`keep_followed`), but the router
    /// follows it only while the chain holds now (enrolment, ACLs, paths). When the record is
    /// still `on` for the program the binding names, the binding's newest step is a dev-folder
    /// step, and the record was not allowed after that step, yet no move is confirmed, the
    /// record goes void on disk (`Changed`), so the file never says `on` for a build the router
    /// refused: the user is asked again, and a fresh router does not revive it. Fails closed:
    /// if the file cannot be written the record is void in this copy all the same. Returns the
    /// ids voided. Hub-side; call it before [`Registry::attested_keyboard_moves`].
    pub fn void_unchained_dev_keyboards(&mut self, now: &str) -> Vec<String> {
        let ids: Vec<String> =
            self.inventory
                .modules
                .iter()
                .filter(|(id, m)| {
                    let (Some(k), Some(b)) = (m.keyboard.as_ref(), m.bound.as_ref()) else {
                        return false;
                    };
                    let newest_is_dev = b.attested.last().is_some_and(|a| {
                        a.by == crate::dev_folder::DEV_ATTESTED_BY && k.at <= a.at
                    });
                    m.source == ADOPTED
                        && k.state == crate::keyboard::GrantState::On
                        && !b.void
                        && newest_is_dev
                        && k.allows(&b.exe_path, &b.exe_sha256)
                        && self.attested_keyboard_move(id).is_none()
                })
                .map(|(id, _)| id.clone())
                .collect();
        for id in &ids {
            let reason = crate::keyboard::VoidReason::Changed;
            if self.void_keyboard(id, reason, now).is_err() {
                if let Some(k) = self
                    .inventory
                    .modules
                    .get_mut(id)
                    .and_then(|m| m.keyboard.as_mut())
                {
                    k.state = crate::keyboard::GrantState::Void;
                    k.sha256.clear();
                }
            }
        }
        ids
    }

    fn attested_keyboard_move(&self, id: &str) -> Option<crate::keyboard::KeyboardMove> {
        let m = self.inventory.modules.get(id)?;
        let (k, b) = (m.keyboard.as_ref()?, m.bound.as_ref()?);
        let hash = |s: &str| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit());
        if m.source != ADOPTED
            || k.state != crate::keyboard::GrantState::On
            || b.void
            || b.by != FOLLOW_SET_BY
            || !hash(&k.sha256)
            || !k.sha256.eq_ignore_ascii_case(&b.exe_sha256)
            || !crate::keyboard::same_path(&k.path, &b.exe_path)
            || crate::keyboard::is_unc(&k.path)
            || !on_local_fixed_drive(&k.path)
            || self.run_args(id) != b.run_args
        {
            return None;
        }
        let mut ancestors: Vec<(String, String)> = Vec::new();
        let mut next = b.exe_sha256.clone();
        let mut next_path = b.exe_path.clone();
        // The enrolled folder, looked up now (not read from history) the first time a dev step
        // is met; None when enrolment does not hold, which ends the chain at that step.
        let mut dev_folder: Option<Option<String>> = None;
        let mut used_dev = None;
        for a in b.attested.iter().rev() {
            // Only steps made under this binding: a re-Allow carries older steps over, which
            // this binding never vouched for. Hashes never repeat (no loop, no fork).
            if a.at.as_str() < b.at.as_str()
                || a.from_sha256.eq_ignore_ascii_case(&b.exe_sha256)
                || ancestors
                    .iter()
                    .any(|(_, h)| h.eq_ignore_ascii_case(&a.from_sha256))
            {
                break;
            }
            let dev = a.by == crate::dev_folder::DEV_ATTESTED_BY;
            let folder = if dev {
                dev_folder
                    .get_or_insert_with(|| self.dev_enrolment(id, m, b, &b.exe_path))
                    .clone()
            } else {
                None
            };
            let by_ok = match a.by.as_str() {
                "updater" | "installer" => true,
                // A folder step never carries the keyboard (#1465): bytes changed with no
                // signed update behind them.
                FOLLOW_ATTESTED_BY => false,
                _ => folder.is_some(),
            };
            let paths_ok = match &folder {
                Some(f) => {
                    crate::dev_folder::path_within(f, &a.from_path)
                        && crate::dev_folder::path_within(f, &a.to_path)
                        && !crate::keyboard::is_unc(&a.from_path)
                        && on_local_fixed_drive(&a.from_path)
                }
                None => {
                    crate::keyboard::same_path(&a.from_path, &b.exe_path)
                        && crate::keyboard::same_path(&a.to_path, &b.exe_path)
                }
            };
            if !by_ok
                || !paths_ok
                || !crate::keyboard::same_path(&a.to_path, &next_path)
                || !a.to_sha256.eq_ignore_ascii_case(&next)
                || !hash(&a.from_sha256)
                || a.from_sha256.eq_ignore_ascii_case(&a.to_sha256)
            {
                break;
            }
            if folder.is_some() {
                used_dev = folder;
            }
            ancestors.push((a.from_path.clone(), a.from_sha256.to_ascii_lowercase()));
            next = a.from_sha256.clone();
            next_path = a.from_path.clone();
        }
        if ancestors.is_empty() {
            return None;
        }
        ancestors.reverse();
        Some(crate::keyboard::KeyboardMove {
            id: id.into(),
            ancestors,
            to: k.clone(),
            dev_folder: used_dev,
        })
    }
}
