use alex_lar::Limits;

pub const MAX_FUZZ_INPUT: usize = 1024 * 1024;

pub fn bounded(data: &[u8]) -> &[u8] {
    &data[..data.len().min(MAX_FUZZ_INPUT)]
}

pub fn limits() -> Limits {
    Limits {
        max_header_length: 64 * 1024,
        max_frame_payload: 256 * 1024,
        max_chunk_uncompressed: 128 * 1024,
        max_body_length: 1024 * 1024,
        max_manifest_chunks: 4_096,
        max_header_atoms: 2_048,
        max_field_length: 64 * 1024,
        max_dictionaries: 16,
        max_identifier_length: 4 * 1024,
        max_stream_reads: 4_096,
        max_stream_frames: 4_096,
        max_exchange_stages: 2_048,
        max_stream_indexes: 2_048,
        max_stages: 4_096,
        max_exchanges: 2_048,
        max_session_exchanges: 2_048,
        max_metadata_page_uncompressed: 256 * 1024,
        max_conversation_entry_ranges: 2_048,
        max_generation_entries: 4_096,
        max_turn_response_entries: 2_048,
        max_conversation_entries: 4_096,
        max_generations: 2_048,
        max_turn_views: 2_048,
    }
}
