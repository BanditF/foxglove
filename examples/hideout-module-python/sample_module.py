"""A sample Hideout module in Python: `example.counter` (docs/hideout-contract.md §7.5, §7.6, §5).

One page panel with a count and an Add button, a busy drain while a count-down runs, and the
count handed over through state.export/state.import. The hub starts it with HIDEOUT_PIPE,
HIDEOUT_TOKEN and HIDEOUT_INSTANCE set; run by hand, it exits saying so.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk" / "python"))

from hideout_module import models  # noqa: E402
from hideout_module.session import Handler, RpcError, connect_env  # noqa: E402

ID = "example.counter"


class Counter(Handler):
    def __init__(self):
        self.count = 0
        self.counting_down = False
        self.patch = None  # a ui.patch to send after the current message
        self.exiting = False

    @staticmethod
    def registration():
        return models.ModuleRegister(methods=["example.counter.*"])

    def panels(self, instance):
        tree = models.Node(type="column", id="root", children=[
            models.Node(type="label", id="count", props={"text": str(self.count)}),
            models.Node(type="button", id="add", props={"label": "Add"}),
        ])
        return models.Register(instance=instance, panels=[
            models.Panel(id="counter", slot="page", title="Counter", icon="plus", tree=tree),
        ])

    def _show_count(self):
        self.patch = models.UiPatch(panel="counter", ops=[
            models.PatchOp(op="set", node="count", props={"text": str(self.count)}),
        ])

    def drain(self, drain):
        if self.counting_down:
            return models.DrainResultBusy(reason="a count-down")
        return models.DrainResultDone()

    def state_export(self):
        return {"blob": {"count": self.count}}

    def state_import(self, params):
        self.count = int(params.get("blob", {}).get("count", 0))
        return {}

    def ui_event(self, event):
        if (event.panel, event.node) != ("counter", "add"):
            raise RpcError(1, "no such control")
        self.count += 1
        self._show_count()
        return {}

    def action(self, action):
        if action.action != "reset":
            raise RpcError(1, "no such action")
        self.count = 0
        self._show_count()
        return {}

    def exit(self):
        self.exiting = True


class Scripted(Handler):
    """Answers each hub call with the module's answer from a transcript (set `answer` first). The
    session still parses the params into the models and encodes the typed result."""

    def __init__(self):
        self.answer = None
        self.notifications = []

    def _next(self):
        answer, self.answer = self.answer, None
        if answer is None:
            raise RpcError(-32603, "no scripted answer")
        if isinstance(answer, RpcError):
            raise answer
        return answer[0]

    def drain(self, drain):
        return _decode_drain(self._next())

    def state_export(self):
        return self._next()

    def state_import(self, params):
        return self._next()

    def ui_event(self, event):
        return self._next()

    def window_open(self, open_):
        return self._next()

    def action(self, action):
        return self._next()

    def settings_get(self, params):
        return self._next()

    def settings_set(self, set_):
        return self._next()

    def channel_create(self, create):
        return self._next()

    def exit(self):
        self.notifications.append("module.exit")

    def notification(self, method, params):
        self.notifications.append(method)

    def call(self, method, params):
        return self._next()


def _decode_drain(result):
    from hideout_module._model import decode

    return decode(models.DRAINRESULT_SPEC, result, "module.drain result")


def main():
    session, (pipe, token, instance) = connect_env()
    counter = Counter()
    init = models.InitializeModule(id=ID, version="0.1.0", instance=instance, token=token, hub=models.HUB)
    session.call("initialize", init, counter)
    session.call("module.register", Counter.registration(), counter)
    session.call("ui.register", counter.panels(instance), counter)
    session.ready(instance)
    while (msg := session.read()) is not None:
        session.handle(msg, counter)
        if counter.patch is not None:
            session.ui_patch(counter.patch)
            counter.patch = None
        if counter.exiting:
            break


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as e:
        sys.exit(f"{ID}: {e}")
