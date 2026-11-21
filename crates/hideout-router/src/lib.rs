// SPDX-License-Identifier: MIT
//! Hideout's IPC router (docs/hideout-contract.md §8), clean-room MIT.
//!
//! - [`Router`] is the whole routing logic with no I/O: lines in, [`Out`] actions out, the clock
//!   passed in. It holds the launch tokens, the provider table, the facts cache, the
//!   subscriptions, the pending calls and their 30 s deadlines, and checks every call against
//!   the caller's grant (§6, grants from `hideout-registry`).
//! - [`server::Hub`] drives a router over real connections: the router pipe
//!   `\\.\pipe\hideout-<user>` with a current-user ACL, the old `\\.\pipe\foxglove-<user>` as an
//!   alias (§8.2, §8.6), and the legacy stdio bridge for `transport = "stdio"` plugins.

mod alias;
pub use alias::{plan as alias_plan, AliasOutcome, AliasPlan};
mod channels;
pub mod checks;
mod router;
pub mod server;
pub mod tool_id;
pub use tool_id::ToolId;
mod tool_queue;
pub use tool_queue::{AskItem, ToolAsk, ITEMS_MAX, QUEUE_MAX};

pub mod secrets;
pub use router::{
    fresh_secret, host_methods, Approver, ConnId, IdentityLookup, LaunchCheck, ModuleEntry, Origin,
    Out, Router, ServiceCall, ServiceError, Settings, Triggers, APPROVED_MAX, CALL_TIMEOUT,
    HOST_CONSUMER, MAX_HUB_QUEUED, MAX_INFLIGHT_PER_CALLER, MAX_INFLIGHT_PER_PROVIDER,
    MAX_LEASES_PER_CALLER, NOT_STARTED, SHOW_GAP,
};
