#!/usr/bin/env python3
"""Deterministic loopback-only OpenAI chat mock for the installed macOS gate."""

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


MODEL = "ci-smoke-model"


def compact(value):
    return json.dumps(value, separators=(",", ":"), sort_keys=True).encode("utf-8")


class Handler(BaseHTTPRequestHandler):
    server_version = "AlexCleanMachineMock/1"

    def log_message(self, _format, *_args):
        return

    def send_json(self, status, value):
        body = compact(value)
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def record(self, value):
        with self.server.log_path.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(value, sort_keys=True) + "\n")

    def do_GET(self):
        if self.path == "/v1/models":
            self.record({"event": "models", "path": self.path})
            self.send_json(
                200,
                {
                    "object": "list",
                    "data": [{"id": MODEL, "object": "model", "owned_by": "ci-loopback"}],
                },
            )
            return
        self.send_json(404, {"error": {"message": "not found", "type": "mock_error"}})

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length)
        try:
            request = json.loads(raw)
        except (UnicodeDecodeError, json.JSONDecodeError):
            self.send_json(400, {"error": {"message": "invalid JSON", "type": "mock_error"}})
            return

        authorized = self.headers.get("authorization") == "Bearer x"
        model = request.get("model")
        self.record(
            {
                "authorized": authorized,
                "event": "chat",
                "model": model,
                "path": self.path,
                "stream": request.get("stream", False),
            }
        )
        if self.path != "/v1/chat/completions":
            self.send_json(404, {"error": {"message": "not found", "type": "mock_error"}})
            return
        if not authorized or model != MODEL or request.get("stream") is True:
            self.send_json(
                400,
                {"error": {"message": "unexpected routed request", "type": "mock_error"}},
            )
            return

        self.send_json(
            200,
            {
                "id": "chatcmpl-ci-installed-smoke",
                "object": "chat.completion",
                "created": 946684800,
                "model": MODEL,
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "installed route ok"},
                        "finish_reason": "stop",
                    }
                ],
                "usage": {"prompt_tokens": 1, "completion_tokens": 3, "total_tokens": 4},
            },
        )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ready-file", required=True, type=Path)
    parser.add_argument("--log-file", required=True, type=Path)
    args = parser.parse_args()

    args.ready_file.parent.mkdir(parents=True, exist_ok=True)
    args.log_file.parent.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.log_path = args.log_file
    args.ready_file.write_text(str(server.server_address[1]) + "\n", encoding="ascii")
    server.serve_forever()


if __name__ == "__main__":
    main()
