#!/usr/bin/env python3
import argparse
import json
import os
import sys


def main():
    ap = argparse.ArgumentParser(description="Assert a proxy trace row matches expected routing")
    ap.add_argument("--traces", required=True)
    ap.add_argument("--session", required=True)
    ap.add_argument("--provider")
    ap.add_argument("--format-prefix")
    ap.add_argument("--bucket")
    ap.add_argument("--routed")
    ap.add_argument("--cross", action="store_true")
    ap.add_argument("--expect-dario", action="store_true")
    a = ap.parse_args()

    fails = []
    warns = []
    try:
        with open(a.traces) as f:
            data = json.load(f)
    except Exception as e:
        print(f"cannot read traces json: {e}")
        return 1

    traces = [t for t in data.get("traces", []) if t.get("session_id") == a.session]
    if not traces:
        print(f"no trace row for session {a.session}")
        return 1
    t = traces[0]

    def chk(cond, msg):
        if not cond:
            fails.append(msg)

    chk(t.get("status") == 200, f"trace status={t.get('status')} want 200")
    if a.provider:
        chk(t.get("upstream_provider") == a.provider,
            f"upstream_provider={t.get('upstream_provider')} want {a.provider}")
    if a.format_prefix:
        uf = t.get("upstream_format") or ""
        chk(uf.startswith(a.format_prefix),
            f"upstream_format={uf!r} want prefix {a.format_prefix!r}")
    if a.bucket:
        chk(t.get("billing_bucket") == a.bucket,
            f"billing_bucket={t.get('billing_bucket')} want {a.bucket}")
    if a.routed:
        chk(t.get("routed_model") == a.routed,
            f"routed_model={t.get('routed_model')} want {a.routed}")

    itok = t.get("input_tokens")
    otok = t.get("output_tokens")
    chk(itok is not None or otok is not None, "no usage tokens in trace")
    if t.get("cost_usd") is None:
        warns.append("cost_usd null")

    for key in ("req_body_path", "resp_body_path"):
        p = t.get(key)
        if not p:
            fails.append(f"{key} missing from trace")
        elif not os.path.isfile(p):
            fails.append(f"{key} not on disk: {p}")

    chk(t.get("error") in (None, ""), f"error={t.get('error')!r} want null")

    if a.cross:
        chk(t.get("client_format") != t.get("upstream_format"),
            f"client_format==upstream_format=={t.get('client_format')!r} "
            "(translation path not exercised)")

    if a.expect_dario:
        blob = json.dumps(t).lower()
        chk("dario" in blob or "generation" in blob,
            "trace does not reference a dario generation")

    if fails:
        print("; ".join(fails))
        return 1
    msg = f"tokens={itok}/{otok} cost={t.get('cost_usd')}"
    if warns:
        msg += " [warn: " + ", ".join(warns) + "]"
    print(msg)
    return 0


if __name__ == "__main__":
    sys.exit(main())
