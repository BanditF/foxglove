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
pub mod contain;
pub mod diagnostics;
#[cfg(windows)]
pub mod driver;
#[cfg(windows)]
pub mod finder;
pub mod instances;
pub mod pin;
pub mod policy;
#[cfg(windows)]
pub mod sandbox;
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

/// Now as `YYYY-MM-DDTHH:MM:SSZ` (UTC), for the grant records.
pub fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
