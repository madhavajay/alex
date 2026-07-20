#!/usr/bin/env python3
"""Measure a legacy Alex corpus without emitting captured content or identifiers.

The input database and body files are opened read-only. The JSON report contains
only aggregate counts, byte sizes, distributions, and duplication ratios. It
never contains body text, header values, filesystem paths, trace IDs, session
IDs, or body hashes. A separate aggregate shape profile can be fed to
``generate-lar-corpus.py`` to create privacy-safe benchmark data.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import sqlite3
import sys
from collections import Counter, defaultdict
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO, Iterator
from urllib.parse import quote


MIB = 1024 * 1024
FINE_BOUNDS = (512, 2 * 1024, 8 * 1024)
LARGE_BOUNDS = (2 * 1024, 8 * 1024, 32 * 1024)
LARGE_BODY_THRESHOLD = 8 * MIB
READ_SIZE = 1024 * 1024
MASK64 = (1 << 64) - 1


@dataclass(frozen=True)
class Artifact:
    owner: str
    session: str | None
    timestamp_ms: int
    kind: str
    path: Path


@dataclass
class Scan:
    state: str
    compressed_bytes: int = 0
    logical_bytes: int = 0
    digest: bytes | None = None


def percentile(sorted_values: list[int], fraction: float) -> int | None:
    if not sorted_values:
        return None
    index = max(0, min(len(sorted_values) - 1, int((len(sorted_values) - 1) * fraction)))
    return sorted_values[index]


def distribution(values: list[int]) -> dict[str, int | float | None]:
    ordered = sorted(values)
    total = sum(ordered)
    return {
        "count": len(ordered),
        "sum": total,
        "min": ordered[0] if ordered else None,
        "p50": percentile(ordered, 0.50),
        "p95": percentile(ordered, 0.95),
        "p99": percentile(ordered, 0.99),
        "max": ordered[-1] if ordered else None,
        "mean": total / len(ordered) if ordered else None,
    }


def counter_distribution(lengths: Counter[int]) -> dict[str, object]:
    count = sum(lengths.values())
    total = sum(length * occurrences for length, occurrences in lengths.items())

    def weighted_percentile(fraction: float) -> int | None:
        if count == 0:
            return None
        target = max(1, int((count - 1) * fraction) + 1)
        seen = 0
        for length, occurrences in sorted(lengths.items()):
            seen += occurrences
            if seen >= target:
                return length
        return max(lengths)

    buckets = {
        "0-511": 0,
        "512-1023": 0,
        "1k-2k": 0,
        "2k-4k": 0,
        "4k-8k": 0,
        "8k-16k": 0,
        "16k-32k": 0,
        ">32k": 0,
    }
    for length, occurrences in lengths.items():
        if length < 512:
            key = "0-511"
        elif length < 1024:
            key = "512-1023"
        elif length < 2048:
            key = "1k-2k"
        elif length < 4096:
            key = "2k-4k"
        elif length < 8192:
            key = "4k-8k"
        elif length < 16384:
            key = "8k-16k"
        elif length <= 32768:
            key = "16k-32k"
        else:
            key = ">32k"
        buckets[key] += occurrences
    return {
        "count": count,
        "bytes": total,
        "min": min(lengths) if lengths else None,
        "p50": weighted_percentile(0.50),
        "p95": weighted_percentile(0.95),
        "p99": weighted_percentile(0.99),
        "max": max(lengths) if lengths else None,
        "mean": total / count if count else None,
        "buckets": buckets,
    }


def gear_value(byte: int) -> int:
    value = (byte + 0x9E3779B97F4A7C15) & MASK64
    value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
    value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & MASK64
    return (value ^ (value >> 31)) & MASK64


GEAR_TABLE = tuple(gear_value(byte) for byte in range(256))


def chunk_lengths(
    stream: BinaryIO,
    bounds: tuple[int, int, int],
    logical_bytes: int,
    byte_limit: int,
) -> tuple[Counter[int], int, bool]:
    minimum, target, maximum = bounds
    mask = (1 << (target - 1).bit_length()) - 1
    result: Counter[int] = Counter()
    rolling = 0
    pending = 0
    selected_bytes = logical_bytes if byte_limit == 0 else min(logical_bytes, byte_limit)
    consumed = 0
    while consumed < selected_bytes:
        block = stream.read(min(READ_SIZE, selected_bytes - consumed))
        if not block:
            break
        consumed += len(block)
        for byte in block:
            pending += 1
            rolling = (
                ((rolling << 1) | (rolling >> 63)) + GEAR_TABLE[byte]
            ) & MASK64
            if pending >= maximum or (pending >= minimum and rolling & mask == 0):
                result[pending] += 1
                pending = 0
                rolling = 0
    complete = consumed == logical_bytes
    # A deliberately truncated sample does not invent a short terminal chunk.
    if pending and complete:
        result[pending] += 1
    return result, consumed, complete


@contextmanager
def body_stream(path: Path) -> Iterator[BinaryIO]:
    raw = path.open("rb")
    try:
        magic = raw.read(2)
        raw.seek(0)
        if magic == b"\x1f\x8b":
            with gzip.GzipFile(fileobj=raw, mode="rb") as decoded:
                yield decoded
        else:
            yield raw
    finally:
        raw.close()


def scan_body(path: Path) -> Scan:
    try:
        compressed_bytes = path.stat().st_size
    except (FileNotFoundError, OSError):
        return Scan("missing")
    digest = hashlib.sha256()
    logical_bytes = 0
    try:
        with body_stream(path) as stream:
            while block := stream.read(READ_SIZE):
                digest.update(block)
                logical_bytes += len(block)
        return Scan("valid", compressed_bytes, logical_bytes, digest.digest())
    except (OSError, EOFError, gzip.BadGzipFile):
        return Scan("corrupt", compressed_bytes=compressed_bytes)


def common_prefix_bytes(left: Path, right: Path) -> int:
    matched = 0
    with body_stream(left) as first, body_stream(right) as second:
        left_pending = b""
        right_pending = b""
        left_done = False
        right_done = False
        while True:
            if not left_pending and not left_done:
                left_pending = first.read(READ_SIZE)
                left_done = not left_pending
            if not right_pending and not right_done:
                right_pending = second.read(READ_SIZE)
                right_done = not right_pending
            if not left_pending or not right_pending:
                return matched
            compared = min(len(left_pending), len(right_pending))
            left_slice = left_pending[:compared]
            right_slice = right_pending[:compared]
            if left_slice != right_slice:
                # Equality checks run in C. Narrow to a small block before the
                # byte-wise search so long common JSON prefixes stay cheap.
                block_size = 64 * 1024
                for start in range(0, compared, block_size):
                    end = min(compared, start + block_size)
                    if left_slice[start:end] != right_slice[start:end]:
                        for index, (a, b) in enumerate(
                            zip(left_slice[start:end], right_slice[start:end])
                        ):
                            if a != b:
                                return matched + start + index
            matched += compared
            left_pending = left_pending[compared:]
            right_pending = right_pending[compared:]


def table_columns(connection: sqlite3.Connection, table: str) -> set[str]:
    return {str(row[1]) for row in connection.execute(f"PRAGMA table_info({table})")}


def resolve_body_path(value: str, body_root: Path) -> Path:
    path = Path(value).expanduser()
    return path if path.is_absolute() else body_root / path


def inventory(
    connection: sqlite3.Connection,
    body_root: Path,
    session_id: str | None,
) -> tuple[list[Artifact], list[tuple[str, str, int, int]], list[tuple[str, str]]]:
    trace_columns = table_columns(connection, "traces")
    required = {"id", "ts_request_ms"}
    if not required.issubset(trace_columns):
        raise SystemExit("input database does not contain a compatible traces table")
    if session_id and "session_id" not in trace_columns:
        raise SystemExit("input database cannot filter by session_id")
    wanted = [
        "id",
        "session_id",
        "ts_request_ms",
        "ts_response_ms",
        "req_body_path",
        "upstream_req_body_path",
        "resp_body_path",
        "req_headers_json",
        "resp_headers_json",
    ]
    selected = [column for column in wanted if column in trace_columns]
    where = " WHERE session_id=?" if session_id and "session_id" in selected else ""
    parameters = (session_id,) if where else ()
    artifacts: list[Artifact] = []
    sessions: list[tuple[str, str, int, int]] = []
    header_blocks: list[tuple[str, str]] = []
    for values in connection.execute(
        f"SELECT {', '.join(selected)} FROM traces{where} ORDER BY ts_request_ms, id",
        parameters,
    ):
        row = dict(zip(selected, values))
        owner = str(row["id"])
        session = str(row["session_id"]) if row.get("session_id") else None
        started = int(row["ts_request_ms"])
        finished = int(row.get("ts_response_ms") or started)
        sessions.append((owner, session or f"missing:{owner}", started, finished))
        for column, kind in (
            ("req_body_path", "request"),
            ("upstream_req_body_path", "upstream-request"),
            ("resp_body_path", "response"),
        ):
            if row.get(column):
                artifacts.append(
                    Artifact(owner, session, started, kind, resolve_body_path(str(row[column]), body_root))
                )
        for column, kind in (
            ("req_headers_json", "request"),
            ("resp_headers_json", "response"),
        ):
            if row.get(column):
                header_blocks.append((kind, str(row[column])))

    tool_columns = table_columns(connection, "tool_calls")
    if {"id", "session_id", "ts_start_ms"}.issubset(tool_columns):
        wanted = ["id", "session_id", "ts_start_ms", "args_body_path", "result_body_path"]
        selected = [column for column in wanted if column in tool_columns]
        where = " WHERE session_id=?" if session_id else ""
        parameters = (session_id,) if where else ()
        for values in connection.execute(
            f"SELECT {', '.join(selected)} FROM tool_calls{where} ORDER BY ts_start_ms, id",
            parameters,
        ):
            row = dict(zip(selected, values))
            owner = f"tool:{row['id']}"
            session = str(row["session_id"]) if row.get("session_id") else None
            timestamp = int(row["ts_start_ms"])
            for column, kind in (
                ("args_body_path", "tool-arguments"),
                ("result_body_path", "tool-result"),
            ):
                if row.get(column):
                    artifacts.append(
                        Artifact(owner, session, timestamp, kind, resolve_body_path(str(row[column]), body_root))
                    )
    return artifacts, sessions, header_blocks


def duplication_report(scans: list[tuple[Artifact, Scan]]) -> dict[str, object]:
    valid = [(artifact, scan) for artifact, scan in scans if scan.state == "valid"]
    groups: dict[tuple[int, bytes], list[tuple[Artifact, Scan]]] = defaultdict(list)
    for artifact, scan in valid:
        assert scan.digest is not None
        groups[(scan.logical_bytes, scan.digest)].append((artifact, scan))
    duplicate_artifacts = sum(len(group) - 1 for group in groups.values())
    duplicate_bytes = sum(
        (len(group) - 1) * key[0] for key, group in groups.items()
    )
    logical_bytes = sum(scan.logical_bytes for _, scan in valid)

    by_kind: dict[str, dict[str, int | float]] = {}
    for kind in sorted({artifact.kind for artifact, _ in valid}):
        subset = [(artifact, scan) for artifact, scan in valid if artifact.kind == kind]
        subset_groups: dict[tuple[int, bytes], int] = Counter(
            (scan.logical_bytes, scan.digest) for _, scan in subset
        )
        repeated = sum(count - 1 for count in subset_groups.values())
        repeated_bytes = sum(
            (count - 1) * key[0] for key, count in subset_groups.items()
        )
        subset_bytes = sum(scan.logical_bytes for _, scan in subset)
        by_kind[kind] = {
            "valid_artifacts": len(subset),
            "unique_bodies": len(subset_groups),
            "duplicate_artifacts": repeated,
            "duplicate_logical_bytes": repeated_bytes,
            "duplicate_byte_fraction": repeated_bytes / subset_bytes if subset_bytes else 0.0,
        }

    owner_parts: dict[str, dict[str, Scan]] = defaultdict(dict)
    for artifact, scan in valid:
        owner_parts[artifact.owner][artifact.kind] = scan
    passthrough = 0
    passthrough_identical = 0
    passthrough_bytes = 0
    for parts in owner_parts.values():
        request = parts.get("request")
        upstream = parts.get("upstream-request")
        if request and upstream:
            passthrough += 1
            if request.digest == upstream.digest and request.logical_bytes == upstream.logical_bytes:
                passthrough_identical += 1
                passthrough_bytes += request.logical_bytes
    return {
        "valid_artifacts": len(valid),
        "unique_bodies": len(groups),
        "duplicate_artifacts": duplicate_artifacts,
        "duplicate_logical_bytes": duplicate_bytes,
        "duplicate_byte_fraction": duplicate_bytes / logical_bytes if logical_bytes else 0.0,
        "client_upstream_pairs": passthrough,
        "identical_client_upstream_pairs": passthrough_identical,
        "identical_client_upstream_fraction": passthrough_identical / passthrough if passthrough else 0.0,
        "identical_client_upstream_bytes": passthrough_bytes,
        "by_kind": by_kind,
    }


def prefix_report(scans: list[tuple[Artifact, Scan]]) -> dict[str, object]:
    previous: dict[tuple[str, str], tuple[Artifact, Scan]] = {}
    rows: dict[str, dict[str, int]] = defaultdict(
        lambda: {
            "comparisons": 0,
            "current_logical_bytes": 0,
            "common_prefix_bytes": 0,
            "complete_predecessor_prefixes": 0,
            "complete_predecessor_prefix_bytes": 0,
        }
    )
    for artifact, scan in sorted(scans, key=lambda item: (item[0].timestamp_ms, item[0].owner, item[0].kind)):
        if scan.state != "valid" or artifact.session is None:
            continue
        key = (artifact.session, artifact.kind)
        prior = previous.get(key)
        if prior:
            prior_artifact, prior_scan = prior
            if scan.digest == prior_scan.digest and scan.logical_bytes == prior_scan.logical_bytes:
                common = scan.logical_bytes
            else:
                try:
                    common = common_prefix_bytes(prior_artifact.path, artifact.path)
                except (OSError, EOFError, gzip.BadGzipFile):
                    common = 0
            values = rows[artifact.kind]
            values["comparisons"] += 1
            values["current_logical_bytes"] += scan.logical_bytes
            values["common_prefix_bytes"] += common
            if common == prior_scan.logical_bytes and prior_scan.logical_bytes <= scan.logical_bytes:
                values["complete_predecessor_prefixes"] += 1
                values["complete_predecessor_prefix_bytes"] += common
        previous[key] = (artifact, scan)

    def finish(values: dict[str, int]) -> dict[str, int | float]:
        current = values["current_logical_bytes"]
        return {
            **values,
            "common_prefix_fraction": values["common_prefix_bytes"] / current if current else 0.0,
        }

    total = {key: sum(values[key] for values in rows.values()) for key in next(iter(rows.values()), {
        "comparisons": 0,
        "current_logical_bytes": 0,
        "common_prefix_bytes": 0,
        "complete_predecessor_prefixes": 0,
        "complete_predecessor_prefix_bytes": 0,
    })}
    return {"all": finish(total), "by_kind": {kind: finish(rows[kind]) for kind in sorted(rows)}}


def header_report(blocks: list[tuple[str, str]]) -> dict[str, object]:
    def summarize(subset: list[str]) -> dict[str, int | float]:
        block_hashes: Counter[bytes] = Counter()
        atom_hashes: Counter[bytes] = Counter()
        bytes_seen = 0
        parse_errors = 0
        atoms = 0
        for raw in subset:
            encoded = raw.encode("utf-8", errors="surrogatepass")
            bytes_seen += len(encoded)
            block_hashes[hashlib.sha256(encoded).digest()] += 1
            try:
                value = json.loads(raw)
                pairs = value.items() if isinstance(value, dict) else value
                for name, atom_value in pairs:
                    atom = json.dumps([name, atom_value], separators=(",", ":"), ensure_ascii=False).encode()
                    atom_hashes[hashlib.sha256(atom).digest()] += 1
                    atoms += 1
            except (ValueError, TypeError):
                parse_errors += 1
        duplicate_blocks = sum(count - 1 for count in block_hashes.values())
        duplicate_atoms = sum(count - 1 for count in atom_hashes.values())
        return {
            "blocks": len(subset),
            "serialized_bytes": bytes_seen,
            "unique_blocks": len(block_hashes),
            "duplicate_blocks": duplicate_blocks,
            "duplicate_block_fraction": duplicate_blocks / len(subset) if subset else 0.0,
            "atoms": atoms,
            "unique_atoms": len(atom_hashes),
            "duplicate_atoms": duplicate_atoms,
            "duplicate_atom_fraction": duplicate_atoms / atoms if atoms else 0.0,
            "parse_errors": parse_errors,
        }

    return {
        "all": summarize([raw for _, raw in blocks]),
        "by_kind": {
            kind: summarize([raw for candidate, raw in blocks if candidate == kind])
            for kind in sorted({kind for kind, _ in blocks})
        },
    }


def evenly_spread(items: list[tuple[Artifact, Scan]], limit: int) -> list[tuple[Artifact, Scan]]:
    unique: list[tuple[Artifact, Scan]] = []
    seen: set[Path] = set()
    for item in items:
        if item[0].path not in seen:
            unique.append(item)
            seen.add(item[0].path)
    if limit == 0 or len(unique) <= limit:
        return unique
    if limit == 1:
        return [unique[len(unique) // 2]]
    indices = {
        round(index * (len(unique) - 1) / (limit - 1)) for index in range(limit)
    }
    return [unique[index] for index in sorted(indices)]


def sampled_chunks(
    subset: list[tuple[Artifact, Scan]],
    artifact_limit: int,
    byte_limit: int,
) -> tuple[Counter[int], dict[str, int | bool]]:
    chunks: Counter[int] = Counter()
    sampled_bytes = 0
    complete_artifacts = 0
    truncated_artifacts = 0
    selected = evenly_spread(subset, artifact_limit)
    for artifact, scan in selected:
        remaining = 0 if byte_limit == 0 else max(0, byte_limit - sampled_bytes)
        if byte_limit != 0 and remaining == 0:
            break
        bounds = LARGE_BOUNDS if scan.logical_bytes >= LARGE_BODY_THRESHOLD else FINE_BOUNDS
        try:
            with body_stream(artifact.path) as stream:
                measured, consumed, complete = chunk_lengths(
                    stream, bounds, scan.logical_bytes, remaining
                )
        except (OSError, EOFError, gzip.BadGzipFile):
            continue
        chunks.update(measured)
        sampled_bytes += consumed
        if complete:
            complete_artifacts += 1
        else:
            truncated_artifacts += 1
    return chunks, {
        "candidate_valid_artifacts": len(subset),
        "selected_artifacts": len(selected),
        "complete_artifacts": complete_artifacts,
        "truncated_artifacts": truncated_artifacts,
        "sampled_logical_bytes": sampled_bytes,
        "artifact_limit": artifact_limit,
        "byte_limit_per_kind": byte_limit,
        "unbounded": artifact_limit == 0 and byte_limit == 0,
    }


def build_report(
    artifacts: list[Artifact],
    session_rows: list[tuple[str, str, int, int]],
    headers: list[tuple[str, str]],
    chunk_sample_artifacts: int,
    chunk_sample_bytes: int,
) -> dict[str, object]:
    path_cache: dict[Path, Scan] = {}
    scans: list[tuple[Artifact, Scan]] = []
    for index, artifact in enumerate(artifacts, 1):
        scan = path_cache.get(artifact.path)
        if scan is None:
            scan = scan_body(artifact.path)
            path_cache[artifact.path] = scan
        scans.append((artifact, scan))
        if index % 500 == 0:
            print(f"measured {index}/{len(artifacts)} artifact references", file=sys.stderr)

    per_kind: dict[str, dict[str, object]] = {}
    all_chunks: Counter[int] = Counter()
    chunk_coverage: dict[str, dict[str, int | bool]] = {}
    for kind in sorted({artifact.kind for artifact in artifacts}):
        subset = [(artifact, scan) for artifact, scan in scans if artifact.kind == kind]
        valid_subset = [(artifact, scan) for artifact, scan in subset if scan.state == "valid"]
        chunks, coverage = sampled_chunks(
            valid_subset, chunk_sample_artifacts, chunk_sample_bytes
        )
        chunk_coverage[kind] = coverage
        all_chunks.update(chunks)
        per_kind[kind] = {
            "references": len(subset),
            "valid": sum(scan.state == "valid" for _, scan in subset),
            "missing": sum(scan.state == "missing" for _, scan in subset),
            "corrupt": sum(scan.state == "corrupt" for _, scan in subset),
            "compressed_bytes": sum(scan.compressed_bytes for _, scan in subset),
            "logical_bytes": sum(scan.logical_bytes for _, scan in subset if scan.state == "valid"),
            "logical_size_distribution": distribution(
                [scan.logical_bytes for _, scan in subset if scan.state == "valid"]
            ),
            "adaptive_gear_chunk_sizes": counter_distribution(chunks),
            "chunk_sample": coverage,
        }

    sessions: dict[str, tuple[int, int, int]] = {}
    for _owner, session, started, finished in session_rows:
        if session not in sessions:
            sessions[session] = (1, started, max(started, finished))
        else:
            turns, first, last = sessions[session]
            sessions[session] = (turns + 1, min(first, started), max(last, finished, started))
    turns = [values[0] for values in sessions.values()]
    durations = [values[2] - values[1] for values in sessions.values()]
    logical_sizes = {
        kind: per_kind[kind]["logical_size_distribution"] for kind in per_kind
    }
    shape_profile = {
        "schema": "alex-lar-shape-profile-v1",
        "session_turns": distribution(turns),
        "session_duration_ms": distribution(durations),
        "artifact_logical_bytes": logical_sizes,
    }
    return {
        "schema": "alex-lar-private-corpus-measurement-v1",
        "privacy": "aggregate-only; no content, paths, identifiers, or digests",
        "artifact_references": len(artifacts),
        "unique_source_paths": len(path_cache),
        "unique_source_compressed_bytes": sum(
            scan.compressed_bytes for scan in path_cache.values()
        ),
        "unique_source_states": {
            state: sum(scan.state == state for scan in path_cache.values())
            for state in ("valid", "missing", "corrupt")
        },
        "artifact_states": {
            state: sum(scan.state == state for _, scan in scans)
            for state in ("valid", "missing", "corrupt")
        },
        "referenced_compressed_bytes": sum(
            scan.compressed_bytes for _, scan in scans
        ),
        "logical_bytes": sum(scan.logical_bytes for _, scan in scans if scan.state == "valid"),
        "artifacts_by_kind": per_kind,
        "whole_body_duplication": duplication_report(scans),
        "consecutive_session_prefix_duplication": prefix_report(scans),
        "adaptive_gear_chunk_sizes": counter_distribution(all_chunks),
        "chunk_sample_by_kind": chunk_coverage,
        "headers": header_report(headers),
        "sessions": {
            "count": len(sessions),
            "turns": distribution(turns),
            "duration_ms": distribution(durations),
        },
        "shape_profile": shape_profile,
        "notes": [
            "logical bytes count artifact references; one source path may be referenced more than once",
            "prefix duplication compares consecutive valid artifacts of the same kind within each session",
            "chunk-size distributions are bounded, evenly spread samples using production adaptive Gear bounds: 512/2k/8k below 8 MiB and 2k/8k/32k at or above 8 MiB",
            "legacy header JSON cannot recover wire ordering or casing that was not captured",
        ],
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path, help="path to alexandria.sqlite3")
    parser.add_argument(
        "--body-root",
        type=Path,
        help="base for relative body paths (defaults to the database directory)",
    )
    parser.add_argument("--session-id", help="measure one session without emitting its ID")
    parser.add_argument("--json-out", type=Path, help="write the aggregate report")
    parser.add_argument("--shape-out", type=Path, help="write only the anonymized shape profile")
    parser.add_argument(
        "--chunk-sample-artifacts-per-kind",
        type=int,
        default=64,
        help="evenly spread bodies per artifact kind for CDC distribution; 0 means all",
    )
    parser.add_argument(
        "--chunk-sample-bytes-per-kind",
        type=int,
        default=32 * MIB,
        help="decompressed CDC sample budget per artifact kind; 0 means unbounded",
    )
    args = parser.parse_args()
    if args.chunk_sample_artifacts_per_kind < 0:
        parser.error("--chunk-sample-artifacts-per-kind must not be negative")
    if args.chunk_sample_bytes_per_kind < 0:
        parser.error("--chunk-sample-bytes-per-kind must not be negative")
    return args


def main() -> None:
    args = parse_args()
    database = args.database.resolve()
    body_root = (args.body_root or database.parent).resolve()
    uri = f"file:{quote(database.as_posix(), safe='/:')}?mode=ro"
    try:
        connection = sqlite3.connect(uri, uri=True)
    except sqlite3.Error as error:
        raise SystemExit(f"cannot open corpus database read-only: {error}") from error
    try:
        artifacts, sessions, headers = inventory(
            connection, body_root, args.session_id
        )
    finally:
        connection.close()
    if not artifacts:
        raise SystemExit("no referenced body artifacts matched the selection")
    report = build_report(
        artifacts,
        sessions,
        headers,
        args.chunk_sample_artifacts_per_kind,
        args.chunk_sample_bytes_per_kind,
    )
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    print(rendered, end="")
    if args.json_out:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(rendered, encoding="utf-8")
    if args.shape_out:
        args.shape_out.parent.mkdir(parents=True, exist_ok=True)
        args.shape_out.write_text(
            json.dumps(report["shape_profile"], indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )


if __name__ == "__main__":
    main()
