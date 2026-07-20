#!/usr/bin/env python3
"""Loopback-only delay proxy for the packaged Trace Browser benchmark.

All responses come from the real Alex daemon. Only the first two older-page
responses for the generated long session are delayed, making loading and stale
response suppression deterministic without replacing any production endpoint.
"""

from __future__ import annotations

import argparse
import http.client
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit


HOP_BY_HOP = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
}


class DelayState:
    def __init__(self, long_session_id: str, delays: list[float]) -> None:
        self.long_session_path = f"/traces/sessions/{long_session_id}/transcript"
        self.delays = delays
        self._older_page_count = 0
        self._lock = threading.Lock()

    def delay_for(self, raw_path: str) -> tuple[float, int | None]:
        parsed = urlsplit(raw_path)
        if parsed.path != self.long_session_path or "before_ms" not in parse_qs(parsed.query):
            return 0.0, None
        with self._lock:
            self._older_page_count += 1
            request_number = self._older_page_count
        index = request_number - 1
        return (self.delays[index] if index < len(self.delays) else 0.0), request_number


class BenchmarkProxy(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(
        self,
        address: tuple[str, int],
        upstream_host: str,
        upstream_port: int,
        delay_state: DelayState,
    ) -> None:
        super().__init__(address, ProxyHandler)
        self.upstream_host = upstream_host
        self.upstream_port = upstream_port
        self.delay_state = delay_state


class ProxyHandler(BaseHTTPRequestHandler):
    server: BenchmarkProxy
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        self._proxy()

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        self._proxy()

    def do_PUT(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        self._proxy()

    def log_message(self, format: str, *args: object) -> None:
        sys.stderr.write("benchmark-proxy: " + (format % args) + "\n")

    def _proxy(self) -> None:
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length) if length else None
        headers = {
            name: value
            for name, value in self.headers.items()
            if name.lower() not in HOP_BY_HOP and name.lower() != "host"
        }
        connection = http.client.HTTPConnection(
            self.server.upstream_host, self.server.upstream_port, timeout=15
        )
        try:
            connection.request(self.command, self.path, body=body, headers=headers)
            upstream = connection.getresponse()
            response_body = upstream.read()
            delay, older_page_number = self.server.delay_state.delay_for(self.path)
            if older_page_number is not None:
                self.log_message(
                    "older-page response %d delayed %.3fs",
                    older_page_number,
                    delay,
                )
            if delay:
                time.sleep(delay)
            self.send_response(upstream.status, upstream.reason)
            for name, value in upstream.getheaders():
                if name.lower() not in HOP_BY_HOP and name.lower() != "content-length":
                    self.send_header(name, value)
            self.send_header("content-length", str(len(response_body)))
            self.end_headers()
            self.wfile.write(response_body)
        except (BrokenPipeError, ConnectionResetError):
            self.log_message("client left before delayed response was written")
        except Exception as error:  # benchmark diagnostics must reach its log
            rendered = json.dumps({"error": "proxy_failure", "detail": str(error)}).encode()
            try:
                self.send_response(502)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(rendered)))
                self.end_headers()
                self.wfile.write(rendered)
            except (BrokenPipeError, ConnectionResetError):
                pass
            self.log_message("proxy failure: %s", error)
        finally:
            connection.close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream-host", default="127.0.0.1")
    parser.add_argument("--upstream-port", type=int, required=True)
    parser.add_argument("--listen-port", type=int, default=0)
    parser.add_argument("--long-session-id", required=True)
    parser.add_argument("--ready-file", type=Path, required=True)
    parser.add_argument(
        "--older-page-delays",
        default="0.25,1.5",
        help="comma-separated delays in seconds (default: 0.25,1.5)",
    )
    args = parser.parse_args()
    try:
        args.delays = [float(value) for value in args.older_page_delays.split(",")]
    except ValueError as error:
        parser.error(f"invalid --older-page-delays: {error}")
    if args.upstream_port < 1 or args.upstream_port > 65535:
        parser.error("--upstream-port must be between 1 and 65535")
    if args.listen_port < 0 or args.listen_port > 65535:
        parser.error("--listen-port must be between 0 and 65535")
    if any(delay < 0 for delay in args.delays):
        parser.error("--older-page-delays must not contain negative values")
    return args


def main() -> None:
    args = parse_args()
    server = BenchmarkProxy(
        ("127.0.0.1", args.listen_port),
        args.upstream_host,
        args.upstream_port,
        DelayState(args.long_session_id, args.delays),
    )
    args.ready_file.parent.mkdir(parents=True, exist_ok=True)
    args.ready_file.write_text(
        json.dumps({"host": "127.0.0.1", "port": server.server_port}) + "\n",
        encoding="utf-8",
    )
    sys.stderr.write(f"benchmark-proxy: listening on 127.0.0.1:{server.server_port}\n")
    server.serve_forever(poll_interval=0.05)


if __name__ == "__main__":
    main()
