"""Validate PSRAM uploads on a running A1S (changes the playing module).

Usage: python3 embedded/tests/web_upload_smoke.py <board-ip> [module ...]
Without module paths, uploads repository fixtures for all five formats. Additional
paths can include an owner's module and a fixture whose decoded image exceeds 512 KiB.
"""

import argparse
import json
from pathlib import Path
import socket
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("host")
    parser.add_argument("modules", nargs="*", type=Path)
    arguments = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    modules = arguments.modules or [root / "fuzz" / "seeds" / extension / (("mk-minimal." if extension == "mod" else "minimal.") + extension) for extension in ("mod", "s3m", "mtm", "xm", "it")]
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(path, data=None):
        deadline = time.monotonic() + 2
        while True:
            try:
                message = urllib.request.Request("http://" + arguments.host + path, data=data, headers={"Content-Type": "application/json" if path == "/api/modules/store" else "application/octet-stream"})
                with opener.open(message, timeout=35) as response:
                    return response.status, response.read()
            except urllib.error.HTTPError as error:
                return error.code, error.read()
            except urllib.error.URLError as error:
                if not isinstance(error.reason, ConnectionRefusedError) or time.monotonic() >= deadline:
                    raise
                time.sleep(0.1)

    def status():
        code, body = request("/api/status")
        assert code == 200, (code, body)
        return json.loads(body)

    def identity():
        current = status()
        return tuple(current[key] for key in ("title", "format", "source", "playing"))

    code, body = request("/api/upload-limits")
    assert code == 200, (code, body)
    limits = json.loads(body)
    assert all(isinstance(limits.get(key), int) and limits[key] > 0 for key in ("max_upload_bytes", "max_image_bytes", "max_stored_image_bytes")), limits
    assert limits["max_stored_image_bytes"] <= limits["max_image_bytes"], limits
    assert limits["max_upload_bytes"] > 512 * 1024, limits
    print("Upload limits:", limits, flush=True)

    for module in modules:
        data = module.read_bytes()
        assert len(data) <= limits["max_upload_bytes"], module
        started = time.monotonic()
        code, body = request("/api/modules", data)
        assert code == 201, (str(module), code, body)
        time.sleep(0.25)
        current = status()
        assert current["source"] == "upload" and current["playing"], current
        if module.suffix.lower() != ".spmi":
            assert current["format"].lower() == module.suffix.lstrip(".").lower(), current
        print(module.name, len(data), "bytes:", current["title"], "in", round(time.monotonic() - started, 2), "s", flush=True)

        before = identity()
        if module.suffix.lower() == ".spmi" and len(data) > limits["max_stored_image_bytes"]:
            code, body = request("/api/modules/store", b'{"id":1}')
            assert code == 409, (code, body)
            assert identity() == before, "oversized store refusal changed playback"
        for invalid in (b"", b"not a tracker module", data[:16]):
            code, body = request("/api/modules", invalid)
            assert 400 <= code < 500, (code, body)
            assert identity() == before, "failed upload changed playback"

    def connect():
        deadline = time.monotonic() + 2
        while True:
            try:
                return socket.create_connection((arguments.host, 80), timeout=10)
            except ConnectionRefusedError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.1)

    before = identity()
    with connect() as connection:
        connection.sendall(("POST /api/modules HTTP/1.1\r\nHost: " + arguments.host + "\r\nContent-Length: " + str(limits["max_upload_bytes"] + 1) + "\r\nConnection: close\r\n\r\n").encode())
        # End the body: picoserve drains unread request data before sending its refusal.
        connection.shutdown(socket.SHUT_WR)
        with connection.makefile("rb") as reader:
            reply = reader.readline()
            assert b" 413 " in reply, reply
    assert identity() == before, "oversized upload changed playback"

    # A disconnected partial body must relinquish staging without adopting any bytes.
    before = identity()
    connection = connect()
    with connection:
        connection.sendall(("POST /api/modules HTTP/1.1\r\nHost: " + arguments.host + "\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n").encode() + b"partial")
        time.sleep(0.1)
        code, body = request("/api/modules", b"concurrent upload")
        assert code == 409, (code, body)
    time.sleep(0.5)
    assert identity() == before, "disconnected upload changed playback"

    # Repeated replacement exercises reuse of both image buffers and their retirement.
    for iteration in range(4):
        module = modules[iteration % len(modules)]
        code, body = request("/api/modules", module.read_bytes())
        assert code == 201, (iteration, code, body)
    print("PASS: uploads, malformed/oversized rejection, partial disconnect and repeated replacement", flush=True)


if __name__ == "__main__":
    main()
