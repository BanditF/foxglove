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
import threading

from . import models
from ._model import Model, ModelError, encode

MAX_MESSAGE = 256 * 1024

PARSE, INVALID_REQUEST, METHOD_NOT_FOUND, INVALID_PARAMS, INTERNAL = -32700, -32600, -32601, -32602, -32603
UNSUPPORTED, DISCONNECTED, BUSY, TIMEOUT, DENIED, BACKEND, VERSION, RATE_LIMITED = 1, 2, 3, 4, 5, 6, 7, 8
# `module.grants` (H -> M, v1.22): an Allow widened the module's grants while it runs.
MODULE_GRANTS = "module.grants"
# Denied subscriptions a session keeps to send again (the oldest goes first).
MAX_DENIED_SUBS = 64


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
        `{"pipe": channel.serve_create(module_id, create, stream)}`, which admits only the
        consumer the hub named (v1.10, #1092)."""
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


def hub_pid_from_env(env=os.environ):
    """`HIDEOUT_HUB_PID` (§7.5, v1.10, #983): the hub's process id, or None (an older hub)."""
    try:
        return int(env.get("HIDEOUT_HUB_PID", "").strip())
    except ValueError:
        return None


def open_router_pipe(name, hub_pid=None):
    """Opens the router pipe `name` as a binary reader and writer. On Windows it is overlapped, so
    a thread blocked in a read never delays another thread's write (the Rust SDK's #1338), with
    client rights only (what a module in an AppContainer is let in with, §7.7) and
    identification-level impersonation;
    with `hub_pid`, nothing can be sent before the pipe's server is known to be that process
    (PermissionError otherwise, §7.5): another process may have made an instance of the name."""
    if os.name != "nt":
        return open(name, "r+b", buffering=0)
    from . import channel

    h = channel.open_pipe(name, overlapped=True)
    try:
        if hub_pid is not None:
            channel.check_server(h, hub_pid)
    except BaseException:
        channel.close_handle(h)
        raise
    return channel.RouterPipe(h)


def connect_env():
    """Opens the router pipe the hub named; returns (Session, (pipe, token, instance)). Nothing
    is sent, the token least of all, unless the pipe's server is `HIDEOUT_HUB_PID`."""
    launch = launch_from_env()
    if launch is None:
        raise RuntimeError("HIDEOUT_PIPE, HIDEOUT_TOKEN and HIDEOUT_INSTANCE are not set: the hub starts modules")
    pipe = open_router_pipe(launch[0], hub_pid_from_env())
    return Session(pipe, pipe), launch


def _dump(value):
    return encode(value.to_json() if isinstance(value, Model) else value)


class Session:
    def __init__(self, reader, writer):
        self.reader, self.writer = reader, writer
        self.next_id = 1
        self.pending = {}
        # Thread safety: `_cond` guards next_id, pending, and the call bookkeeping below;
        # `_wlock` keeps one message's bytes together on the writer; one thread at a time holds
        # the reader role (`_owner`) and hands every response for a `call` to its waiter.
        self._cond = threading.Condition()
        self._wlock = threading.Lock()
        self._waiters = set()
        self._done = {}
        self._owner = None
        self._closed = False
        # `facts.subscribe` params: not answered yet (by id), and denied by the hub. The denied
        # ones are sent again when `module.grants` says an Allow widened the module's grants.
        self._subscribing = {}
        self._denied_subs = []
        # Live subscriptions' params by the hub's subscription id: one the hub ends
        # (`module.grants {ended}`) goes back to the denied ones.
        self._live_subs = {}

    def _write(self, msg):
        line = json.dumps(msg, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        if len(line) > MAX_MESSAGE:
            raise ValueError(f"a message is at most {MAX_MESSAGE} bytes")
        with self._wlock:
            self.writer.write(line + b"\n")
            if hasattr(self.writer, "flush"):
                self.writer.flush()

    def request(self, method, params=None):
        return self._request(method, params, False)

    def _request(self, method, params, wait):
        body = _dump(params or {})
        with self._cond:
            id_ = self.next_id
            self.next_id += 1
            # Registered before the write: the answer may arrive before write returns.
            self.pending[id_] = method
            if method == "facts.subscribe":
                self._subscribing[id_] = body
            elif method == "facts.unsubscribe" and isinstance(body, dict):
                self._live_subs.pop(body.get("subscription"), None)
            if wait:
                self._waiters.add(id_)
        try:
            self._write({"jsonrpc": "2.0", "id": id_, "method": method, "params": body})
        except BaseException:
            with self._cond:
                self.pending.pop(id_, None)
                self._subscribing.pop(id_, None)
                self._waiters.discard(id_)
            raise
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

    def subscribe(self, pattern):
        """`facts.subscribe {pattern}`. One the hub denies (not allowed yet) is kept and sent
        again by itself on `module.grants` (v1.22); the new answer is another response."""
        return self.request("facts.subscribe", {"pattern": pattern})

    def denied_subscriptions(self):
        """The `facts.subscribe` params the hub denied and that wait for an Allow."""
        with self._cond:
            return list(self._denied_subs)

    def _keep_denied(self, params):
        # Under self._cond.
        if params not in self._denied_subs:
            del self._denied_subs[: max(0, len(self._denied_subs) - MAX_DENIED_SUBS + 1)]
            self._denied_subs.append(params)

    def _grants_changed(self, params):
        params = params if isinstance(params, dict) else {}
        with self._cond:
            for n in params.get("ended") or []:
                ended = self._live_subs.pop(n, None) if isinstance(n, int) else None
                if ended is not None:
                    self._keep_denied(ended)
            if not params.get("added"):
                return
            again, self._denied_subs = self._denied_subs, []
        while again:
            params = again.pop(0)
            try:
                self.request("facts.subscribe", params)
            except BaseException:
                # Not sent: these still wait for the next `module.grants`.
                with self._cond:
                    self._denied_subs = [params] + again + self._denied_subs
                raise

    def handle(self, msg, handler):
        """Handles one message from the hub. Returns ("response", id, method, result | RpcError),
        ("answered", method) or ("notified", method)."""
        if msg.get("jsonrpc") != "2.0":
            raise ValueError('jsonrpc must be "2.0"')
        method = msg.get("method")
        if method is None:
            id_ = msg.get("id")
            with self._cond:
                if id_ not in self.pending:
                    raise ValueError(f"no request {id_} is waiting")
                sent = self.pending.pop(id_)
                got = RpcError.from_json(msg["error"]) if "error" in msg else msg["result"]
                asked = self._subscribing.pop(id_, None)
                if asked is not None:
                    if isinstance(got, RpcError):
                        if got.code == DENIED:
                            self._keep_denied(asked)
                    elif isinstance(got, dict) and isinstance(got.get("subscription"), int):
                        self._live_subs[got["subscription"]] = asked
                if id_ in self._waiters:
                    self._done[id_] = got
                    self._cond.notify_all()
            return ("response", id_, sent, got)
        params = msg.get("params", {})
        if "id" not in msg:
            if method == "module.exit":
                handler.exit()
            elif method == MODULE_GRANTS:
                self._grants_changed(params)
                handler.notification(method, params)
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

    def _pump(self, handler):
        """Reads and handles one message as the reader; None once the pipe closed."""
        msg = self.read()
        if msg is None:
            with self._cond:
                self._closed = True
                self._cond.notify_all()
            return None
        return self.handle(msg, handler)

    def _as_reader(self, handler):
        """Waits for the reader role (a handler's own nested call already has it), then pumps one
        message. Returns None if the pipe closed, ("wait",) if another thread read meanwhile."""
        me = threading.get_ident()
        with self._cond:
            if self._owner is not None and self._owner != me:
                self._cond.wait()
                return ("wait",)
            prev, self._owner = self._owner, me
        try:
            return self._pump(handler)
        finally:
            with self._cond:
                self._owner = prev
                self._cond.notify_all()

    def call(self, method, params, handler):
        """Sends a request and waits for its answer, handling hub calls meanwhile. Safe from
        several threads: each gets its own answer, whichever thread reads it."""
        id_ = self._request(method, params, True)
        try:
            while True:
                with self._cond:
                    if id_ in self._done:
                        got = self._done.pop(id_)
                        break
                    if self._closed:
                        raise SessionClosed()
                self._as_reader(handler)
        finally:
            with self._cond:
                self._waiters.discard(id_)
                self._done.pop(id_, None)
        if isinstance(got, RpcError):
            raise got
        return got

    def run(self, handler):
        """Answers hub calls until the pipe closes or `module.exit` arrives."""
        while True:
            got = self._as_reader(handler)
            if got is None or got == ("notified", "module.exit"):
                return
