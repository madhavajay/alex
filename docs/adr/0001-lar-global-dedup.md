# ADR 0001: Global content-addressed storage for LAR v1

Status: accepted for implementation

Date: 2026-07-20

## Context

Alex currently writes a gzip file for each request, changed upstream request,
response, and tool payload. Agent clients resend the conversation prefix on
every turn, so storing whole bodies makes disk use grow with bytes that Alex has
already captured. Rolling one-file-per-day archives alone would preserve that
duplication across rotations.

The core product requirement is stronger than whole-body compression: a byte
range that is reused by later exchanges should have one canonical stored copy,
while every exchange must retain its own ordered headers, stages, timestamps,
and body references.

## Decision

LAR v1 uses a global content-addressed chunk namespace for the live store.

- Raw application-boundary body bytes are split with deterministic
  content-defined chunking.
- Chunks are identified by an algorithm-tagged BLAKE3 digest and uncompressed
  length. A digest hit is accepted only after equality verification.
- A content-addressed body manifest records the ordered chunk ranges and the
  whole-body digest. Identical complete bodies therefore share one manifest;
  related growing bodies reuse their common chunks.
- Rolling v1 body-pack files hold unique compressed chunks together with the
  exchange, stage, header, timing, and external-manifest references published
  at the same durability boundary. The format reserves a separate event-log
  role, but the live writer will split it only if measurements justify the
  additional recovery and archive-set coordination.
- SQLite maps stable IDs and hashes to replaceable file locations. Physical
  offsets are never durable identities.
- A standalone export copies its transitive chunk closure and may duplicate
  chunks that also exist in the live store because it must remain
  self-contained.
- Retention removes references first. Mark-and-sweep and verified repacking
  reclaim unreferenced chunks; a file is never rewritten in place.

The application-level fidelity boundary is the bytes and logical headers Alex
observes after TLS/HTTP decoding and capture-time redaction. LAR does not claim
to reproduce TLS records, TCP packets, or HPACK/QPACK encoder bytes.

## Consequences

Benefits:

- body storage approaches genuinely new bytes rather than resent prefixes;
- unchanged client and upstream bodies have no second stored payload;
- compression, retention, and archive rotation no longer define the dedupe
  boundary;
- bodies remain byte-exact even if normalized conversation parsing changes;
- active and sealed records use the same logical identifiers.

Costs:

- deletion and retention require reference tracking and verified garbage
  collection;
- the catalog and append coordinator must survive partial commits and orphaned
  records;
- a live archive set is not just one independently movable daily file;
- equality-revealing hashes and search indexes inherit the archive's privacy
  requirements.

## Rejected alternatives

- Per-exchange gzip files: simple, but preserves the measured quadratic growth.
- Whole-body hashes only: removes identical client/upstream copies but not
  repeated prefixes or small mutations.
- Per-day dedupe only: repeated sessions crossing rotation boundaries regain
  duplicate chunks.
- Normalized messages as the source of truth: cannot guarantee byte-exact
  replay across provider formats, malformed streams, or parser upgrades.
- In-place mutable pack files: makes crash recovery, repair, and rollback
  substantially harder to prove.

## Required validation

Before enabling LAR writes by default, the implementation must demonstrate
byte/hash equality on the migration corpus, concurrent duplicate insertion
safety, recovery at record truncation boundaries, reference-safe retention,
and the storage/latency thresholds in `lar-format.md`.
