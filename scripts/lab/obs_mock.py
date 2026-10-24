"""A stand-in for OBS's WebSocket server (obs-websocket v5) on 127.0.0.1, for testing Broadcast Status
without OBS: password challenge, Identify/Identified, the three snapshot requests, and the events
StreamStateChanged / RecordStateChanged / CurrentProgramSceneChanged. A wrong password is refused with
close code 4009, as OBS does. Stdlib only.

    mock = ObsMock(password='...'); mock.start()
    mock.set(streaming=True); mock.set(scene='Gaming'); mock.stop()
"""
import base64, hashlib, json, os, socket, struct, threading

GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11'


def _frame(opcode, payload):
    n = len(payload)
    head = bytes([0x80 | opcode]) + (bytes([n]) if n < 126 else bytes([126]) + struct.pack('>H', n) if n < 65536
                                     else bytes([127]) + struct.pack('>Q', n))
    return head + payload


def _recv_frame(sock):
    def exact(n):
        b = b''
        while len(b) < n:
            c = sock.recv(n - len(b))
            if not c:
                raise ConnectionError('closed')
            b += c
        return b
    b0, b1 = exact(2)
    n = b1 & 0x7F
    if n == 126:
        n = struct.unpack('>H', exact(2))[0]
    elif n == 127:
        n = struct.unpack('>Q', exact(8))[0]
    mask = exact(4) if b1 & 0x80 else b'\0\0\0\0'
    data = bytes(c ^ mask[i % 4] for i, c in enumerate(exact(n)))
    return b0 & 0x0F, data


class ObsMock:
    def __init__(self, password, port=4455):
        self.password, self.port = password, port
        self.state = {'streaming': False, 'recording': False, 'paused': False, 'scene': 'Scene'}
        self.clients, self.lock, self.log = [], threading.Lock(), []
        self.server = None

    def start(self):
        self.server = socket.socket()
        self.server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.server.bind(('127.0.0.1', self.port))
        self.server.listen(4)
        threading.Thread(target=self._accept, daemon=True).start()

    def stop(self):
        """OBS quits: every connection closes and nothing listens."""
        if self.server:
            self.server.close()
            self.server = None
        with self.lock:
            for c in self.clients:
                try:
                    c.close()
                except OSError:
                    pass
            self.clients.clear()

    def identified(self):
        with self.lock:
            return len(self.clients)

    def set(self, **change):
        """Changes OBS's state and sends the matching events, as OBS would."""
        events = []
        with self.lock:
            s = self.state
            if 'streaming' in change and change['streaming'] != s['streaming']:
                s['streaming'] = change['streaming']
                events.append(('StreamStateChanged', {'outputActive': s['streaming'],
                               'outputState': 'OBS_WEBSOCKET_OUTPUT_STARTED' if s['streaming'] else 'OBS_WEBSOCKET_OUTPUT_STOPPED'}))
            if 'recording' in change or 'paused' in change:
                s['recording'] = change.get('recording', s['recording'])
                s['paused'] = s['recording'] and change.get('paused', s['paused'] if 'recording' not in change else False)
                state = ('OBS_WEBSOCKET_OUTPUT_PAUSED' if s['paused'] else 'OBS_WEBSOCKET_OUTPUT_STARTED') if s['recording'] \
                    else 'OBS_WEBSOCKET_OUTPUT_STOPPED'
                events.append(('RecordStateChanged', {'outputActive': s['recording'], 'outputState': state}))
            if 'scene' in change:
                s['scene'] = change['scene']
                events.append(('CurrentProgramSceneChanged', {'sceneName': s['scene']}))
            clients = list(self.clients)
        for kind, data in events:
            msg = json.dumps({'op': 5, 'd': {'eventType': kind, 'eventIntent': 1 << 6, 'eventData': data}}).encode()
            for c in clients:
                try:
                    c.sendall(_frame(1, msg))
                except OSError:
                    pass

    def _accept(self):
        while self.server:
            try:
                sock, _ = self.server.accept()
            except OSError:
                return
            threading.Thread(target=self._serve, args=(sock,), daemon=True).start()

    def _serve(self, sock):
        try:
            req = b''
            while b'\r\n\r\n' not in req:
                c = sock.recv(4096)
                if not c:
                    return
                req += c
            key = next(l.split(b':', 1)[1].strip() for l in req.split(b'\r\n') if l.lower().startswith(b'sec-websocket-key'))
            accept = base64.b64encode(hashlib.sha1(key + GUID.encode()).digest()).decode()
            sock.sendall(('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
                          f'Sec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: obswebsocket.json\r\n\r\n').encode())
            salt, challenge = base64.b64encode(os.urandom(16)).decode(), base64.b64encode(os.urandom(16)).decode()
            sock.sendall(_frame(1, json.dumps({'op': 0, 'd': {'obsWebSocketVersion': '5.5.0', 'rpcVersion': 1,
                                               'authentication': {'salt': salt, 'challenge': challenge}}}).encode()))
            secret = base64.b64encode(hashlib.sha256((self.password + salt).encode()).digest()).decode()
            expected = base64.b64encode(hashlib.sha256((secret + challenge).encode()).digest()).decode()
            while True:
                op, data = _recv_frame(sock)
                if op == 8:
                    return
                if op == 9:
                    sock.sendall(_frame(10, data))
                    continue
                msg = json.loads(data)
                if msg['op'] == 1:
                    if msg['d'].get('authentication') != expected:
                        self.log.append('auth refused')
                        sock.sendall(_frame(8, struct.pack('>H', 4009) + b'Authentication failed.'))
                        return
                    self.log.append('identified')
                    sock.sendall(_frame(1, json.dumps({'op': 2, 'd': {'negotiatedRpcVersion': 1}}).encode()))
                    with self.lock:
                        self.clients.append(sock)
                elif msg['op'] == 6:
                    rt, rid = msg['d']['requestType'], msg['d'].get('requestId')
                    with self.lock:
                        s = dict(self.state)
                    data = {'GetStreamStatus': {'outputActive': s['streaming']},
                            'GetRecordStatus': {'outputActive': s['recording'], 'outputPaused': s['paused']},
                            'GetCurrentProgramScene': {'currentProgramSceneName': s['scene'], 'sceneName': s['scene']}}.get(rt)
                    sock.sendall(_frame(1, json.dumps({'op': 7, 'd': {'requestType': rt, 'requestId': rid,
                                                       'requestStatus': {'result': data is not None, 'code': 100 if data else 204},
                                                       'responseData': data or {}}}).encode()))
        except (OSError, ConnectionError, StopIteration, ValueError):
            pass
        finally:
            with self.lock:
                if sock in self.clients:
                    self.clients.remove(sock)
            try:
                sock.close()
            except OSError:
                pass
