#!/usr/bin/env python3
"""Log every Ollama request/response, then forward it untouched.

Exists because a request that works from a shell was failing inside the app with a
zero-value Ollama answer (`{"model":"","created_at":"0001-01-01T00:00:00Z",...}`),
and the only way to see the difference is to look at the bytes on the wire.

Usage:
    scripts/ollama-proxy.py                      # listens on 127.0.0.1:11435
    scripts/ollama-proxy.py --port 11435 --upstream http://127.0.0.1:11434

Then start the app pointing at it:

    SHERLOCK_OLLAMA_BASE=http://127.0.0.1:11435 npm run tauri dev

Files land in `cache/tmp/proxy/` (one pair per request, in order):

    req_0001.json / res_0001.json    bodies
    req_0001.http                    method, path and headers we received

Everything is forwarded verbatim, so the proxy is transparent to the app.
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import pathlib
import sys
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DEFAULT_OUT = os.path.expanduser("~/.local/share/frank_sherlock/cache/tmp/proxy")
HOP_BY_HOP = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    counter = 0
    out_dir = pathlib.Path(DEFAULT_OUT)
    upstream = "http://127.0.0.1:11434"

    def log_message(self, fmt: str, *args) -> None:  # keep the console quiet
        pass

    def _forward(self) -> None:
        Handler.counter += 1
        index = f"{Handler.counter:04d}"
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        path = self.path

        if "generate" in path and body:
            Handler.out_dir.mkdir(parents=True, exist_ok=True)
            (Handler.out_dir / f"req_{index}.json").write_bytes(body)
            headers = "\n".join(f"{k}: {v}" for k, v in self.headers.items())
            (Handler.out_dir / f"req_{index}.http").write_text(
                f"{self.command} {path}\n{headers}\n\nContent-Length: {len(body)}\n",
                encoding="utf-8",
            )
            try:
                parsed = json.loads(body)
                print(
                    f"#{index} {path} model={parsed.get('model')!r} "
                    f"prompt={len(parsed.get('prompt') or '')}B "
                    f"images={[len(i) for i in parsed.get('images') or []]} "
                    f"format={parsed.get('format')!r} options={parsed.get('options')}"
                )
            except json.JSONDecodeError as err:
                print(f"#{index} {path} BODY IS NOT JSON ({err}): {body[:120]!r}")

        url = urllib.parse.urlparse(Handler.upstream)
        conn = http.client.HTTPConnection(url.hostname, url.port or 80, timeout=600)
        headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP_BY_HOP}
        conn.request(self.command, path, body=body, headers=headers)
        upstream = conn.getresponse()
        payload = upstream.read()

        if "generate" in path and payload:
            try:
                parsed = json.loads(payload)
                head = {
                    "model": parsed.get("model"),
                    "done": parsed.get("done"),
                    "done_reason": parsed.get("done_reason"),
                    "response_len": len(parsed.get("response") or ""),
                    "error": parsed.get("error"),
                }
                print(f"   -> {upstream.status} {head}")
            except json.JSONDecodeError:
                pass
            if upstream.status >= 400 or not (json.loads(payload).get("response") or ""):
                (Handler.out_dir / f"res_{index}.json").write_bytes(payload)

        self.send_response(upstream.status)
        for key, value in upstream.getheaders():
            if key.lower() not in HOP_BY_HOP:
                self.send_header(key, value)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
        conn.close()

    do_POST = _forward
    do_GET = _forward


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=11435)
    parser.add_argument("--upstream", default="http://127.0.0.1:11434")
    parser.add_argument("--out", default=DEFAULT_OUT)
    args = parser.parse_args()

    Handler.out_dir = pathlib.Path(args.out)
    Handler.upstream = args.upstream
    Handler.out_dir.mkdir(parents=True, exist_ok=True)
    print(f"proxying 127.0.0.1:{args.port} -> {args.upstream}")
    print(f"captures in {Handler.out_dir}")
    print("start the app with: SHERLOCK_OLLAMA_BASE=http://127.0.0.1:%d" % args.port)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
