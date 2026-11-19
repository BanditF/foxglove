// SPDX-License-Identifier: MIT
//! Dev folders (trust model, dev mode; replaces the per-machine dev signing key). With developer
//! mode on, a folder the user marked as a **dev folder** vouches for the apps inside it: an adopted
//! app whose program canonically sits inside one keeps its grants over a rebuild (a new file hash,
//! even another file in the folder) with no new Allow. Guard rails:
//!
//! - dev mode is off unless the process says so ([`crate::dev_mode_from_env`]); with it off the
//!   folder list is kept but counts for nothing;
//! - a folder is accepted only as a canonical directory on this PC's local fixed drive, never a
//!   drive root, UNC, removable or network path; the program path is canonical too (a junction,
//!   symlink or 8.3 name that leads outside the folder is another path and never matches);
//! - only an Allow made in the Hideout window enrols an app (identity `dev-folder`); an app that
//!   was never allowed, whose grant is void, or that now runs from outside the folder falls back
//!   to the ordinary check, which voids the grant (a full re-ask: the identity is dropped and
//!   sensitive items are never carried);
//! - the carry pins the folder, not the bytes: the launch check accepts any file hash inside an
//!   enrolled folder, so `attested[].to_sha256` is the file as hashed at the carry, which may not
//!   be the image that later ran (anyone who can write into the folder can swap it, which is why
//!   [`folder_problem`] refuses a folder others can write);
//! - the carry moves only the file binding. It never adds a grant: what a rebuild newly asks for
//!   stays pending, and the user's Allow is still the only thing that grants it.

use crate::inventory::{Bound, Identity, IdentityClass, Installed, ADOPTED};
use crate::keyboard::{canonical_path, same_path, strip_verbatim};
use crate::permission::on_local_fixed_drive;
use crate::{Failure, HubConfig, Registry};

/// `attested[].by` of a program change a dev folder kept.
pub const DEV_ATTESTED_BY: &str = "dev-folder";

fn norm(p: &str) -> String {
    strip_verbatim(p).replace('/', "\\").to_lowercase()
}

/// Whether `path` is strictly inside `folder`, compared by whole components (case folded, either
/// slash). `C:\Dev` does not contain `C:\Dev2\x.exe`, and a folder does not contain itself.
pub fn path_within(folder: &str, path: &str) -> bool {
    let f = norm(folder);
    let f = f.trim_end_matches('\\');
    let p = norm(path);
    !f.is_empty() && p.len() > f.len() + 1 && p.starts_with(f) && p.as_bytes()[f.len()] == b'\\'
}

/// Rights that let a trustee change what is in a folder (add a file or subfolder, delete, rewrite
/// the ACL, take ownership, and the generic write and all bits).
const FOLDER_WRITE: u32 =
    0x2 | 0x4 | 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x4000_0000 | 0x1000_0000;
/// What matters on a folder above: swapping or renaming the folder below it.
const PARENT_WRITE: u32 = 0x2 | 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x4000_0000 | 0x1000_0000;

/// Trustees that may write: SYSTEM, Administrators, the TrustedInstaller service, CREATOR OWNER
/// (an inherit-only placeholder) and the current user.
fn trusted_writer(sid: &str, me: &str) -> bool {
    sid == me
        || matches!(sid, "S-1-5-18" | "S-1-5-32-544" | "S-1-3-0")
        || sid.starts_with("S-1-5-80-")
}

/// What matters on the drive root: swapping or deleting what is in it. Add-file and add-folder
/// are left out (every default root grants them); inherit-only ACEs there apply to children only.
const ROOT_WRITE: u32 = 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000;

#[derive(Clone, Copy, PartialEq)]
enum Where {
    Folder,
    Root,
}

#[cfg(windows)]
fn writable_by_others(path: &std::path::Path, mask: u32, at: Where, me: &str) -> bool {
    // A DACL or owner that can't be read is not known to be safe.
    let Ok(d) = hideout_acl::dacl(path) else {
        return true;
    };
    let acl_open = d.aces.iter().any(|a| {
        if at == Where::Root && a.inherit_only {
            return false;
        }
        if a.kind > 1 {
            // A callback, object or other kind of ACE carries conditions that aren't read here:
            // one that could grant write counts as granting it.
            a.mask & mask != 0
        } else {
            a.allow && a.mask & mask != 0 && !trusted_writer(&a.sid, me)
        }
    });
    if acl_open {
        return true;
    }
    // The owner can rewrite the DACL whatever it says.
    at == Where::Folder && !hideout_acl::owner(path).is_ok_and(|o| trusted_writer(&o, me))
}

#[cfg(not(windows))]
fn writable_by_others(_: &std::path::Path, _: u32, _: Where, _: &str) -> bool {
    true
}

#[cfg(windows)]
fn current_sid() -> std::io::Result<String> {
    hideout_acl::current_user_sid()
}

#[cfg(not(windows))]
fn current_sid() -> std::io::Result<String> {
    Err(std::io::Error::other("windows only"))
}

/// An environment folder in the form program paths have: canonical, no `\?\` prefix (so an 8.3
/// `%TEMP%` is compared by its long name).
fn env_path(name: &str) -> Option<String> {
    let v = std::env::var(name).ok().filter(|v| !v.is_empty())?;
    canonical_path(std::path::Path::new(&v)).ok()
}

/// Why `canon` (a canonical folder path) can't be a dev folder, in plain words, or None. Applied
/// when a folder is added and again every time the list is read, so a hand edit or a later ACL
/// change fails closed.
///
/// Refused: a drive root (or `C:`); the user's profile folder (and any folder holding it);
/// `AppData`, the temp folder and Downloads, those folders and everything inside or above them;
/// Windows, Program Files, ProgramData and Public (and everything inside them); a folder that
/// another account can write into or owns, and a folder above it, drive root included, that
/// another account can swap out. "Another account" is anyone but you, SYSTEM, Administrators and
/// TrustedInstaller. Names are compared in canonical long form.
pub fn folder_problem(canon: &str) -> Option<&'static str> {
    let trimmed = canon.trim_end_matches(['\\', '/']);
    if norm(trimmed).len() <= 2 {
        return Some("a drive is too much to trust as a dev folder");
    }
    let profile = env_path("USERPROFILE");
    if profile
        .iter()
        .any(|b| same_path(trimmed, b) || path_within(trimmed, b))
    {
        return Some("that folder holds your whole profile");
    }
    let drop_zones = [
        env_path("APPDATA"),
        env_path("LOCALAPPDATA"),
        env_path("TEMP"),
        env_path("TMP"),
        profile.map(|p| format!("{}\\Downloads", p.trim_end_matches('\\'))),
    ];
    if drop_zones
        .iter()
        .flatten()
        .any(|b| same_path(trimmed, b) || path_within(trimmed, b) || path_within(b, trimmed))
    {
        return Some("AppData, Temp and Downloads can't hold a dev folder");
    }
    let system = [
        env_path("SystemRoot"),
        env_path("ProgramFiles"),
        env_path("ProgramFiles(x86)"),
        env_path("ProgramData"),
        env_path("PUBLIC"),
    ];
    if system
        .iter()
        .flatten()
        .any(|b| same_path(trimmed, b) || path_within(b, trimmed) || path_within(trimmed, b))
    {
        return Some("a system or shared folder can't be a dev folder");
    }
    let Ok(me) = current_sid() else {
        return Some("couldn't tell who you are to check the folder's permissions");
    };
    let mut dir = std::path::Path::new(trimmed);
    if writable_by_others(dir, FOLDER_WRITE, Where::Folder, &me) {
        return Some("other accounts on this PC can change files in that folder");
    }
    while let Some(parent) = dir.parent() {
        let root = parent.parent().is_none();
        let (mask, at) = if root {
            (ROOT_WRITE, Where::Root)
        } else {
            (PARENT_WRITE, Where::Folder)
        };
        if writable_by_others(parent, mask, at, &me) {
            return Some("other accounts on this PC can change a folder above it");
        }
        if root {
            break;
        }
        dir = parent;
    }
    None
}

/// Whether the program `exe` inside the dev folder `folder` can be changed by another account:
/// every directory from the program's own up to `folder`, and the file itself. A project that was
/// moved in from a shared place keeps the permissions it had there, so the folder's own check
/// is not enough. Fails closed on anything that can't be read.
fn exe_open_to_others(folder: &str, exe: &str) -> bool {
    let Ok(me) = current_sid() else {
        return true;
    };
    let exe_path = std::path::Path::new(exe);
    if writable_by_others(exe_path, FOLDER_WRITE, Where::Folder, &me) {
        return true;
    }
    let mut dir = exe_path.parent();
    while let Some(d) = dir {
        if !path_within(folder, &d.display().to_string()) {
            break;
        }
        if writable_by_others(d, FOLDER_WRITE, Where::Folder, &me) {
            return true;
        }
        dir = d.parent();
    }
    false
}

impl Registry {
    /// The dev folders as `hideout.toml` has them on disk now (a file that can't be read gives
    /// none: fail closed).
    pub fn dev_folders(&self) -> Vec<String> {
        HubConfig::load(&self.home.join("hideout.toml"))
            .ok()
            .and_then(Result::ok)
            .map(|h| h.dev_folders())
            .unwrap_or_default()
    }

    /// Developer mode is on, and not only through the legacy Foxglove plugin variable: a plugin
    /// developer who set `FOXGLOVE_DEV_UNSIGNED_PLUGINS` did not ask for dev-folder trust.
    fn dev_folders_on(&self) -> bool {
        let var = |n: &str| std::env::var(n).is_ok_and(|s| s == "1");
        self.dev_mode && !(var("FOXGLOVE_DEV_UNSIGNED_PLUGINS") && !var("HIDEOUT_DEV_UNSIGNED"))
    }

    /// Marks `path` as a dev folder. Needs developer mode; the folder must exist, be a directory,
    /// canonicalize onto this PC's local fixed drive and not be a drive root. Returns the stored
    /// canonical path. Adding one that is already there is fine.
    pub fn add_dev_folder(&mut self, path: &str) -> Result<String, Failure> {
        if !self.dev_folders_on() {
            return Err(crate::permission::io_invalid("developer mode is off"));
        }
        let p = std::path::Path::new(path);
        if !p.is_dir() {
            return Err(crate::permission::io_invalid("that is not a folder"));
        }
        let canon = canonical_path(p)?;
        let probe = format!("{}\\x", canon.trim_end_matches('\\'));
        if !on_local_fixed_drive(&probe) {
            return Err(crate::permission::io_invalid(
                "a dev folder must be on this PC's own drive",
            ));
        }
        if let Some(why) = folder_problem(&canon) {
            return Err(crate::permission::io_invalid(why));
        }
        let mut list = self.dev_folders();
        if !list.iter().any(|f| same_path(f, &canon)) {
            list.push(canon.clone());
            self.change_hub(|h| h.set_dev_folders(&list))?;
        }
        Ok(canon)
    }

    /// Takes `path` off the list. Apps in it stop being carried: their next changed file voids
    /// the grant as for any app.
    ///
    /// The path may be an alias (8.3 name, junction, other spelling) of a listed folder that still
    /// exists. Returns whether a folder was taken off.
    pub fn remove_dev_folder(&mut self, path: &str) -> Result<bool, Failure> {
        let canon = canonical_path(std::path::Path::new(path)).ok();
        let all = self.dev_folders();
        let list: Vec<String> = all
            .iter()
            .filter(|f| !(same_path(f, path) || canon.as_ref().is_some_and(|c| same_path(f, c))))
            .cloned()
            .collect();
        if list.len() == all.len() {
            return Ok(false);
        }
        self.change_hub(|h| h.set_dev_folders(&list))?;
        Ok(true)
    }

    /// The dev folder a canonical program path sits in, now: developer mode on, the path on a
    /// local fixed drive, and strictly inside a listed folder that is still a directory on a
    /// local fixed drive.
    pub fn dev_folder_for(&self, path: &str) -> Option<String> {
        if !self.dev_folders_on() || path.is_empty() || !on_local_fixed_drive(path) {
            return None;
        }
        let folders = self.dev_folders();
        static WARNED: std::sync::Once = std::sync::Once::new();
        if !folders.is_empty() {
            WARNED.call_once(|| {
                log::warn!(
                    "developer mode is on and {} dev folder(s) are listed: apps inside them keep                      their grants over a rebuild",
                    folders.len()
                );
            });
        }
        folders.into_iter().find(|f| {
            path_within(f, path)
                && on_local_fixed_drive(&format!("{}\\x", f.trim_end_matches('\\')))
                && std::path::Path::new(f).is_dir()
                && folder_problem(f).is_none()
                && !exe_open_to_others(f, path)
        })
    }

    /// The dev folder `id` is enrolled in and runs from now, for the **Dev** chip. None when dev
    /// mode is off, the module is not enrolled, or it runs from outside the folder.
    pub fn dev_folder_of(&self, id: &str) -> Option<String> {
        let m = self.inventory.modules.get(id)?;
        let b = m.bound.as_ref()?;
        // Only the place, not the bytes: this runs for every row of the Modules page.
        let exe = crate::adopted_exe(&self.manifest_checked(id).ok()?).ok()?;
        let path = canonical_path(&exe).ok()?;
        self.dev_enrolment(id, m, b, &path)
    }

    /// The enrolment check shared by the chip and the carry: the dev folder that keeps the
    /// binding `b` of `m` for the program now at `path`, or None.
    pub(crate) fn dev_enrolment(
        &self,
        id: &str,
        m: &Installed,
        b: &Bound,
        path: &str,
    ) -> Option<String> {
        let Some(Identity {
            class: IdentityClass::DevFolder {
                folder: Some(enrolled),
            },
            established_by: Some(by),
            ..
        }) = m.identity.as_ref()
        else {
            return None;
        };
        if m.source != ADOPTED
            || by != crate::permission::FOLLOW_SET_BY
            || b.by != crate::permission::FOLLOW_SET_BY
            || b.void
            || b.exe_sha256.is_empty()
            || self.run_args(id) != b.run_args
        {
            return None;
        }
        // The folder the Allow enrolled, still listed, and both the allowed file and the file
        // now there inside it.
        let now = self.dev_folder_for(path)?;
        if !same_path(&now, enrolled) || !path_within(enrolled, &b.exe_path) {
            return None;
        }
        Some(now)
    }
}
