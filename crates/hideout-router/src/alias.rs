//! The `foxglove-<user>` alias and the one-owner rule (contract §8.6, #1143 P4).
//!
//! The alias is served only when nobody is on the name. A live `foxglove.exe` (identity
//! verified by process, never by a reply: see `foxglove_plugin::owner`) keeps it: the hub says so
//! in its log and keeps its own `hideout-<user>` pipe. A server that is not `foxglove.exe` is
//! ignored the same way: the hub claims nothing next to it.

#[allow(unused_imports)]
use foxglove_plugin::owner::{self, Expect, Probe};
#[allow(unused_imports)]
use std::time::Duration;

/// What to do with the alias name after a probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasPlan {
    /// Nobody is on the name: serve it.
    Serve,
    /// A verified foxglove.exe serves it: leave it alone.
    AppOwns { pid: u32 },
    /// Something else is on it, or it could not be looked at: claim nothing.
    Ignore { why: String },
}

/// The plan for one probe result.
pub fn plan(p: &Probe) -> AliasPlan {
    match p {
        Probe::Free => AliasPlan::Serve,
        Probe::Verified { pid, .. } => AliasPlan::AppOwns { pid: *pid },
        Probe::Foreign { pid, why } => AliasPlan::Ignore {
            why: match pid {
                Some(pid) => format!("pid {pid}: {why}"),
                None => why.clone(),
            },
        },
    }
}

/// How a [`serve_alias`] attempt ended.
#[derive(Debug)]
pub enum AliasOutcome {
    Served,
    /// Not served on purpose (the plan said so); the string is the log line.
    Left(String),
    /// The plan said serve, but the name was taken in between (or serving failed).
    Failed(std::io::Error),
}

#[cfg(windows)]
impl crate::server::Hub {
    /// Probes `name` (at most `timeout`) for a server that must be `expect`, then serves it only
    /// if nobody is there. The hub's own router pipe is not touched.
    pub fn serve_alias_checked(
        &self,
        name: &str,
        expect: Expect,
        timeout: Duration,
    ) -> AliasOutcome {
        match plan(&owner::probe(name, expect, timeout)) {
            AliasPlan::Serve => match self.serve_pipe(name) {
                Ok(_) => AliasOutcome::Served,
                Err(e) => AliasOutcome::Failed(e),
            },
            AliasPlan::AppOwns { pid } => AliasOutcome::Left(format!(
                "{} is served by {} (pid {pid}); the hub keeps its own pipe and leaves it",
                name, expect.exe
            )),
            AliasPlan::Ignore { why } => AliasOutcome::Left(format!(
                "{name} is held by a server that is not {} ({why}); ignored, nothing claimed",
                expect.exe
            )),
        }
    }

    /// [`Hub::serve_alias_checked`] for the real alias: `foxglove.exe`, within the probe limit.
    pub fn serve_alias(&self, name: &str) -> AliasOutcome {
        self.serve_alias_checked(name, Expect::FOXGLOVE, owner::TIMEOUT)
    }
}
