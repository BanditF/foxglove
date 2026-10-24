"""#298 spike listener: a one-file WebSocket server on 127.0.0.1:47911 (standard library only).

It logs each connection's request line and headers (Origin above all), completes the
handshake, prints the first message, answers it, and closes. It stops by itself after
--minutes (default 10) or after --connections (default 3) handshakes, whichever comes first.

    python -I scripts/spikes/we-websocket/listener.py
"""

import argparse
import base64
import hashlib
import json
import socket
import struct
import time

GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def read_frame(conn):
    head = conn.recv(2)
    if len(head) < 2:
        return None
    length = head[1] & 0x7F
    if length == 126:
        length = struct.unpack(">H", conn.recv(2))[0]
    elif length == 127:
        length = struct.unpack(">Q", conn.recv(8))[0]
    mask = conn.recv(4) if head[1] & 0x80 else b"\0\0\0\0"
    data = b""
    while len(data) < length:
        chunk = conn.recv(length - len(data))
        if not chunk:
            break
        data += chunk
    return bytes(b ^ mask[i % 4] for i, b in enumerate(data))


def send_text(conn, text):
    payload = text.encode()
    conn.sendall(bytes([0x81, len(payload)]) + payload if len(payload) < 126 else bytes([0x81, 126]) + struct.pack(">H", len(payload)) + payload)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--port", type=int, default=47911)
    p.add_argument("--minutes", type=float, default=10)
    p.add_argument("--connections", type=int, default=3)
    args = p.parse_args()
    until = time.monotonic() + args.minutes * 60
    server = socket.socket()
    server.bind(("127.0.0.1", args.port))
    server.listen()
    print(f"listening on 127.0.0.1:{args.port} for {args.minutes} min or {args.connections} connections", flush=True)
    done = 0
    while done < args.connections:
        left = until - time.monotonic()
        if left <= 0:
            print("time limit reached", flush=True)
            break
        server.settimeout(left)
        try:
            conn, peer = server.accept()
        except socket.timeout:
            print("time limit reached", flush=True)
            break
        with conn:
            conn.settimeout(10)
            request = b""
            while b"\r\n\r\n" not in request:
                chunk = conn.recv(4096)
                if not chunk:
                    break
                request += chunk
            lines = request.decode(errors="replace").split("\r\n")
            headers = {k.strip().lower(): v.strip() for k, _, v in (l.partition(":") for l in lines[1:] if ":" in l)}
            print(f"\n== connection from {peer}: {lines[0]}", flush=True)
            for k in ("origin", "user-agent", "sec-websocket-key", "sec-websocket-protocol"):
                print(f"   {k}: {headers.get(k, '(none)')}", flush=True)
            key = headers.get("sec-websocket-key")
            if not key:
                continue
            accept = base64.b64encode(hashlib.sha1((key + GUID).encode()).digest()).decode()
            conn.sendall(f"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n".encode())
            done += 1
            try:
                msg = read_frame(conn)
                print(f"   first message: {msg.decode(errors='replace') if msg else '(none)'}", flush=True)
                send_text(conn, json.dumps({"hello": "foxglove spike", "origin_seen": headers.get("origin")}))
                conn.sendall(bytes([0x88, 0]))
            except socket.timeout:
                print("   no message within 10 s", flush=True)
    server.close()


if __name__ == "__main__":
    main()
