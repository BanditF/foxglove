// SPDX-License-Identifier: MIT
//! The grant step of a direct channel (§8.4): the router brokers `channel.open` and
//! `channel.create`, checks the provider's `{pipe}` answer, and steps out. Frames never pass
//! through it: they flow on the provider's own one-time pipe.

use hideout_contract::channel::is_channel_pipe;
use serde_json::{json, Value};

/// Error 6, backend (§8.1).
const BACKEND: i64 = 6;

/// The consumer's answer from the provider's `channel.create` result: `{pipe, nonce}` when the
/// pipe is a local channel pipe (v1.4, #977), else error 6 with `data.id` `channel.pipe`, so a
/// consumer is never sent to another machine's pipe or to the router's own.
pub(crate) fn grant(
    result: Result<Value, Value>,
    nonce: &str,
    provider: &str,
) -> Result<Value, Value> {
    let r = result?;
    match r.get("pipe").and_then(Value::as_str) {
        Some(pipe) if is_channel_pipe(pipe) => Ok(json!({"pipe": pipe, "nonce": nonce})),
        _ => {
            log::warn!("router: {provider} answered channel.create with a bad pipe: {r}");
            Err(json!({
                "code": BACKEND,
                "message": "backend",
                "data": {
                    "id": "channel.pipe",
                    "reason": format!(
                        "{provider} offered a channel Hideout won't connect to. Update {provider}."
                    ),
                },
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_channel_pipes_pass() {
        let ok = grant(
            Ok(json!({"pipe": r"\\.\pipe\hideout-ch-1-0-a-b"})),
            "n",
            "A",
        );
        assert_eq!(ok.unwrap()["nonce"], "n");
        for bad in [
            json!({"pipe": r"\\host\pipe\hideout-ch-x"}),
            json!({}),
            json!({"pipe": 3}),
        ] {
            let e = grant(Ok(bad), "n", "A").unwrap_err();
            assert_eq!(e["data"]["id"], "channel.pipe");
        }
        let passed = grant(Err(json!({"code": 1})), "n", "A").unwrap_err();
        assert_eq!(passed["code"], 1, "a provider's own error passes unchanged");
    }
}
