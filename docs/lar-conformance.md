# LAR v1 conformance corpus

The public v1 vectors live in `crates/alex-lar/testdata/`:

- `v1-envelope.hex` freezes the base header and record-envelope encoding;
- `v1.0-full.lar` exercises a chunk, mixed metadata page, ordered headers,
  stream timing, stage/exchange references, indexes, checkpoint, and footer;
- `v1.future-minor-optional.lar` proves a v1 reader accepts a newer minor and
  skips unknown optional records and schemas;
- `v1.conversation-dag.lar` covers required feature bit `0x2`, normalized raw
  ranges, generation and turn-view indexes, and exact trace lookup;
- `v1.exchange-metadata.lar` covers optional Type-15 ExchangeMetadata with all
  current fields and an unknown optional attribute;
- `conformance-v1.json` publishes byte lengths, SHA-256 transport digests, and
  expected semantic reader results.

Verify checked-in bytes and reader behavior:

```sh
cargo run -p alex-lar --example verify_conformance
cargo run -p alex-lar --example generate_golden
cargo test -p alex-lar --test conformance --test golden_compatibility
```

`generate_golden` is verification-only unless `--write` is explicitly passed.
Regeneration is a wire-format change: review the spec and compatibility impact,
update the conformance manifest hashes/expectations, and retain old fixtures
whenever the new reader claims backward compatibility.

Third-party readers should begin with the manifest's byte/hash checks, apply
the bounds and compatibility rules from `lar-format-v1.md`, then compare all
published counts, anchors, and body bytes. Passing the small corpus is necessary
but not sufficient: implementations should also run malformed-length,
checksum, unknown-feature, and every-prefix-truncation tests.

## Proposed media type

Until a standards registration exists, Alex uses the vendor-tree proposal:

```text
application/vnd.alexandria.lar
```

The representation is binary, has no `charset` parameter, uses `.lar`, and is
identified independently by the leading `LAR1` magic. Compression is internal
and per-record/page; serving a file with `Content-Encoding: zstd` would describe
an additional outer encoding and is discouraged because it defeats range
access. Suggested content disposition is `attachment; filename="capture.lar"`.

The JSONL interchange representation is distinct and may be labeled
`application/vnd.alexandria.lar+jsonl`. Legacy-only v1 is import-compatible but
lossy. Canonical v2 preserves the exact timeline graph and splits body data into
bounded records, but the current importer intentionally rejects v2; it is not a
substitute for a self-contained, currently re-importable standalone LAR archive.
Neither proposal is an IANA registration. Readers must still validate magic,
required features, lengths, checksums, hashes, and reference closure rather
than trusting a filename or media type.

LAR files can contain sensitive prompts, responses, tool data, ordered headers,
and equality-revealing content hashes. Operators should serve them only with
the same authentication, filesystem permissions, retention controls, and
redaction policy as the source trace store.
