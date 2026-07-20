# ADR 0002: Native LAR framing instead of an MCAP profile

Status: accepted for v1

Date: 2026-07-20

## Context

LAR needs an appendable content-addressed body store, lookup by trace and
content identity, exact stage/header order, crash checkpoints, and standalone
archives. MCAP was the strongest existing-container candidate because it has
well-specified records, timestamped channels, compression, indexes, checksums,
summary data, and mature readers.

The decision cannot be based on feature-list similarity. The central invariant
is that one compressed copy of each unique raw chunk serves every request,
response, tool result, replay view, and normalized entry. We therefore built
the same 77-turn, 18,938,807-logical-byte workload in both representations:

- native LAR chunk records plus body manifests and a sealed content-ID index;
- a valid MCAP profile where each unique zstd chunk is one indexed attachment
  and each binary body manifest is a message on `alex.body.manifest`.

Both variants hash-check and reconstruct the selected body. The MCAP variant
uses first-use order—new chunk attachments followed by the referencing
manifest—so truncation recovery is not biased by placing all bodies first.
The executable is `cargo run -p alex-lar --release --example container_gate`.

## Measurement

One release-mode Linux run produced:

| Property | Native LAR | MCAP profile |
| --- | ---: | ---: |
| File bytes | 379,886 | 723,145 |
| Median in-memory indexed final-body reconstruction | 2.13 ms | 1.66 ms |
| Complete manifests/messages before an 80% byte cut | 61/77 | 69/77 |
| Unique chunks stored | 284 | 284 |

The MCAP profile was about 22% faster in this in-memory microbenchmark and
recovered more turn manifests at the chosen proportional cut. Native LAR was
47% smaller and separately meets the filesystem open plus 1 KiB body target at
7.28 ms warm p99. These numbers are directional; the checked-in benchmark and
tests are the reproducible evidence, not the single run.

## Decision

LAR v1 uses native LAR framing.

MCAP's standard indexes are organized around time ranges and channel IDs. A
LAR reader instead needs direct stable-ID indexes for chunks, manifests,
headers, stages, exchanges, conversation generations, and traces. Mapping the
one-copy invariant into standard MCAP required Alex-specific attachment names,
a custom manifest encoding, content-hash verification rules, reverse
references, and an additional hash-to-attachment lookup. Generic MCAP tooling
can list those records but cannot reconstruct or safely garbage-collect an LLM
conversation without implementing the LAR profile.

MCAP's compressed message chunks are also the wrong random-access unit for
large content-addressed bodies: its indexed seek locates a timestamped message
inside a compressed chunk, whereas LAR addresses and decompresses an
independent unique body chunk directly. Attachments avoid that issue but are
not compressed by the MCAP container, forcing the profile to define another
compression envelope. MCAP private records would make the mapping cleaner but
would amount to placing the native LAR format inside an MCAP envelope.

Native framing keeps active append checkpoints, stable content indexes, strict
bounded parsing, and archive-set external body references first-class while
using substantially fewer bytes in the measured workload. The implementation
still borrows MCAP's good ideas: skippable length-prefixed records, independent
compression, CRCs, a fixed footer, summary indexes, and forward recovery.

The relevant upstream behavior is documented in the
[MCAP format specification](https://mcap.dev/spec) and its
[implementation notes](https://mcap.dev/spec/notes). The MCAP dependency is a
development-only benchmark dependency and is not part of Alex's runtime reader
or writer.

## Consequences

- `.lar` needs its own maintained public specification and conformance corpus.
- Generic MCAP tools do not open `.lar`; LAR can still offer an MCAP export if
  timestamp/channel interoperability becomes valuable.
- The native container is free to index content and trace IDs directly without
  pretending they are timestamps or channels.
- The decision is versioned, not permanent. A future envelope change requires
  a new major version and measured migration path; MCAP remains a comparison
  target in the design-gate executable.
