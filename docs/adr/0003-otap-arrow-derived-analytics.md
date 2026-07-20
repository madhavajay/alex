# ADR 0003: OTAP/Arrow is a derived analytics format

Status: accepted for v1

Date: 2026-07-20

## Context

LAR now has a stable normalized semantic view in addition to its exact raw
transport record. We evaluated whether OpenTelemetry Protocol with Apache Arrow
(OTAP), Arrow IPC, or Parquet should become Alex's canonical representation or
an optional analytics export. The evaluation uses only upstream OpenTelemetry
specifications, documentation, and source as of this date.

OTAP and LAR solve different problems. OTAP is a columnar representation and
stateful gRPC transport for the OpenTelemetry logs, metrics, and traces data
model. LAR is the authoritative archive for ordered headers, exact body bytes,
stream reads and timing, routing stages, and conversation lineage.

## Evidence

| Property | Upstream evidence | Consequence for Alex |
| --- | --- | --- |
| Fidelity | OTAP promises non-lossy OTLP-to-OTAP round trips for logs, metrics, and traces. Its validation compares logical OTLP JSON equivalence, allowing field order and batching to change. | This is strong semantic fidelity after Alex has mapped a trace to OpenTelemetry. It does not preserve LAR's byte-exact HTTP headers/bodies, SSE framing, read timing, or stage order. |
| Schema and version stability | The formal OTAP specification is `Draft`; its protobuf package is `opentelemetry.proto.experimental.arrow.v1` and says it is subject to change. The Collector components are Beta and promise a compatibility approach before intentional breakage. Stable OTLP remains the safer interoperability boundary. | Pin the OpenTelemetry semantic/export version. Do not make an experimental OTAP schema part of the LAR compatibility contract. |
| Rust maturity | The Rust `pdata` code is a low-level reference implementation in the dataflow workspace. The workspace is version 0.49.0, requires Rust 1.87, sets `publish = false`, and upstream describes the engine as incubation-stage with no prebuilt releases. | There is no small, published, stable Rust surface suitable for an Alex runtime dependency today. |
| Size | Upstream's Phase 1 target is at least 30% improvement for every signal and typically 50% over OTLP/zstd. Its detailed results vary substantially with signal, batch size, and stream lifetime; small batches pay schema overhead. | OTAP is promising for moving large semantic telemetry streams, but the upstream measurements do not predict LAR archive size and do not replace an Alex-corpus benchmark. |
| Queryability | OTAP uses several related Arrow record batches per signal, stateful dictionaries, adaptive schemas, and schema resets. Upstream uses DataFusion for in-pipeline work and exposes Parquet for broad query-tool compatibility. | Arrow batches are useful for vectorized processing. A raw OTAP stream is not a durable, directly queryable Alex archive; Parquet would be the more practical derived file if local analytics demand it. |

Primary upstream material:

- [OTAP formal specification](https://github.com/open-telemetry/otel-arrow/blob/main/docs/otap-spec.md)
- [OTAP experimental protobuf service](https://github.com/open-telemetry/otel-arrow/blob/main/proto/opentelemetry/proto/experimental/arrow/v1/arrow_service.proto)
- [Stable OTLP specification](https://opentelemetry.io/docs/specs/otlp/)
- [OTel-Arrow data model](https://github.com/open-telemetry/otel-arrow/blob/main/docs/data_model.md)
- [OTel-Arrow validation process](https://github.com/open-telemetry/otel-arrow/blob/main/docs/validation_process.md)
- [Phase 1 compatibility and compression goals](https://github.com/open-telemetry/otel-arrow/blob/main/docs/project-phases.md)
- [Phase 1 benchmark results](https://github.com/open-telemetry/otel-arrow/blob/main/docs/benchmarks-phase1.md)
- [Rust `pdata` reference implementation](https://github.com/open-telemetry/otel-arrow/blob/main/rust/otap-dataflow/crates/pdata/README.md)
- [Rust workspace manifest](https://github.com/open-telemetry/otel-arrow/blob/main/rust/otap-dataflow/Cargo.toml)
- [Collector OTAP exporter](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/exporter/otelarrowexporter/README.md)
- [Upstream maturity statement](https://opentelemetry.io/blog/2026/otel-arrow-phase-2/#current-maturity-level)

## Decision

LAR remains the canonical archive and SQLite remains the live query index.
OTAP/Arrow is an optional, lossy, derived analytics/interchange representation
of Alex's versioned normalized semantic view. It is never the source for exact
replay, retention, repair, or billing evidence.

We will not implement or embed the current Rust OTAP exporter. Alex's stable
interoperability boundary remains its version-pinned OpenTelemetry semantic
adapter with an explicit loss report. A deployment that needs OTAP transport can
convert equivalent OTLP data through the official Collector `otelarrow`
exporter instead of embedding OTAP in Alex. A future local analytics export
should prefer Parquet (or plain Arrow IPC for an in-process consumer), carry the
Alex normalized-schema version and export-loss metadata, and reference raw
artifacts by stable ID and digest instead of duplicating them by default.

This decision does not claim that OTAP is unsuitable. It says that its current
benefit is downstream columnar processing and transport compression, not
canonical capture fidelity or a stable embedded Rust API.

## Revisit criteria

Reconsider a native exporter when all of the following are true:

1. The OTAP protocol has a published compatibility/stability policy beyond its
   current draft and experimental package status.
2. The official Rust encoder is published as a supported crate with a bounded,
   versioned API, or Alex can target a stable Arrow/Parquet schema without
   depending on dataflow internals.
3. An Alex benchmark covers long agent sessions and measures export size,
   peak memory, throughput, cold-open latency, and representative DataFusion or
   DuckDB queries against OpenTelemetry JSONL, OTAP, and Parquet candidates.
4. Conformance tests prove the pinned semantic mapping round-trips through OTLP
   and OTAP, and enumerate every LAR field omitted from the derived output.

## Consequences

- No new runtime dependency or exporter is added in v1.
- LAR's one-copy body invariant and byte-exact replay contract stay unchanged.
- OTAP can be adopted later without a LAR migration because it is derived from
  the versioned normalized view.
- Any future Arrow/Parquet export must be rebuildable, disposable, schema-pinned,
  and accompanied by the same explicit loss accounting as other semantic
  exports.
