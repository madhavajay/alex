#!/usr/bin/env python3
import json
import os
import re
import sys


def sort_key(cid):
    if cid == "UNIT":
        rank = 0
    elif cid.startswith("PING-"):
        rank = 1
    elif cid.startswith("W"):
        rank = 2
    elif cid.startswith("H"):
        rank = 3
    else:
        rank = 4
    parts = [(0, int(p)) if p.isdigit() else (1, p)
             for p in re.split(r"(\d+)", cid) if p]
    return (rank, parts)


def main():
    args = sys.argv[1:]
    as_json = "--json" in args
    dirs = [a for a in args if not a.startswith("--")]
    if not dirs or not os.path.isdir(dirs[0]):
        print("usage: test-report.py <results-dir> [--json]", file=sys.stderr)
        return 2

    rows = []
    for name in sorted(os.listdir(dirs[0])):
        if not name.endswith(".res"):
            continue
        try:
            with open(os.path.join(dirs[0], name)) as f:
                line = f.readline().rstrip("\n")
        except OSError:
            continue
        fields = line.split("\t", 3)
        if len(fields) < 3:
            continue
        rows.append({
            "id": fields[0],
            "status": fields[1],
            "ms": int(fields[2]) if fields[2].isdigit() else 0,
            "message": fields[3] if len(fields) > 3 else "",
        })
    rows.sort(key=lambda r: sort_key(r["id"]))

    summary = {
        "pass": sum(r["status"] == "PASS" for r in rows),
        "fail": sum(r["status"] == "FAIL" for r in rows),
        "skip": sum(r["status"] == "SKIP" for r in rows),
    }

    if as_json:
        print(json.dumps({"cells": rows, "summary": summary}, indent=2))
    else:
        if not rows:
            print("no cells selected")
            return 0
        tty = sys.stdout.isatty()
        colors = {"PASS": "\033[32m", "FAIL": "\033[31m", "SKIP": "\033[33m"} if tty else {}
        reset = "\033[0m" if tty else ""
        wid = max(len(r["id"]) for r in rows)
        wid = max(wid, 2)
        print(f"{'ID':<{wid}}  {'STATUS':<6}  {'TIME':>9}  MESSAGE")
        for r in rows:
            c = colors.get(r["status"], "")
            t = f"{r['ms']}ms" if r["ms"] else "-"
            print(f"{r['id']:<{wid}}  {c}{r['status']:<6}{reset}  {t:>9}  {r['message']}")
        print(f"\n{summary['pass']} pass, {summary['fail']} fail, {summary['skip']} skip")
    return 1 if summary["fail"] else 0


if __name__ == "__main__":
    sys.exit(main())
