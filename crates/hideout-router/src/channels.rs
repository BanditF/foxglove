// SPDX-License-Identifier: MIT
//! The grant step of a direct channel (§8.4): the router brokers `channel.open` and
//! `channel.create`, checks the provider's `{pipe}` answer, and steps out. Frames never pass
//! through it: they flow on the provider's own one-time pipe.

use hideout_contract::channel::{app_container_of, is_channel_pipe};
use serde_json::{json, Value};

/// Error 6, backend (§8.1).
const BACKEND: i64 = 6;

/// The provider connection a `channel.create` answer came from.
pub(crate) struct Provider {
    /// Its display name, for refusals.
    pub name: String,
    /// Its process id, when the transport knows it: the consumer checks the pipe's server.
    pub pid: Option<u32>,
    /// Its AppContainer SID, when it runs in one (§7.7).
    pub app_container: Option<String>,
}

/// The consumer's answer from the provider's `channel.create` result: `{pipe, nonce}` (and
/// `pid`, the provider's, when known: v1.10, #1092) when the pipe is a local channel pipe
/// (v1.4, #977), else error 6 with `data.id` `channel.pipe`, so a consumer is never sent to
/// another machine's pipe or to the router's own. A contained provider's pipe must be in its
/// own container's namespace, and only its pipe may be: a provider can't send a consumer to
/// another module's pipe that way.
pub(crate) fn grant(
    result: Result<Value, Value>,
    nonce: &str,
    provider: &Provider,
) -> Result<Value, Value> {
    let r = result?;
    let pipe = r.get("pipe").and_then(Value::as_str);
    // A contained provider can make pipes only in its own namespace, so any other name from it
    // is someone else's pipe; a provider outside one never names a container's.
    let ok = pipe.is_some_and(|p| {
        is_channel_pipe(p) && app_container_of(p) == provider.app_container.as_deref()
    });
    match pipe {
        Some(pipe) if ok => {
            let mut g = json!({"pipe": pipe, "nonce": nonce});
            if let Some(pid) = provider.pid {
                g["pid"] = json!(pid);
            }
            Ok(g)
        }
        _ => {
            let name = &provider.name;
            log::warn!("router: {name} answered channel.create with a bad pipe: {r}");
            Err(json!({
                "code": BACKEND,
                "message": "backend",
                "data": {
                    "id": "channel.pipe",
                    "reason": format!(
                        "{name} offered a channel Hideout won't connect to. Update {name}."
                    ),
                },
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Provider {
        Provider {
            name: "A".into(),
            pid: None,
            app_container: None,
        }
    }

    #[test]
    fn only_local_channel_pipes_pass() {
        let ok = grant(
            Ok(json!({"pipe": r"\\.\pipe\hideout-ch-1-0-a-b"})),
            "n",
            &plain(),
        );
        let ok = ok.unwrap();
        assert_eq!(ok["nonce"], "n");
        assert!(ok.get("pid").is_none(), "no pid when the router has none");
        for bad in [
            json!({"pipe": r"\\host\pipe\hideout-ch-x"}),
            json!({}),
            json!({"pipe": 3}),
        ] {
            let e = grant(Ok(bad), "n", &plain()).unwrap_err();
            assert_eq!(e["data"]["id"], "channel.pipe");
        }
        let passed = grant(Err(json!({"code": 1})), "n", &plain()).unwrap_err();
        assert_eq!(passed["code"], 1, "a provider's own error passes unchanged");
    }

    /// #1092: the provider's pid goes to the consumer, and a pipe in an AppContainer's
    /// namespace passes only from that container.
    #[test]
    fn a_contained_providers_pipe_is_its_own() {
        let mine = r"\\.\pipe\Sessions\1\AppContainerNamedObjects\S-1-15-2-7-8\hideout-ch-1-0-a-b";
        let theirs =
            r"\\.\pipe\Sessions\1\AppContainerNamedObjects\S-1-15-2-9-9\hideout-ch-1-0-a-b";
        let p = Provider {
            name: "A".into(),
            pid: Some(42),
            app_container: Some("S-1-15-2-7-8".into()),
        };
        let g = grant(Ok(json!({ "pipe": mine })), "n", &p).unwrap();
        assert_eq!(g["pid"], 42);
        assert_eq!(g["pipe"], mine);
        // Another container's pipe, from a contained provider or from one outside any.
        assert!(grant(Ok(json!({ "pipe": theirs })), "n", &p).is_err());
        assert!(grant(Ok(json!({ "pipe": mine })), "n", &plain()).is_err());
        // A contained provider can't make a plain pipe: one it names is someone else's.
        assert!(grant(Ok(json!({"pipe": r"\\.\pipe\hideout-ch-1"})), "n", &p).is_err());
    }
}
