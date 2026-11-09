//! A sample Hideout module (docs/hideout-contract.md §7.5, §7.6, §5): `example.counter`.
//!
//! [`Counter`] is the module: one page panel with a count and an Add button, a busy drain while a
//! count-down runs, and its count handed over through `state.export`/`state.import`.
//! [`Scripted`] answers the hub from a transcript's module side, for the replay test
//! (tests/transcripts.rs).

use foxglove_plugin::hideout::{
    ActionInvoke, ChannelCreate, DrainResult, Handler, ModuleDrain, ModuleRegister, Node, PatchOp,
    PatchOpKind, Register, RpcError, SettingsSet, UiEvent, UiPatch, UiWindowOpen,
};
use serde_json::{json, Value};

pub const ID: &str = "example.counter";

/// The sample module's state.
#[derive(Default)]
pub struct Counter {
    pub count: i64,
    /// A count-down is running: a drain answers busy until it ends.
    pub counting_down: bool,
    /// A patch to send after the current message (`ui.patch` is the module's to send).
    pub patch: Option<UiPatch>,
    pub exit: bool,
}

impl Counter {
    pub fn registration() -> ModuleRegister {
        ModuleRegister {
            methods: vec!["example.counter.*".into()],
            ..Default::default()
        }
    }

    pub fn panels(&self, instance: &str) -> Register {
        let node = |kind: &str, id: &str, props: Value| Node {
            kind: kind.into(),
            id: id.into(),
            props: props.as_object().cloned().unwrap_or_default(),
            bind: None,
            stream: None,
            children: vec![],
        };
        let mut root = node("column", "root", json!({}));
        root.children = vec![
            node("label", "count", json!({"text": self.count.to_string()})),
            node("button", "add", json!({"label": "Add"})),
        ];
        serde_json::from_value(json!({
            "instance": instance,
            "panels": [{"id": "counter", "slot": "page", "title": "Counter", "icon": "plus", "tree": root}],
        }))
        .expect("a valid ui.register")
    }

    fn show_count(&mut self) {
        self.patch = Some(UiPatch {
            panel: "counter".into(),
            ops: vec![PatchOp {
                op: PatchOpKind::Set,
                node: "count".into(),
                props: json!({"text": self.count.to_string()}).as_object().cloned(),
                index: None,
                tree: None,
            }],
        });
    }
}

impl Handler for Counter {
    fn drain(&mut self, _drain: ModuleDrain) -> Result<DrainResult, RpcError> {
        Ok(if self.counting_down {
            DrainResult::Busy {
                reason: Some("a count-down".into()),
            }
        } else {
            DrainResult::Done
        })
    }

    fn state_export(&mut self) -> Result<Option<Value>, RpcError> {
        Ok(Some(json!({"blob": {"count": self.count}})))
    }

    fn state_import(&mut self, params: Value) -> Result<Value, RpcError> {
        self.count = params["blob"]["count"].as_i64().unwrap_or(0);
        Ok(json!({}))
    }

    fn ui_event(&mut self, event: UiEvent) -> Result<Value, RpcError> {
        if (event.panel.as_str(), event.node.as_str()) != ("counter", "add") {
            return Err(RpcError::new(1, "no such control"));
        }
        self.count += 1;
        self.show_count();
        Ok(json!({}))
    }

    fn action(&mut self, action: ActionInvoke) -> Result<Value, RpcError> {
        match action.action.as_str() {
            "reset" => {
                self.count = 0;
                self.show_count();
                Ok(json!({}))
            }
            _ => Err(RpcError::new(1, "no such action")),
        }
    }

    fn exit(&mut self) {
        self.exit = true;
    }
}

/// Answers each hub call with the module's answer from a transcript: set [`Scripted::answer`]
/// before the call is handled. The SDK still parses the params into the contract's types and
/// encodes the typed result, so the replay checks both directions of the wire.
#[derive(Default)]
pub struct Scripted {
    pub answer: Option<Result<Value, RpcError>>,
    pub notifications: Vec<String>,
}

impl Scripted {
    fn next(&mut self) -> Result<Value, RpcError> {
        self.answer
            .take()
            .unwrap_or_else(|| Err(RpcError::new(-32603, "no scripted answer")))
    }

    fn typed<T: serde::de::DeserializeOwned>(&mut self) -> Result<T, RpcError> {
        let v = self.next()?;
        serde_json::from_value(v).map_err(|e| RpcError::new(-32603, e.to_string()))
    }
}

impl Handler for Scripted {
    fn drain(&mut self, _d: ModuleDrain) -> Result<DrainResult, RpcError> {
        self.typed()
    }
    fn state_export(&mut self) -> Result<Option<Value>, RpcError> {
        self.typed()
    }
    fn state_import(&mut self, _p: Value) -> Result<Value, RpcError> {
        self.next()
    }
    fn ui_event(&mut self, _e: UiEvent) -> Result<Value, RpcError> {
        self.next()
    }
    fn window_open(&mut self, _o: UiWindowOpen) -> Result<Value, RpcError> {
        self.next()
    }
    fn action(&mut self, _a: ActionInvoke) -> Result<Value, RpcError> {
        self.next()
    }
    fn settings_get(&mut self, _p: Value) -> Result<Value, RpcError> {
        self.next()
    }
    fn settings_set(&mut self, _s: SettingsSet) -> Result<Value, RpcError> {
        self.next()
    }
    fn channel_create(&mut self, _c: ChannelCreate) -> Result<Value, RpcError> {
        self.next()
    }
    fn exit(&mut self) {
        self.notifications.push("module.exit".into());
    }
    fn notification(&mut self, method: &str, _p: Value) {
        self.notifications.push(method.into());
    }
    fn call(&mut self, _method: &str, _p: Value) -> Result<Value, RpcError> {
        self.next()
    }
}
