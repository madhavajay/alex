#!/usr/bin/env python3
"""Generate a deterministic, synthetic legacy Alex corpus for LAR benchmarks.

The output mirrors the important parts of the existing SQLite + bodies layout,
but contains no captured user data. Bodies deliberately include repeated
conversation prefixes, passthrough duplicates, compaction, tool payloads,
stream frames, missing files, and corrupt gzip members.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import random
import shutil
import sqlite3
import time
from pathlib import Path


TRACE_SCHEMA = """
CREATE TABLE traces (
  id TEXT PRIMARY KEY,
  ts_request_ms INTEGER NOT NULL,
  ts_response_ms INTEGER,
  session_id TEXT,
  harness TEXT,
  client_format TEXT,
  upstream_provider TEXT,
  upstream_format TEXT,
  requested_model TEXT,
  routed_model TEXT,
  method TEXT,
  path TEXT,
  status INTEGER,
  streamed INTEGER,
  req_body_path TEXT,
  upstream_req_body_path TEXT,
  resp_body_path TEXT,
  req_headers_json TEXT,
  resp_headers_json TEXT,
  attempts TEXT,
  error TEXT,
  run_id TEXT,
  tags_json TEXT
);
CREATE TABLE tool_calls (
  id TEXT PRIMARY KEY,
  harness TEXT NOT NULL,
  session_id TEXT NOT NULL,
  turn_id TEXT,
  tool_call_id TEXT NOT NULL,
  trace_id TEXT,
  tool_name TEXT NOT NULL,
  ts_start_ms INTEGER NOT NULL,
  ts_end_ms INTEGER,
  is_error INTEGER,
  exit_status INTEGER,
  args_body_path TEXT,
  result_body_path TEXT
);
"""


def json_bytes(value: object) -> bytes:
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class Corpus:
    def __init__(self, root: Path, session_id: str, seed: int) -> None:
        self.root = root
        self.session_id = session_id
        self.random = random.Random(seed)
        self.manifest: list[dict[str, object]] = []
        self.body_dir = root / "bodies" / "2026-01-15"
        self.body_dir.mkdir(parents=True)

    def body(self, trace_id: str, kind: str, data: bytes) -> str:
        path = self.body_dir / f"{trace_id}.{kind}.gz"
        path.write_bytes(gzip.compress(data, compresslevel=6, mtime=0))
        self.manifest.append(
            {
                "trace_id": trace_id,
                "kind": kind,
                "path": str(path),
                "length": len(data),
                "sha256": sha256(data),
                "state": "valid",
            }
        )
        return str(path)

    def missing_body(self, trace_id: str, kind: str) -> str:
        path = self.body_dir / f"{trace_id}.{kind}.gz"
        self.manifest.append(
            {
                "trace_id": trace_id,
                "kind": kind,
                "path": str(path),
                "state": "missing",
            }
        )
        return str(path)

    def corrupt_body(self, trace_id: str, kind: str) -> str:
        path = self.body_dir / f"{trace_id}.{kind}.gz"
        path.write_bytes(b"\x1f\x8b\x08\x00truncated")
        self.manifest.append(
            {
                "trace_id": trace_id,
                "kind": kind,
                "path": str(path),
                "state": "corrupt",
            }
        )
        return str(path)


def response_sse(turn: int, target_bytes: int | None = None) -> bytes:
    pieces = [
        {"type": "response.output_text.delta", "delta": f"answer-{turn}-"},
        {"type": "response.output_text.delta", "delta": "synthetic"},
        {
            "type": "response.completed",
            "response": {"id": f"resp-{turn:04d}", "status": "completed"},
        },
    ]
    rendered = b"".join(
        b"event: message\n" + b"data: " + json_bytes(piece) + b"\n\n"
        for piece in pieces
    )
    if target_bytes and target_bytes > len(rendered):
        # Add deterministic, locally high-entropy output rather than a run of
        # one repeated byte, which would make the compression shape misleading.
        padding = "".join(
            hashlib.sha256(f"response:{turn}:{line}".encode()).hexdigest()
            for line in range((target_bytes - len(rendered)) // 64 + 1)
        )
        extra = (
            b"event: message\n"
            + b"data: "
            + json_bytes({"type": "response.output_text.delta", "delta": padding})
            + b"\n\n"
        )
        rendered += extra
    return rendered


def apply_shape_profile(args: argparse.Namespace) -> None:
    if not args.shape_profile:
        args.turns = args.turns if args.turns is not None else 77
        args.tool_lines = args.tool_lines if args.tool_lines is not None else 256
        args.duration_ms = (
            args.duration_ms
            if args.duration_ms is not None
            else max(0, (args.turns - 1) * 1_000 + 250)
        )
        return

    profile = json.loads(args.shape_profile.read_text(encoding="utf-8"))
    if profile.get("schema") != "alex-lar-shape-profile-v1":
        raise SystemExit("--shape-profile is not an alex-lar-shape-profile-v1 file")
    quantile = args.shape_quantile

    def shaped(section: str, fallback: int) -> int:
        value = profile.get(section, {}).get(quantile)
        return max(1, int(value)) if value is not None else fallback

    args.turns = (
        args.turns if args.turns is not None else shaped("session_turns", 77)
    )
    args.duration_ms = (
        args.duration_ms
        if args.duration_ms is not None
        else shaped(
            "session_duration_ms", max(0, (args.turns - 1) * 1_000 + 250)
        )
    )
    tool_bytes = (
        profile.get("artifact_logical_bytes", {})
        .get("tool-result", {})
        .get(quantile)
    )
    # Each generated tool-output line is about 100 bytes. Keep the explicit
    # option authoritative when the caller wants a controlled comparison.
    args.tool_lines = (
        args.tool_lines
        if args.tool_lines is not None
        else max(1, int(tool_bytes) // 100)
        if tool_bytes is not None
        else 256
    )
    response_bytes = (
        profile.get("artifact_logical_bytes", {})
        .get("response", {})
        .get(quantile)
    )
    args.response_bytes = (
        args.response_bytes
        if args.response_bytes is not None
        else max(1, int(response_bytes))
        if response_bytes is not None
        else None
    )


def build(args: argparse.Namespace) -> None:
    apply_shape_profile(args)
    root = args.output.resolve()
    if root.exists() and any(root.iterdir()):
        if not args.force:
            raise SystemExit(f"refusing to replace non-empty output: {root} (use --force)")
        shutil.rmtree(root)
    root.mkdir(parents=True, exist_ok=True)

    corpus = Corpus(root, args.session_id, args.seed)
    db = sqlite3.connect(root / "alexandria.sqlite3")
    db.executescript(TRACE_SCHEMA)
    messages: list[dict[str, object]] = [
        {
            "role": "system",
            "content": "Synthetic coding agent. Preserve exact tool results.",
        }
    ]
    # Real agent tool results contain paths, source, hashes, identifiers, and
    # diagnostics. A single repeated word makes each legacy gzip body tiny and
    # masks the cross-turn duplication LAR is intended to measure. Generate
    # deterministic but locally high-entropy text, then resend that exact tool
    # result in every later request just as a harness resends conversation
    # history.
    repeated_tool_blob = "".join(
        f"{line:06d} module-{line % 37:02d}/file-{line % 211:03d}.rs "
        f"{hashlib.sha256(f'{args.seed}:{line}'.encode()).hexdigest()}\n"
        for line in range(args.tool_lines)
    )
    base_ms = args.base_ms
    compact_at = max(4, args.turns * 3 // 5)
    turn_spacing_ms = max(
        1,
        (max(0, args.duration_ms - 250) // max(1, args.turns - 1))
        if args.turns > 1
        else 1,
    )

    for turn in range(args.turns):
        trace_id = f"synthetic-{turn:06d}"
        user_text = f"turn {turn}: inspect module-{turn % 11} and report deterministic result"
        messages.append({"role": "user", "content": user_text})

        if turn and turn % 7 == 0:
            call_id = f"call-{turn:06d}"
            args_bytes = json_bytes({"command": "ls -la", "cwd": f"/workspace/{turn % 5}"})
            result_bytes = json_bytes(
                {
                    "call_id": call_id,
                    "stdout": repeated_tool_blob,
                    "exit_code": 0,
                }
            )
            args_path = corpus.body(call_id, "tool-args.json", args_bytes)
            result_path = corpus.body(call_id, "tool-result.json", result_bytes)
            db.execute(
                "INSERT INTO tool_calls VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (
                    call_id,
                    "synthetic-codex",
                    args.session_id,
                    f"turn-{turn}",
                    call_id,
                    trace_id,
                    "shell",
                    base_ms + turn * turn_spacing_ms + 100,
                    base_ms + turn * turn_spacing_ms + 180,
                    0,
                    0,
                    args_path,
                    result_path,
                ),
            )
            messages.append(
                {
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": result_bytes.decode(),
                }
            )

        if turn == compact_at:
            surviving = messages[-6:]
            messages = [
                messages[0],
                {
                    "role": "system",
                    "content": f"Compacted synthetic summary through turn {turn}; "
                    "all commands succeeded and module findings were retained.",
                },
                *surviving,
            ]

        request = json_bytes(
            {
                "model": "gpt-synthetic-code",
                "messages": messages,
                "tools": [
                    {
                        "type": "function",
                        "function": {
                            "name": "shell",
                            "description": "Run a command in the synthetic workspace",
                            "parameters": {
                                "type": "object",
                                "properties": {"command": {"type": "string"}},
                            },
                        },
                    }
                ],
                "stream": True,
                "cache_control": {"turn_marker": turn % 3},
            }
        )
        response = response_sse(turn, args.response_bytes)
        req_path = corpus.body(trace_id, "request.json", request)

        # Model the legacy duplicate: passthrough stages often wrote the same
        # body under another per-exchange filename.
        upstream_path = (
            corpus.body(trace_id, "upstream-request.json", request)
            if turn % 4 != 0
            else None
        )
        resp_path = corpus.body(trace_id, "response.body", response)

        if args.include_failures and turn == args.turns - 2:
            resp_path = corpus.missing_body(trace_id, "missing-response.body")
        elif args.include_failures and turn == args.turns - 1:
            resp_path = corpus.corrupt_body(trace_id, "corrupt-response.body")

        request_headers = {
            "authorization": "[redacted]",
            "content-type": "application/json",
            "user-agent": "synthetic-agent/1",
            "x-session-id": args.session_id,
            "x-turn-bucket": str(turn % 3),
        }
        response_headers = {
            "content-type": "text/event-stream",
            "x-request-id": f"upstream-{turn:06d}",
            "cache-control": "no-cache",
        }
        attempts = [
            {
                "attempt": 1,
                "provider": "synthetic",
                "model": "gpt-synthetic-code",
                "status": 200,
            }
        ]
        if turn % 13 == 12:
            attempts.insert(
                0,
                {
                    "attempt": 0,
                    "provider": "synthetic-fallback",
                    "model": "gpt-synthetic-code",
                    "error": "synthetic_capacity",
                },
            )

        db.execute(
            "INSERT INTO traces VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                trace_id,
                base_ms + turn * turn_spacing_ms,
                base_ms + turn * turn_spacing_ms + 250,
                args.session_id,
                "synthetic-codex",
                "openai-responses",
                "synthetic",
                "openai-responses",
                "gpt-synthetic-code",
                "gpt-synthetic-code",
                "POST",
                "/v1/responses",
                200,
                1,
                req_path,
                upstream_path,
                resp_path,
                json.dumps(request_headers, separators=(",", ":")),
                json.dumps(response_headers, separators=(",", ":")),
                json.dumps(attempts, separators=(",", ":")),
                None,
                "synthetic-run",
                json.dumps({"corpus": "lar", "branch": turn % 5}),
            ),
        )
        messages.append({"role": "assistant", "content": f"answer-{turn}-synthetic"})

    # A deliberately small second session lets the packaged Trace Browser
    # benchmark leave a delayed page request in flight, navigate elsewhere,
    # and prove that the stale long-session response is discarded. Keep this
    # opt-in so existing corpus/storage benchmarks retain their exact shape.
    short_messages: list[dict[str, object]] = [
        {"role": "system", "content": "Synthetic short-session fixture."}
    ]
    short_base_ms = base_ms + args.duration_ms + 1_000
    for turn in range(args.short_session_turns):
        trace_id = f"short-synthetic-{turn:06d}"
        short_messages.append(
            {"role": "user", "content": f"short turn {turn}: deterministic navigation fixture"}
        )
        request = json_bytes(
            {
                "model": "gpt-synthetic-code",
                "messages": short_messages,
                "stream": True,
            }
        )
        response = response_sse(args.turns + turn)
        req_path = corpus.body(trace_id, "request.json", request)
        upstream_path = corpus.body(trace_id, "upstream-request.json", request)
        resp_path = corpus.body(trace_id, "response.body", response)
        ts_request_ms = short_base_ms + turn * 1_000
        db.execute(
            "INSERT INTO traces VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                trace_id,
                ts_request_ms,
                ts_request_ms + 250,
                args.short_session_id,
                "synthetic-codex",
                "openai-responses",
                "synthetic",
                "openai-responses",
                "gpt-synthetic-code",
                "gpt-synthetic-code",
                "POST",
                "/v1/responses",
                200,
                1,
                req_path,
                upstream_path,
                resp_path,
                json.dumps(
                    {
                        "authorization": "[redacted]",
                        "content-type": "application/json",
                        "user-agent": "synthetic-agent/1",
                        "x-session-id": args.short_session_id,
                    },
                    separators=(",", ":"),
                ),
                json.dumps(
                    {"content-type": "text/event-stream", "cache-control": "no-cache"},
                    separators=(",", ":"),
                ),
                json.dumps(
                    [
                        {
                            "attempt": 1,
                            "provider": "synthetic",
                            "model": "gpt-synthetic-code",
                            "status": 200,
                        }
                    ],
                    separators=(",", ":"),
                ),
                None,
                "synthetic-short-run",
                json.dumps({"corpus": "lar", "fixture": "short"}),
            ),
        )
        short_messages.append(
            {"role": "assistant", "content": f"short-answer-{turn}-synthetic"}
        )

    db.commit()
    db.close()
    summary = {
        "schema": "alex-lar-synthetic-corpus-v1",
        "generated_unix_ms": int(time.time() * 1000),
        "seed": args.seed,
        "session_id": args.session_id,
        "turns": args.turns,
        "base_ms": base_ms,
        "short_session_id": args.short_session_id,
        "short_session_turns": args.short_session_turns,
        "tool_lines": args.tool_lines,
        "response_target_bytes": args.response_bytes,
        "duration_ms": args.duration_ms,
        "turn_spacing_ms": turn_spacing_ms,
        "shape_profile": bool(args.shape_profile),
        "shape_quantile": args.shape_quantile if args.shape_profile else None,
        "compaction_turn": compact_at,
        "artifacts": corpus.manifest,
        "valid_uncompressed_bytes": sum(
            int(item.get("length", 0))
            for item in corpus.manifest
            if item["state"] == "valid"
        ),
    }
    (root / "corpus-manifest.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(
        json.dumps(
            {
                "output": str(root),
                "turns": args.turns,
                "artifacts": len(corpus.manifest),
                "valid_uncompressed_bytes": summary["valid_uncompressed_bytes"],
            },
            sort_keys=True,
        )
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--turns", type=int)
    parser.add_argument("--tool-lines", type=int)
    parser.add_argument("--duration-ms", type=int)
    parser.add_argument("--response-bytes", type=int)
    parser.add_argument(
        "--shape-profile",
        type=Path,
        help="aggregate profile emitted by measure-lar-corpus.py",
    )
    parser.add_argument(
        "--shape-quantile",
        choices=("p50", "p95", "p99", "max"),
        default="p95",
        help="which aggregate shape to reproduce (default: p95)",
    )
    parser.add_argument("--seed", type=int, default=1784466165325)
    parser.add_argument(
        "--base-ms",
        type=int,
        default=1_768_435_200_000,
        help="request timestamp for the first long-session turn",
    )
    parser.add_argument(
        "--session-id", default="019f6872-a3ee-7431-b4bb-2bafbabb7235-synthetic"
    )
    parser.add_argument(
        "--short-session-id", default="synthetic-short-session-navigation"
    )
    parser.add_argument(
        "--short-session-turns",
        type=int,
        default=0,
        help="also generate a distinct short session for navigation/cancellation tests",
    )
    parser.add_argument("--include-failures", action="store_true")
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    if args.turns is not None and args.turns < 1:
        parser.error("--turns must be at least 1")
    if args.tool_lines is not None and args.tool_lines < 1:
        parser.error("--tool-lines must be at least 1")
    if args.duration_ms is not None and args.duration_ms < 0:
        parser.error("--duration-ms must not be negative")
    if args.response_bytes is not None and args.response_bytes < 1:
        parser.error("--response-bytes must be at least 1")
    if args.base_ms < 0:
        parser.error("--base-ms must not be negative")
    if args.short_session_turns < 0:
        parser.error("--short-session-turns must not be negative")
    if args.short_session_turns and args.short_session_id == args.session_id:
        parser.error("--short-session-id must differ from --session-id")
    return args


if __name__ == "__main__":
    build(parse_args())
