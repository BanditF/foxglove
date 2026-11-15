// SPDX-License-Identifier: MIT
//! CLI grant requests waiting for the user (#1183). `hideout --modules grant` never widens
//! anything: a request that names a list the hub still shows is recorded here, in the hub's
//! memory only, and the Modules page offers it as a header line. Only the user's Allow in the
//! permission dialog (`PermUi`) writes the grant, for the list code the dialog showed. A request
//! is data, not a decision: restarting the hub drops it, and a changed list voids it.

use std::sync::{Arc, Mutex};

/// One queued request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    /// The hub folder the request is for (tests run many hubs in one process).
    pub home: std::path::PathBuf,
    pub id: String,
    /// The list code the CLI caller named: the dialog preselects these items only while the
    /// module's list still has this code.
    pub code: String,
    pub items: Vec<String>,
}

/// The queue. Cloned between the control pipe's thread and the Modules page.
#[derive(Clone, Default)]
pub struct GrantQueue(Arc<Mutex<Vec<Queued>>>);

impl GrantQueue {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Queued>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Records a request; a newer one for the same module replaces the older.
    pub fn submit(&self, q: Queued) {
        let mut v = self.lock();
        v.retain(|o| !(o.home == q.home && o.id == q.id));
        v.push(q);
    }

    /// The requests for one hub folder.
    pub fn for_home(&self, home: &std::path::Path) -> Vec<Queued> {
        self.lock()
            .iter()
            .filter(|q| q.home == home)
            .cloned()
            .collect()
    }

    pub fn get(&self, home: &std::path::Path, id: &str) -> Option<Queued> {
        self.lock()
            .iter()
            .find(|q| q.home == home && q.id == id)
            .cloned()
    }

    /// Drops the request for a module (answered, refused or stale).
    pub fn clear(&self, home: &std::path::Path, id: &str) {
        self.lock().retain(|q| !(q.home == home && q.id == id));
    }
}

/// The hub process's one queue, shared by the control pipe and the Modules page.
pub fn shared() -> &'static GrantQueue {
    static Q: std::sync::OnceLock<GrantQueue> = std::sync::OnceLock::new();
    Q.get_or_init(GrantQueue::default)
}
