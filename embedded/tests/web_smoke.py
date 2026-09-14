"""Exercise both A1S web workers and hold a telemetry WebSocket open.

Usage: python3 embedded/tests/web_smoke.py <board-ip>
"""

import base64
import concurrent.futures
import gzip
import hashlib
import json
import os
import socket
import struct
import sys
import threading
import time
import urllib.error
import urllib.request


host = sys.argv[1]
base = "http://" + host
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
connect_retry_count = 0
connect_retry_lock = threading.Lock()


def connect_with_retry(connector):
    """Retry only a refused connection while the two web workers cycle listeners."""
    global connect_retry_count
    deadline = time.monotonic() + 2
    while True:
        try:
            return connector()
        except ConnectionRefusedError as error:
            refused = error
        except urllib.error.URLError as error:
            if not isinstance(error.reason, ConnectionRefusedError):
                raise
            refused = error

        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise refused
        with connect_retry_lock:
            connect_retry_count += 1
        time.sleep(min(0.1, remaining))


def get(path):
    with connect_with_retry(lambda: opener.open(base + path, timeout=10)) as response:
        body = response.read()
        assert response.status == 200
        if path.startswith("/api/"):
            json.loads(body)
        elif path == "/":
            assert response.headers.get("Content-Encoding") == "gzip"
            assert b"<html" in gzip.decompress(body).lower()
        return len(body)


for path in ["/", "/api/status", "/api/modules"]:
    print(path, get(path), flush=True)

with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
    lengths = list(pool.map(get, ["/api/status", "/api/modules"] * 10))
assert all(length > 0 for length in lengths)
print("20 concurrent API requests passed", flush=True)

error_cases = [
    ("/missing", None, 404),
    ("/api/seek", b"{", 400),
    ("/api/modules", b"", 400),
]
for path, data, expected in error_cases:
    request = urllib.request.Request(base + path, data=data, headers={"Content-Type": "application/json"})
    try:
        connect_with_retry(lambda: opener.open(request, timeout=10))
        raise AssertionError("expected HTTP error")
    except urllib.error.HTTPError as error:
        assert error.code == expected, error.code
        error.read()
print("404, malformed JSON and empty upload responses passed", flush=True)

with connect_with_retry(lambda: socket.create_connection((host, 80), timeout=10)) as connection:
    key = base64.b64encode(os.urandom(16)).decode()
    connection.sendall(
        (
            "GET /ws HTTP/1.1\r\n"
            + "Host: " + host + "\r\n"
            + "Upgrade: websocket\r\n"
            + "Connection: Upgrade\r\n"
            + "Sec-WebSocket-Key: " + key + "\r\n"
            + "Sec-WebSocket-Version: 13\r\n\r\n"
        ).encode()
    )
    reader = connection.makefile("rb")
    status = reader.readline()
    assert b" 101 " in status, status
    headers = {}
    while True:
        line = reader.readline()
        if line == b"\r\n":
            break
        assert line
        name, value = line.decode().split(":", 1)
        headers[name.lower()] = value.strip()
    expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
    assert headers["sec-websocket-accept"] == expected

    frames = 0
    deadline = time.monotonic() + 60
    next_request = time.monotonic()
    while time.monotonic() < deadline:
        header = reader.read(2)
        assert len(header) == 2, "WebSocket closed"
        opcode, length = header[0] & 15, header[1] & 127
        assert not header[1] & 128
        if length == 126:
            length = struct.unpack("!H", reader.read(2))[0]
        elif length == 127:
            length = struct.unpack("!Q", reader.read(8))[0]
        payload = reader.read(length)
        assert len(payload) == length
        assert opcode == 2 and payload, (opcode, length)
        frames += 1
        if time.monotonic() >= next_request:
            get("/api/status")
            get("/api/modules")
            next_request = time.monotonic() + 2

    connection.sendall(b"\x88\x80" + os.urandom(4))
    print("60-second WebSocket + parallel HTTP soak passed:", frames, "telemetry frames", flush=True)

print("Connection-refused retries:", connect_retry_count, flush=True)
print("PASS", flush=True)
