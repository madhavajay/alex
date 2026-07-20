#!/usr/bin/env python3
"""Generate, import, and verify a synthetic legacy Alex corpus.

The benchmark measures referenced legacy gzip bytes against the complete LAR
body-pack size. It keeps its output directory so a slow trace, archive, or
catalog can be inspected after the run.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]


def run(command: list[str], *, env: dict[str, str] | None = None) -> tuple[str, float]:
    started = time.perf_counter()
    completed = subprocess.run(
        command,
        cwd=REPO,
        env=env,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    )
    return completed.stdout, time.perf_counter() - started


def referenced_legacy_paths(root: Path) -> set[Path]:
    connection = sqlite3.connect(root / "alexandria.sqlite3")
    try:
        paths: set[Path] = set()
        for row in connection.execute(
            "SELECT req_body_path, upstream_req_body_path, resp_body_path FROM traces"
        ):
            paths.update(Path(value) for value in row if value)
        for row in connection.execute(
            "SELECT args_body_path, result_body_path FROM tool_calls"
        ):
            paths.update(Path(value) for value in row if value)
        return paths
    finally:
        connection.close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="new/empty benchmark directory")
    parser.add_argument("--alex-bin", type=Path, default=REPO / "target/debug/alex")
    parser.add_argument("--turns", type=int)
    parser.add_argument(
        "--profile",
        choices=("standard", "tool-heavy"),
        help="tool-heavy uses 1,024 realistic tool-output lines instead of 256",
    )
    parser.add_argument(
        "--shape-profile",
        type=Path,
        help="aggregate profile emitted by measure-lar-corpus.py",
    )
    parser.add_argument(
        "--shape-quantile",
        choices=("p50", "p95", "p99", "max"),
        default="p95",
    )
    parser.add_argument("--json-out", type=Path)
    parser.add_argument(
        "--require-ratio",
        type=float,
        help="exit nonzero unless legacy gzip bytes / LAR bytes reaches this value",
    )
    args = parser.parse_args()
    if args.turns is not None and args.turns < 1:
        parser.error("--turns must be at least 1")
    if args.shape_profile and args.profile:
        parser.error("--shape-profile and --profile are mutually exclusive")
    return args


def main() -> None:
    args = parse_args()
    output = (
        args.output.resolve()
        if args.output
        else Path(tempfile.mkdtemp(prefix="alex-lar-benchmark-"))
    )
    if output.exists() and any(output.iterdir()):
        raise SystemExit(f"benchmark output must be empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    alex_bin = args.alex_bin.resolve()
    if not alex_bin.is_file():
        raise SystemExit(f"Alex binary not found: {alex_bin}; run `cargo build -p alex`")

    profile = args.profile or (None if args.shape_profile else "standard")
    tool_lines = 1024 if profile == "tool-heavy" else 256
    generator = [
        sys.executable,
        str(REPO / "scripts/generate-lar-corpus.py"),
        str(output),
    ]
    if args.shape_profile:
        generator.extend(
            [
                "--shape-profile",
                str(args.shape_profile.resolve()),
                "--shape-quantile",
                args.shape_quantile,
            ]
        )
    else:
        generator.extend(["--tool-lines", str(tool_lines)])
    if args.turns is not None:
        generator.extend(["--turns", str(args.turns)])
    _, generate_seconds = run(
        generator
    )
    corpus_summary = json.loads(
        (output / "corpus-manifest.json").read_text(encoding="utf-8")
    )
    turns = int(corpus_summary["turns"])
    tool_lines = int(corpus_summary["tool_lines"])
    environment = os.environ.copy()
    environment["ALEXANDRIA_HOME"] = str(output)
    imported, import_seconds = run(
        [str(alex_bin), "lar", "import-legacy", "--json"], env=environment
    )
    import_report = json.loads(imported)

    archives = sorted((output / "lar").glob("*.lar"))
    if not archives:
        raise SystemExit("import produced no .lar archives")
    verify_reports = []
    verify_seconds = 0.0
    for archive in archives:
        verified, elapsed = run(
            [str(alex_bin), "lar", "verify", str(archive), "--json"],
            env=environment,
        )
        verify_report = json.loads(verified)
        # Verification can enumerate every manifest ID; the benchmark needs
        # counts/timings, not a multi-megabyte echo of the archive index.
        verify_report.pop("manifest_ids", None)
        verify_reports.append(verify_report)
        verify_seconds += elapsed

    legacy_paths = referenced_legacy_paths(output)
    missing_paths = sorted(str(path) for path in legacy_paths if not path.is_file())
    legacy_gzip_bytes = sum(path.stat().st_size for path in legacy_paths if path.is_file())
    lar_bytes = sum(path.stat().st_size for path in archives)
    ratio = legacy_gzip_bytes / lar_bytes if lar_bytes else 0.0
    bytes_read = int(import_report["bytes_read"])
    bytes_deduplicated = int(import_report["bytes_deduplicated"])
    result = {
        "schema": "alex-lar-benchmark-v1",
        "profile": profile or f"shape-{args.shape_quantile}",
        "output": str(output),
        "turns": turns,
        "tool_lines": tool_lines,
        "referenced_legacy_artifacts": len(legacy_paths),
        "missing_legacy_artifacts": missing_paths,
        "legacy_gzip_bytes": legacy_gzip_bytes,
        "lar_bytes": lar_bytes,
        "legacy_gzip_to_lar_ratio": ratio,
        "source_uncompressed_bytes": bytes_read,
        "unique_uncompressed_bytes": int(import_report["unique_bytes_written"]),
        "deduplicated_uncompressed_bytes": bytes_deduplicated,
        "uncompressed_dedup_fraction": bytes_deduplicated / bytes_read if bytes_read else 0.0,
        "generate_seconds": generate_seconds,
        "import_seconds": import_seconds,
        "verify_seconds": verify_seconds,
        "import_report": import_report,
        "verify_reports": verify_reports,
    }
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    print(rendered, end="")
    if args.json_out:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(rendered, encoding="utf-8")
    if args.require_ratio is not None and ratio < args.require_ratio:
        raise SystemExit(
            f"storage ratio {ratio:.3f} is below required {args.require_ratio:.3f}"
        )


if __name__ == "__main__":
    main()
