"""A Hideout module's side of the wire (docs/hideout-contract.md §7.5, §7.6, §8), stdlib only.

JSON-RPC 2.0, one UTF-8 message per line, at most 256 KiB (§8.1), over any binary reader and
writer: the router pipe (`connect_env`), or in-process buffers in tests. The same shape as the
Rust SDK (`foxglove_plugin::hideout`):

- calls the module makes are typed (`initialize`, `register`, `ui_register`, `ready`, `ui_patch`,
  `idle`, ...) and return the request id; `call` blocks for the answer;
- calls the hub makes go to a `Handler`, with params parsed into the generated models
  (`models.py`, from schema/hideout-v1.json); `handle` answers them, `run` loops until the pipe
  closes or `module.exit` arrives.
"""

import json
import os
import re

from . import models
from ._model import Model, ModelError, encode

MAX_MESSAGE = 256 * 1024

PARSE, INVALID_REQUEST, METHOD_NOT_FOUND, INVALID_PARAMS, INTERNAL = -32700, -32600, -32601, -32602, -32603
UNSUPPORTED, DISCONNECTED, BUSY, TIMEOUT, DENIED, BACKEND, VERSION, RATE_LIMITED = 1, 2, 3, 4, 5, 6, 7, 8


class RpcError(Exception):
    """A JSON-RPC error, either side's; `data.reason` is plain words for the UI (§8.1)."""

    def __init__(self, code, message, data=None):
        super().__init__(f"error {code}: {message}")
        self.code, self.message, self.data = code, message, data

    def to_json(self):
        e = {"code": self.code, "message": self.message}
        if self.data is not None:
            e["data"] = self.data
        return e

    @classmethod
    def from_json(cls, e):
        return cls(e.get("code", INTERNAL), e.get("message", ""), e.get("data"))


class SessionClosed(Exception):
    """The hub closed the pipe (§7.9: a 30-second grace applies)."""


class Handler:
    """The hub's calls on a module. Override what the module supports."""

    def drain(self, drain: models.ModuleDrain):
        """`module.drain`: a DrainResultDone, or DrainResultBusy(reason=...) and `idle()` later."""
        return models.DrainResultDone()

    def state_export(self):
        """`state.export` (handover): the blob (at most 256 KiB) or None."""
        return None

    def state_import(self, params):
        return {}

    def ui_event(self, event: models.UiEvent):
        return {}

    def window_open(self, open_: models.UiWindowOpen):
        return {}

    def action(self, action: models.ActionInvoke):
        return {}

    def settings_get(self, params):
        return {}

    def settings_set(self, set_: models.SettingsSet):
        return {}

    def channel_create(self, create: models.ChannelCreate):
        """`channel.create` (§8.4): make a one-time pipe for the consumer and answer {pipe}:
        `{"pipe": channel.serve(module_id, create.channel, create.nonce, stream)}`."""
        raise RpcError(UNSUPPORTED, "no channels")

    def exit(self):
        """`module.exit`: finish up and exit with code 0."""

    def notification(self, method, params):
        """Any other notification (`facts.changed`, ...)."""

    def call(self, method, params):
        """Any other request: a provided method."""
        raise RpcError(METHOD_NOT_FOUND, f"no method {method}")


# Hub calls whose params have a model, and the handler method they go to.
_TYPED = {
    "module.drain": ("drain", models.ModuleDrain),
    "ui.event": ("ui_event", models.UiEvent),
    "ui.window.open": ("window_open", models.UiWindowOpen),
    "action.invoke": ("action", models.ActionInvoke),
    "settings.set": ("settings_set", models.SettingsSet),
    "channel.create": ("channel_create", models.ChannelCreate),
}
_UNTYPED = {"state.import": "state_import", "settings.get": "settings_get"}


def router_pipe_name(user):
    """`\\\\.\\pipe\\hideout-<user>`, keeping ASCII letters, digits, - and _ (§8.2)."""
    return r"\\.\pipe\hideout-" + re.sub(r"[^A-Za-z0-9_-]", "", user)


def launch_from_env(env=os.environ):
    """(pipe, token, instance) the hub set at launch (§7.5), or None."""
    values = [env.get(k) for k in ("HIDEOUT_PIPE", "HIDEOUT_TOKEN", "HIDEOUT_INSTANCE")]
    return tuple(values) if all(values) else None


def connect_env():
    """Opens the router pipe the hub named; returns (Session, (pipe, token, instance))."""
    launch = launch_from_env()
    if launch is None:
        raise RuntimeError("HIDEOUT_PIPE, HIDEOUT_TOKEN and HIDEOUT_INSTANCE are not set: the hub starts modules")
    pipe = open(launch[0], "r+b", buffering=0)
    return Session(pipe, pipe), launch


def _dump(value):
    return encode(value.to_json() if isinstance(value, Model) else value)


class Session:
    def __init__(self, reader, writer):
        self.reader, self.writer = reader, writer
        self.next_id = 1
        self.pending = {}

    def _write(self, msg):
        line = json.dumps(msg, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        if len(line) > MAX_MESSAGE:
            raise ValueError(f"a message is at most {MAX_MESSAGE} bytes")
        self.writer.write(line + b"\n")
        if hasattr(self.writer, "flush"):
            self.writer.flush()

    def request(self, method, params=None):
        id_ = self.next_id
        self.next_id += 1
        self._write({"jsonrpc": "2.0", "id": id_, "method": method, "params": _dump(params or {})})
        self.pending[id_] = method
        return id_

    def notify(self, method, params=None):
        self._write({"jsonrpc": "2.0", "method": method, "params": _dump(params or {})})

    def initialize(self, params: models.InitializeModule):
        return self.request("initialize", params)

    def register(self, params: models.ModuleRegister):
        return self.request("module.register", params)

    def ui_register(self, params: models.Register):
        return self.request("ui.register", params)

    def ready(self, instance):
        self.notify("module.ready", models.ModuleReady(instance=instance))

    def ui_patch(self, params: models.UiPatch):
        self.notify("ui.patch", params)

    def asset_put(self, params: models.UiAssetPut):
        return self.request("ui.asset.put", params)

    def hub_notify(self, params: models.HubNotify):
        return self.request("hub.notify", params)

    def idle(self):
        """`module.idle` (§7.6): the work a busy drain waited for has ended."""
        self.notify("module.idle", {})

    def set_fact(self, params):
        return self.request("facts.set", params)

    def channel_open(self, params: models.ChannelOpen):
        return self.request("channel.open", params)

    def handle(self, msg, handler):
        """Handles one message from the hub. Returns ("response", id, method, result | RpcError),
        ("answered", method) or ("notified", method)."""
        if msg.get("jsonrpc") != "2.0":
            raise ValueError('jsonrpc must be "2.0"')
        method = msg.get("method")
        if method is None:
            id_ = msg.get("id")
            if id_ not in self.pending:
                raise ValueError(f"no request {id_} is waiting")
            sent = self.pending.pop(id_)
            if "error" in msg:
                return ("response", id_, sent, RpcError.from_json(msg["error"]))
            return ("response", id_, sent, msg["result"])
        params = msg.get("params", {})
        if "id" not in msg:
            if method == "module.exit":
                handler.exit()
            else:
                handler.notification(method, params)
            return ("notified", method)
        try:
            result = _dump(self._dispatch(handler, method, params))
            reply = {"jsonrpc": "2.0", "id": msg["id"], "result": result}
        except RpcError as e:
            reply = {"jsonrpc": "2.0", "id": msg["id"], "error": e.to_json()}
        self._write(reply)
        return ("answered", method)

    @staticmethod
    def _dispatch(handler, method, params):
        if method in _TYPED:
            name, model = _TYPED[method]
            try:
                parsed = model.from_json(params)
            except ModelError as e:
                raise RpcError(INVALID_PARAMS, str(e)) from None
            return getattr(handler, name)(parsed)
        if method == "state.export":
            return handler.state_export()
        if method in _UNTYPED:
            return getattr(handler, _UNTYPED[method])(params)
        return handler.call(method, params)

    def read(self):
        """The next message from the hub, or None when the pipe closed."""
        while True:
            line = self.reader.readline(MAX_MESSAGE + 2)
            if not line:
                return None
            if line.strip():
                return json.loads(line)

    def call(self, method, params, handler):
        """Sends a request and waits for its answer, handling hub calls meanwhile."""
        id_ = self.request(method, params)
        while True:
            msg = self.read()
            if msg is None:
                raise SessionClosed()
            got = self.handle(msg, handler)
            if got[0] == "response" and got[1] == id_:
                if isinstance(got[3], RpcError):
                    raise got[3]
                return got[3]

    def run(self, handler):
        """Answers hub calls until the pipe closes or `module.exit` arrives."""
        while (msg := self.read()) is not None:
            if self.handle(msg, handler) == ("notified", "module.exit"):
                return
