#!/usr/bin/env python3
"""Serve candidate release assets over loopback and record requested paths."""

import argparse
import json
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class Handler(SimpleHTTPRequestHandler):
    server_version = "AlexCandidateAssetServer/1"

    def log_message(self, _format, *_args):
        return

    def do_GET(self):
        with self.server.log_path.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps({"method": "GET", "path": self.path}, sort_keys=True) + "\n")
        super().do_GET()

    def end_headers(self):
        self.send_header("cache-control", "no-store")
        super().end_headers()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--ready-file", required=True, type=Path)
    parser.add_argument("--log-file", required=True, type=Path)
    args = parser.parse_args()

    root = args.directory.resolve(strict=True)
    args.ready_file.parent.mkdir(parents=True, exist_ok=True)
    args.log_file.parent.mkdir(parents=True, exist_ok=True)
    handler = partial(Handler, directory=str(root))
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    server.log_path = args.log_file
    args.ready_file.write_text(str(server.server_address[1]) + "\n", encoding="ascii")
    server.serve_forever()


if __name__ == "__main__":
    main()
