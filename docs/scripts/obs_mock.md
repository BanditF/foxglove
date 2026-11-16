# obs_mock.py: stand-in for OBS's WebSocket server

scripts/lab/obs_mock.py is a stdlib-only library module that stands in for the OBS WebSocket server (obs-websocket v5) on 127.0.0.1, so Broadcast Status can be tested without OBS. It performs the password challenge and the Identify/Identified handshake, answers the three snapshot requests (GetStreamStatus, GetRecordStatus, GetCurrentProgramScene), and emits the events StreamStateChanged, RecordStateChanged and CurrentProgramSceneChanged when state changes. A wrong password is refused with WebSocket close code 4009, as OBS does. It has no command-line entry point: no argparse, no flags, and it never calls sys.exit.

## Using it

It is imported, not executed. The header documents the usage:

    mock = ObsMock(password='...'); mock.start()
    mock.set(streaming=True); mock.set(scene='Gaming'); mock.stop()

ObsMock takes a required password and an optional port (default 4455). start() binds a TCP listener on 127.0.0.1 (SO_REUSEADDR, listen backlog 4) and starts a daemon accept thread; stop() closes the listener and every connection ("OBS quits"); identified() returns the number of currently identified clients.

## Parameters

There are no --flags. The knobs are the constructor argument and the set() keywords:

| Name | Default | Description |
|---|---|---|
| password (constructor) | required | the password clients must authenticate with (SHA-256 salt/challenge) |
| port (constructor) | 4455 | TCP port on 127.0.0.1 |
| streaming (set) | False | toggles streaming; emits StreamStateChanged (outputActive, outputState OBS_WEBSOCKET_OUTPUT_STARTED/STOPPED), only when the value changes |
| recording (set) | False | toggles recording; emits RecordStateChanged (outputActive, outputState STARTED/PAUSED/STOPPED); paused is False unless set() also passes paused |
| paused (set) | False | pauses or resumes an active recording; emits RecordStateChanged |
| scene (set) | 'Scene' | sets the program scene name; emits CurrentProgramSceneChanged (sceneName) |

Every event is sent to all identified clients as op 5 with eventIntent 64 (1 << 6).

## Exit codes

None. The module has no CLI and never calls sys.exit. The only code it emits is the WebSocket close code 4009 with reason "Authentication failed", sent to a client whose Identify authentication value does not match the expected one.

## What it reads and writes

**Reads:** nothing from disk. From the network: the WebSocket handshake (Sec-WebSocket-Key), then client frames — Identify (op 1), Request (op 6), Close (op 8), Ping (op 9).

**Writes:** nothing to disk. To the network: the HTTP 101 upgrade (Sec-WebSocket-Protocol obswebsocket.json), Identify (op 0: obsWebSocketVersion 5.5.0, rpcVersion 1, salt and challenge), Identified (op 2: negotiatedRpcVersion 1), events (op 5), responses (op 7: result true / code 100 with the data for the three snapshot requests; result false / code 204 with an empty body for any other request type), and Pong (op 10). In memory: the state dict (streaming, recording, paused, scene), the client list, and a log holding 'identified' and 'auth refused'.

## Example

    mock = ObsMock(password='hunter2'); mock.start()
    mock.set(streaming=True); mock.set(scene='Gaming'); mock.stop()

A client connected to ws://127.0.0.1:4455 receives Identify (op 0) with a salt and challenge, sends Identify (op 1) with authentication = base64(SHA-256(base64(SHA-256(password + salt)) + challenge)), is acknowledged with Identified (op 2), and then GetStreamStatus returns {'outputActive': True} and GetCurrentProgramScene returns sceneName 'Gaming'.
