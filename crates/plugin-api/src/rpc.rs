//! JSON-RPC 2.0, one message per line (UTF-8, `\n`-terminated, no embedded newlines).
//!
//! Requests carry an `id` and get exactly one response. Notifications have no `id`: the app
//! sends events this way, and clients may send them for fire-and-forget calls.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Longest line either side accepts; longer ones are refused (a panel bitmap or an art push fits).
pub const MAX_LINE: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    Text(String),
}

/// A request (`id` set) or a notification (`id` absent).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    pub jsonrpc: JsonRpc,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Response {
    pub jsonrpc: JsonRpc,
    pub id: Id,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Result(Value),
    Error(Error),
}

/// Always `"2.0"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum JsonRpc {
    #[default]
    #[serde(rename = "2.0")]
    V2,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Error {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Error codes: JSON-RPC's own, then Foxglove's (which mirror the device layer's errors).
pub mod code {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL: i32 = -32603;
    /// The device or the app can't do this.
    pub const UNSUPPORTED: i32 = 1;
    /// The device isn't connected.
    pub const DISCONNECTED: i32 = 2;
    /// Something else is using it; try again.
    pub const BUSY: i32 = 3;
    pub const TIMEOUT: i32 = 4;
    /// Not permitted: the client lacks the permission, or the action needs the user's click.
    pub const DENIED: i32 = 5;
    /// The device or backend failed; the message says how.
    pub const BACKEND: i32 = 6;
    /// The client's API major version isn't served.
    pub const VERSION: i32 = 7;
    /// Sent too much too fast; slow down.
    pub const RATE_LIMITED: i32 = 8;
}

impl Request {
    pub fn call(id: i64, method: &str, params: Value) -> Request {
        Request {
            jsonrpc: JsonRpc::V2,
            method: method.into(),
            params,
            id: Some(Id::Number(id)),
        }
    }

    pub fn notification(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: JsonRpc::V2,
            method: method.into(),
            params,
            id: None,
        }
    }
}

impl Response {
    pub fn ok(id: Id, result: Value) -> Response {
        Response {
            jsonrpc: JsonRpc::V2,
            id,
            outcome: Outcome::Result(result),
        }
    }

    pub fn error(id: Id, code: i32, message: impl Into<String>) -> Response {
        Response {
            jsonrpc: JsonRpc::V2,
            id,
            outcome: Outcome::Error(Error {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_format_matches_json_rpc() {
        let r = Request::call(1, "facts.get", json!({"key": "clock.time"}));
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"jsonrpc": "2.0", "method": "facts.get", "params": {"key": "clock.time"}, "id": 1})
        );
        let n: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"facts.changed","params":{}}"#)
                .unwrap();
        assert_eq!(n.id, None);
        let e = Response::error(Id::Text("a".into()), code::DENIED, "needs lighting.write");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"jsonrpc": "2.0", "id": "a", "error": {"code": 5, "message": "needs lighting.write"}})
        );
        let ok: Response =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":2,"result":null}"#).unwrap();
        assert_eq!(ok.outcome, Outcome::Result(Value::Null));
        assert!(serde_json::from_str::<Request>(r#"{"jsonrpc":"1.0","method":"x"}"#).is_err());
    }
}
