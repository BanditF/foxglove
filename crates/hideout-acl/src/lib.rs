// SPDX-License-Identifier: MIT
//! Current-user-only ACLs (docs/hideout-contract.md §2.3: "every file and pipe here MUST have a
//! current-user-only ACL").
//!
//! - [`Descriptor::current_user_only`]: full access for the current user's SID alone, protected
//!   (the channel and router pipes, #929).
//! - [`Descriptor::private_tree`]: the current user, SYSTEM and Administrators, protected, and
//!   inherited by what is created inside (the hub's config folder).
//! - [`ensure_private_dir`] fixes an existing folder (returns whether it changed anything),
//!   [`create_private_dir_all`] makes a missing one private on creation, [`dacl`] reads a DACL.
//!
//! On other platforms the folder helpers only create the folder.

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
mod other {
    use std::{io, path::Path};
    pub fn ensure_private_dir(_path: &Path) -> io::Result<bool> {
        Ok(false)
    }
    pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }
}
#[cfg(not(windows))]
pub use other::*;
