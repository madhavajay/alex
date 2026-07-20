#![no_main]

mod common;

use alex_lar::ArchiveReader;
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let data = common::bounded(data);
    if let Ok(reader) = ArchiveReader::open(Cursor::new(data), common::limits()) {
        let _ = reader.open_path();
        let _ = reader.recovery_status();
        let _ = reader.is_sealed();
        let _ = reader.record_count();
        let _ = reader.manifest_ids().take(4_096).count();
        let _ = reader.header_block_ids().take(4_096).count();
        let _ = reader.stream_index_ids().take(2_048).count();
        let _ = reader.stage_ids().take(4_096).count();
        let _ = reader.exchange_ids().take(2_048).count();
        let _ = reader.conversation_entry_ids().take(4_096).count();
        let _ = reader.generation_ids().take(2_048).count();
        let _ = reader.turn_view_ids().take(2_048).count();
    }
});
