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

    /// Marks `path` as a dev folder. Needs developer mode; the folder must exist, be a directory,
    /// canonicalize onto this PC's local fixed drive and not be a drive root. Returns the stored
    /// canonical path. Adding one that is already there is fine.
    pub fn add_dev_folder(&mut self, path: &str) -> Result<String, Failure> {
        if !self.dev_mode {
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
        if canon.trim_end_matches('\\').len() <= 2 {
            return Err(crate::permission::io_invalid(
                "a drive is too much to trust as a dev folder",
            ));
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
    pub fn remove_dev_folder(&mut self, path: &str) -> Result<(), Failure> {
        let list: Vec<String> = self
            .dev_folders()
            .into_iter()
            .filter(|f| !same_path(f, path))
            .collect();
        self.change_hub(|h| h.set_dev_folders(&list))
    }

    /// The dev folder a canonical program path sits in, now: developer mode on, the path on a
    /// local fixed drive, and strictly inside a listed folder that is still a directory on a
    /// local fixed drive.
    pub fn dev_folder_for(&self, path: &str) -> Option<String> {
        if !self.dev_mode || path.is_empty() || !on_local_fixed_drive(path) {
            return None;
        }
        self.dev_folders().into_iter().find(|f| {
            path_within(f, path)
                && on_local_fixed_drive(&format!("{}\\x", f.trim_end_matches('\\')))
                && std::path::Path::new(f).is_dir()
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
        self.dev_enrolment(m, b, &path)
    }

    /// The enrolment check shared by the chip and the carry: the dev folder that keeps the
    /// binding `b` of `m` for the program now at `path`, or None.
    pub(crate) fn dev_enrolment(&self, m: &Installed, b: &Bound, path: &str) -> Option<String> {
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
