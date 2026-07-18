#!/usr/bin/env python3
"""Assertions for scripts/harness-regression.sh; intentionally stdlib-only."""
import argparse
import json
import sys


def load(path):
    with open(path) as f:
        return json.load(f)


def fail(message):
    print(message)
    raise SystemExit(1)


def number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def trace(args):
    rows = load(args.traces).get("traces", [])
    rows = [r for r in rows if r.get("run_id") == args.run_id]
    if not rows:
        fail("no trace with the scoped run_id")
    row = next((r for r in rows if r.get("harness") == args.harness), rows[-1])
    for key, expected in (("harness", args.harness), ("upstream_provider", args.provider),
                          ("routed_model", args.model), ("billing_bucket", "subscription")):
        if row.get(key) != expected:
            fail(f"trace {key}={row.get(key)!r}, expected {expected!r}")
    for key in ("input_tokens", "output_tokens", "cost_usd"):
        if not number(row.get(key)):
            fail(f"trace {key} must be a number, got {row.get(key)!r}")
    for key in ("id", "session_id", "req_body_path", "resp_body_path"):
        if not row.get(key):
            fail(f"trace {key} is missing")
    print(json.dumps({"trace_id": row["id"], "session_id": row["session_id"]}))


def lineage(args):
    trace_rows = load(args.traces).get("traces", [])
    sessions = load(args.sessions).get("sessions", [])
    ids = {r.get("session_id") for r in trace_rows if r.get("run_id") == args.run_id and r.get("session_id")}
    if len(ids) < 2:
        fail("subagent task did not produce two scoped sessions")
    edges = [s for s in sessions if s.get("session_id") in ids and s.get("parent_session_id") in ids]
    if not edges:
        fail("no parent/child session_lineage edge in /traces/sessions response")
    edge = edges[0]
    if args.agent_type and edge.get("agent_type") != args.agent_type:
        fail(f"lineage agent_type={edge.get('agent_type')!r}, expected {args.agent_type!r}")
    for key in ("parent_session_id", "child_count", "subagent_started_ms"):
        if edge.get(key) is None:
            fail(f"lineage UI field {key} is missing")
    print(json.dumps({k: edge.get(k) for k in ("session_id", "parent_session_id", "lineage_turn_id", "agent_type", "child_count", "subagent_started_ms", "subagent_stopped_ms")}))


def tools(args):
    transcript = load(args.transcript)
    turns = transcript.get("turns", [])
    tool_turns = [turn for turn in turns if turn.get("executed_tools")]
    if not tool_turns:
        fail("no executed_tools rows in /traces/sessions/{id}/transcript")
    turn = tool_turns[0]
    if not isinstance(turn.get("user"), str) or not turn["user"].strip():
        fail("tool transcript turn has no non-empty outgoing user/tool-result half")
    blocks = turn.get("assistant_blocks") or []
    has_assistant = isinstance(turn.get("assistant"), str) and bool(turn["assistant"].strip())
    has_assistant_block = any(
        (block.get("type") == "text" and isinstance(block.get("text"), str) and block["text"].strip())
        or (block.get("type") == "tool_call" and isinstance(block.get("name"), str) and block["name"].strip())
        for block in blocks
        if isinstance(block, dict)
    )
    if not has_assistant and not has_assistant_block:
        fail("tool transcript turn has no non-empty model half after display reconstruction")
    if not turn.get("tool_calls"):
        fail("tool transcript turn has no assistant tool_calls entry")
    row = turn["executed_tools"][0]
    for key in ("id", "session_id", "turn_id", "tool_call_id", "tool_name", "args_body_path", "result_body_path"):
        if not row.get(key):
            fail(f"tool row {key} is missing")
    if row.get("exit_status") is None:
        fail("tool row exit_status is missing")
    print(json.dumps({"tool_id": row["id"], "tool_name": row["tool_name"]}))


parser = argparse.ArgumentParser()
sub = parser.add_subparsers(dest="command", required=True)
p = sub.add_parser("trace")
p.add_argument("--traces", required=True); p.add_argument("--run-id", required=True)
p.add_argument("--harness", required=True); p.add_argument("--provider", required=True); p.add_argument("--model", required=True)
p.set_defaults(func=trace)
p = sub.add_parser("lineage")
p.add_argument("--traces", required=True); p.add_argument("--sessions", required=True); p.add_argument("--run-id", required=True)
p.add_argument("--agent-type")
p.set_defaults(func=lineage)
p = sub.add_parser("tools")
p.add_argument("--transcript", required=True); p.set_defaults(func=tools)
args = parser.parse_args()
args.func(args)
