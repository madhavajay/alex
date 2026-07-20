# LAR golden fixtures

`v1.0-full.lar` is the frozen released-v1 container fixture. It includes a
chunk, mixed metadata page, stream timing, stage/exchange indexes, checkpoint,
and sealed footer. `v1.future-minor-optional.lar` is a synthetic compatibility
fixture proving that a v1 reader accepts a newer minor and skips unknown
optional records/schemas.

`v1.conversation-dag.lar` is the required-feature fixture for normalized raw
range entries, a generation, and a trace-addressed turn view. It complements
the frozen pre-DAG `v1.0-full.lar` instead of rewriting that compatibility
sentinel.

`conformance-v1.json` publishes the exact byte lengths, SHA-256 transport
digests, and expected semantic reader results. Verify the complete corpus with:

```sh
cargo run -p alex-lar --example verify_conformance
```

Verify deterministic output without modifying anything:

```sh
cargo run -p alex-lar --example generate_golden
```

After reviewing an intentional wire-format change, explicitly regenerate the
fixtures and bounded fuzz seeds with:

```sh
cargo run -p alex-lar --example generate_golden -- --write
```

The generator never overwrites a fixture unless `--write` is present.
