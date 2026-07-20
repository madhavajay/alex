# `alex-lar` fuzzing

The four targets split malformed-input coverage by trust boundary:

- `record_framing`: header/frame lengths, flags, checksums, and truncation;
- `zstd_decompression`: chunk/page decompression plus body reads;
- `metadata_indexes`: metadata pages, checkpoints, indexes, and footers;
- `manifest_recovery`: manifests, reconstruction, and forward recovery at
  input-selected truncation points.

Every harness caps input at 1 MiB and applies much smaller record/count limits
than production defaults. Checked-in corpus seeds are deterministic golden LAR
archives; regenerate them only with the explicit command in
`../testdata/README.md`.

Run a bounded smoke pass (requires `cargo-fuzz` and nightly Rust):

```sh
cd crates/alex-lar
cargo fuzz run record_framing -- -runs=1000 -max_len=1048576
cargo fuzz run zstd_decompression -- -runs=1000 -max_len=1048576
cargo fuzz run metadata_indexes -- -runs=1000 -max_len=1048576
cargo fuzz run manifest_recovery -- -runs=1000 -max_len=1048576
```
