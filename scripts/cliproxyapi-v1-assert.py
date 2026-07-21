#!/usr/bin/env python3
"""Assertions for the loopback CLIProxyAPI V1 Docker fixture."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def load(path: str) -> dict:
    value = json.loads(Path(path).read_text())
    if not isinstance(value, dict):
        raise AssertionError(f"expected JSON object in {path}")
    return value


def success(path: str, protocol: str) -> None:
    body = load(path)
    if protocol == "chat":
        actual = body["choices"][0]["message"]["content"]
    elif protocol == "responses":
        actual = body["output"][0]["content"][0]["text"]
    elif protocol == "anthropic":
        actual = body["content"][0]["text"]
    else:
        raise AssertionError(f"unknown protocol {protocol}")
    if actual != "cliproxyapi-v1-ok":
        raise AssertionError(f"unexpected {protocol} completion: {actual!r}")


def error(path: str, code: str) -> None:
    body = load(path)
    actual = body.get("error", {}).get("code")
    if actual != code:
        raise AssertionError(f"expected error code {code!r}, got {actual!r}")


def stream(path: str) -> None:
    text = Path(path).read_text()
    for marker in ('"tool_use"', '"shell"', "pwd", "message_stop"):
        if marker not in text:
            raise AssertionError(f"stream is missing {marker!r}")


def stats(path: str, expected: int) -> None:
    actual = load(path).get("calls")
    if actual != expected:
        raise AssertionError(f"expected {expected} provider calls, got {actual!r}")


def no_secrets(response_dir: str, secret_file: str) -> None:
    secrets = [line.strip() for line in Path(secret_file).read_text().splitlines() if line.strip()]
    for path in Path(response_dir).iterdir():
        if not path.is_file():
            continue
        payload = path.read_bytes()
        for secret in secrets:
            if secret.encode() in payload:
                raise AssertionError(f"fixture credential leaked into response artifact {path.name}")


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)

    p_success = sub.add_parser("success")
    p_success.add_argument("path")
    p_success.add_argument("protocol", choices=("chat", "responses", "anthropic"))

    p_error = sub.add_parser("error")
    p_error.add_argument("path")
    p_error.add_argument("code")

    p_stream = sub.add_parser("stream")
    p_stream.add_argument("path")

    p_stats = sub.add_parser("stats")
    p_stats.add_argument("path")
    p_stats.add_argument("expected", type=int)

    p_secrets = sub.add_parser("no-secrets")
    p_secrets.add_argument("response_dir")
    p_secrets.add_argument("secret_file")

    args = parser.parse_args()
    if args.command == "success":
        success(args.path, args.protocol)
    elif args.command == "error":
        error(args.path, args.code)
    elif args.command == "stream":
        stream(args.path)
    elif args.command == "stats":
        stats(args.path, args.expected)
    elif args.command == "no-secrets":
        no_secrets(args.response_dir, args.secret_file)


if __name__ == "__main__":
    main()
