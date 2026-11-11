"""A contained Python module's side for scripts/test_hideout_contained.py (#1092). The test copies
a Python and this file into a folder, starts it inside an AppContainer and drives it: one JSON
command per stdin line, one JSON answer per stdout line. It uses only the SDK
(sdk/python/hideout_module), as a module would. stdin closing ends it; 60 s is the fallback."""

import json
import os
import sys
import threading

from hideout_module import channel, session


def say(**answer):
    print(json.dumps(answer), flush=True)


def provide(cmd):
    """A provider's channel.create for the consumer the test names: the answer is the pipe name,
    then whether the consumer was admitted (5 frames, then wait for it to hang up)."""
    consumer = channel.Consumer(cmd.get("consumer_pid"), cmd.get("consumer_sid"))
    pipe = channel.ChannelPipe.create(channel.unique_pipe_name("example.py-contained", "c"), consumer)
    say(pipe=pipe.name)
    try:
        conn = pipe.accept_nonce(cmd["nonce"], 20)
    except OSError as e:
        say(accepted=False, error=str(e))
        return
    with conn:
        for i in range(5):
            conn.write_frame(bytes([i]) * 3)
        end = conn.read_frame()
    say(accepted=True, end=end)


def consume(cmd):
    """A consumer's end of a grant {pipe, nonce, pid}: 5 frames, then hang up."""
    try:
        conn = channel.connect_grant({"pipe": cmd["pipe"], "nonce": cmd["nonce"], "pid": cmd.get("pid")})
    except (OSError, ValueError) as e:
        say(error=f"{type(e).__name__}: {e}", winerror=getattr(e, "winerror", None))
        return
    frames = []
    with conn:
        while len(frames) < 5 and (f := conn.read_frame()) is not None:
            frames.append(f.hex())
    say(frames=frames)


def router(cmd):
    """connect_env against the test's stand-in router pipe (HIDEOUT_PIPE, HIDEOUT_HUB_PID)."""
    try:
        s, launch = session.connect_env()
    except OSError as e:
        say(error=f"{type(e).__name__}: {e}")
        return
    s.request("initialize", {"token": launch[1]})
    s.writer.close()
    say(sent=True)


def main():
    threading.Timer(60, lambda: os._exit(3)).start()
    say(pid=os.getpid(), sid=channel.process_app_container(), ns=channel.own_pipe_namespace(),
        session=channel.own_session())
    for line in sys.stdin:
        cmd = json.loads(line)
        {"provide": provide, "consume": consume, "router": router}[cmd["do"]](cmd)
    os._exit(0)


if __name__ == "__main__":
    main()
