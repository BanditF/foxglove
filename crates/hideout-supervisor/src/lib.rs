// SPDX-License-Identifier: MIT
//! The Hideout hub's light supervisor (docs/hideout-contract.md §7.5, §7.7 to §7.9), clean-room
//! MIT.
//!
//! - [`policy::Policy`] holds every decision with no I/O: states (§7.8), the fixed backoff
//!   (1, 2, 4 … 60 s), the stop after five crashes in ten minutes, the 30 s ready timer, the
//!   grace period for modules taken back after a hub restart, Start again and the recovery
//!   ladder, and the automatic rollback within ten minutes of an update.
//! - [`words`] is every string a person reads (docs/hideout-supervisor-ux.md).
//! - [`driver::Supervisor`] (Windows) carries it out: a named Job Object per instance, the
//!   launch check the router runs at `initialize` (pid and exe hash, §8.2 v1.3),
//!   `run\instances.json` and adoption.

pub mod bisect;
pub mod closes;
pub mod diagnostics;
#[cfg(windows)]
pub mod driver;
#[cfg(windows)]
pub mod finder;
pub mod instances;
pub mod policy;
#[cfg(windows)]
pub mod win;
pub mod words;

use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

/// SHA-256 of a file, lowercase hex.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
