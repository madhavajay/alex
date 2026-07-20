#![no_main]

mod common;

use alex_lar::ArchiveReader;
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let data = common::bounded(data);
    let limits = common::limits();
    if let Ok(mut reader) = ArchiveReader::open(Cursor::new(data), limits) {
        let chunks = reader.chunk_records().take(64).collect::<Vec<_>>();
        for chunk in chunks {
            let _ = reader.read_chunk(&chunk.hash);
        }
        let manifests = reader.manifest_ids().copied().take(32).collect::<Vec<_>>();
        for manifest in manifests {
            let _ = reader.read_body(&manifest);
        }
    }
});
