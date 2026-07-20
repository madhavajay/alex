#![no_main]

mod common;

use alex_lar::ArchiveReader;
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fn open_and_reconstruct(data: &[u8]) {
    if let Ok(mut reader) = ArchiveReader::open(Cursor::new(data), common::limits()) {
        let manifests = reader.manifest_ids().copied().take(64).collect::<Vec<_>>();
        for manifest in manifests {
            let mut output = Vec::new();
            let _ = reader.write_body(&manifest, &mut output);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let data = common::bounded(data);
    open_and_reconstruct(data);
    if !data.is_empty() {
        let selector = data.iter().take(8).fold(0usize, |value, byte| {
            value.wrapping_mul(257) ^ *byte as usize
        });
        open_and_reconstruct(&data[..selector % data.len()]);
        open_and_reconstruct(&data[..data.len() - 1]);
    }
});
