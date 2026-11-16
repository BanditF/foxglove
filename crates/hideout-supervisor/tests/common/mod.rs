// SPDX-License-Identifier: MIT
//! Shared by the tests that start real modules (a windowless `ping.exe` or the test hub's own
//! helper): a supervisor that ends what it started when it goes out of scope, panic or not.
//!
//! Why: Rust's `Command` lets a child inherit the parent's inheritable handles, and under
//! `cargo nextest` (process per test) those include the pipes nextest reads the test's output
//! from. A module still running when the test ends keeps them open, and nextest reports the
//! test as `LEAK` (a `FAIL + LEAK` if an assertion failed before the test's own cleanup).
//! `cargo test` shares one pipe set for the whole binary, so it never showed.
#![allow(dead_code)]

use std::ops::Deref;
use std::path::{Path, PathBuf};

use hideout_router::server::Hub;
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::win::{Job, Process};

/// A [`Supervisor`] that, when dropped, kills the job of every instance it recorded under its
/// home folder and waits for the processes to end. Derefs to the supervisor.
pub struct Reaped {
    sup: Supervisor,
    home: PathBuf,
}

/// [`Supervisor::start`] with the reaping guard.
pub fn start(home: &Path, hub: Hub) -> Reaped {
    Reaped {
        sup: Supervisor::start(home, hub),
        home: home.to_path_buf(),
    }
}

impl Deref for Reaped {
    type Target = Supervisor;
    fn deref(&self) -> &Supervisor {
        &self.sup
    }
}

impl Drop for Reaped {
    fn drop(&mut self) {
        let records = Instances::load(&self.home);
        // Handles first (by the supervisor's live pids, never a stale recorded one), then the
        // jobs by name, then wait: nothing of the test's modules outlives the test.
        let live: Vec<Process> = records
            .modules
            .keys()
            .filter_map(|id| self.sup.pid(id))
            .filter_map(|pid| Process::open(pid).ok())
            .collect();
        for rec in records.modules.values() {
            if let Ok(job) = Job::open(&rec.job) {
                job.kill(1);
            }
        }
        for p in &live {
            p.kill(1);
            let _ = p.wait();
        }
    }
}
