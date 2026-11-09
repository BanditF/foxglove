"""Replays the module side of the shared JSON-RPC transcripts (tests/hideout-vectors/transcripts,
#906) through the Python session, in process: no pipe, no hub.

For each module instance in a transcript (`module`, `old-instance`, `new-instance`, `vulpes`), a
message the module sends is made with the session's typed call for that method and must equal the
transcript's message (request ids mapped); a message the hub sends is fed to the session, which
parses it into the generated models and answers through `Scripted`; the answer must be the
transcript's next message. The same rules as the Rust replay (examples/hideout-module-rust).

    python examples/hideout-module-python/replay.py
"""

import io
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from sample_module import Scripted  # noqa: E402
from hideout_module import models  # noqa: E402
from hideout_module.session import RpcError, Session  # noqa: E402

TRANSCRIPTS = HERE.parents[1] / "tests" / "hideout-vectors" / "transcripts"
ROLES = ("module", "old-instance", "new-instance", "vulpes")

# Module calls with a model, and the session method that sends them.
SENDS = {
    "initialize": ("initialize", models.InitializeModule),
    "module.register": ("register", models.ModuleRegister),
    "ui.register": ("ui_register", models.Register),
    "ui.asset.put": ("asset_put", models.UiAssetPut),
    "hub.notify": ("hub_notify", models.HubNotify),
    "channel.open": ("channel_open", models.ChannelOpen),
    "ui.patch": ("ui_patch", models.UiPatch),
}


class Instance:
    def __init__(self):
        self.out = io.BytesIO()
        self.session = Session(io.BytesIO(), self.out)
        self.module = Scripted()
        self.sent = []
        self.ids = {}

    def collect(self):
        data = self.out.getvalue()
        self.out.seek(0)
        self.out.truncate()
        self.sent += [json.loads(line) for line in data.decode("utf-8").splitlines()]


def strip(v):
    """Drops keys whose value is null or [] below the top: defaults a writer may leave off."""
    if isinstance(v, dict):
        return {k: strip(x) for k, x in v.items() if x is not None and x != []}
    if isinstance(v, list):
        return [strip(x) for x in v]
    return v


def normalize(msg):
    return {k: strip(v) if k in ("params", "result", "error") else v for k, v in msg.items()}


def send(inst, msg):
    method, params = msg["method"], msg.get("params", {})
    s = inst.session
    if method in SENDS:
        name, model = SENDS[method]
        getattr(s, name)(model.from_json(params))
    elif method == "module.ready":
        s.ready(params["instance"])
    elif method == "module.idle":
        s.idle()
    elif method == "facts.set":
        s.set_fact(params)
    elif "id" in msg:
        s.request(method, params)
    else:
        s.notify(method, params)


def answer(steps, start, role, id_):
    for step in steps[start:]:
        m = step.get("msg")
        if m and step["from"] == role and "method" not in m and m.get("id") == id_:
            return (m["result"],) if "result" in m else RpcError.from_json(m["error"])
    return None


def replay(path):
    """Replays one transcript; False when it has no module side. Raises AssertionError."""
    steps = json.loads(path.read_text(encoding="utf-8"))["steps"]
    instances = {}
    for i, step in enumerate(steps):
        msg = step.get("msg")
        if msg is None:
            continue
        src, dst = step["from"], step["to"]
        if src in ROLES:
            inst = instances.setdefault(src, Instance())
            if "method" in msg:
                send(inst, msg)
            inst.collect()
            assert inst.sent, f"step {i}: the module sent nothing; expected {msg}"
            got = inst.sent.pop(0)
            if "method" in msg and "id" in msg:
                inst.ids[msg["id"]] = got["id"]
                got["id"] = msg["id"]
            assert normalize(got) == normalize(msg), f"step {i}: the module sent {got}\n  expected {msg}"
        elif dst in ROLES:
            inst = instances.setdefault(dst, Instance())
            msg = dict(msg)
            if "method" in msg:
                if "id" in msg:
                    inst.module.answer = answer(steps, i, dst, msg["id"])
                got = inst.session.handle(msg, inst.module)
                assert got[0] != "response", f"step {i}: a hub call read as a response"
            else:
                assert msg["id"] in inst.ids, f"step {i}: no request {msg['id']}"
                msg["id"] = inst.ids[msg["id"]]
                got = inst.session.handle(msg, inst.module)
                assert got[0] == "response", f"step {i}: an answer read as {got}"
    for role, inst in instances.items():
        inst.collect()
        assert not inst.sent, f"{role} sent more than the transcript: {inst.sent[0]}"
        assert not inst.session.pending, f"{role} has unanswered requests: {inst.session.pending}"
    return bool(instances)


def replay_all():
    """(replayed count, failures)."""
    replayed, failures = 0, []
    for path in sorted(TRANSCRIPTS.glob("*/*.json")):
        try:
            replayed += replay(path)
        except (AssertionError, ValueError, KeyError) as e:
            failures.append(f"{path.relative_to(TRANSCRIPTS)}: {e}")
    return replayed, failures


if __name__ == "__main__":
    count, failed = replay_all()
    print("\n".join(failed) or f"replayed the module side of {count} transcripts")
    sys.exit(1 if failed else 0)
