use alex_lar::{ArchiveReader, Limits, OpenPath, RecoveryStatus};
use std::io::Cursor;

#[path = "../testsupport/golden.rs"]
mod golden;

#[test]
fn released_v1_0_golden_is_deterministic_and_fully_readable() {
    let frozen = include_bytes!("../testdata/v1.0-full.lar");
    assert_eq!(golden::v1_0_full_archive(), frozen);

    let mut reader = ArchiveReader::open(Cursor::new(frozen), Limits::default()).unwrap();
    assert_eq!(reader.header().container_major, 1);
    assert_eq!(reader.header().container_minor, 0);
    assert_eq!(reader.open_path(), OpenPath::Footer);
    assert_eq!(reader.recovery_status(), RecoveryStatus::Clean);
    assert!(reader.is_sealed());
    assert_eq!(reader.manifest_count(), 1);
    assert_eq!(reader.header_block_count(), 1);
    assert_eq!(reader.stream_index_count(), 1);
    assert_eq!(reader.stage_count(), 1);
    assert_eq!(reader.exchange_count(), 1);
    let exchange = reader.exchange_by_trace(b"golden-trace-v1.0").unwrap();
    assert_eq!(
        exchange.data.session_id.as_deref(),
        Some(b"golden-session-v1.0".as_slice())
    );
    let manifest = *reader.manifest_ids().next().unwrap();
    assert_eq!(reader.read_body(&manifest).unwrap(), golden::FULL_BODY);
}

#[test]
fn newer_minor_and_unknown_optional_records_remain_compatible() {
    let frozen = include_bytes!("../testdata/v1.future-minor-optional.lar");
    assert_eq!(golden::v1_future_minor_optional_archive(), frozen);

    let reader = ArchiveReader::open(Cursor::new(frozen), Limits::default()).unwrap();
    assert_eq!(reader.header().container_major, 1);
    assert_eq!(reader.header().container_minor, 23);
    assert_eq!(reader.header().optional_feature_bits, 0x8000_0000_0000_0042);
    assert_eq!(reader.open_path(), OpenPath::ForwardScan);
    assert_eq!(reader.recovery_status(), RecoveryStatus::Clean);
    assert_eq!(
        reader.record_count(),
        1,
        "unknown optional type is retained as an opaque scanned record"
    );
    assert_eq!(
        reader.header_block_count(),
        0,
        "future optional schema is skipped"
    );
}

#[test]
fn required_conversation_dag_golden_is_deterministic_and_indexed() {
    let frozen = include_bytes!("../testdata/v1.conversation-dag.lar");
    assert_eq!(golden::v1_conversation_dag_archive(), frozen);

    let reader = ArchiveReader::open(Cursor::new(frozen), Limits::default()).unwrap();
    assert_eq!(reader.open_path(), OpenPath::Footer);
    assert_eq!(reader.recovery_status(), RecoveryStatus::Clean);
    assert!(reader.is_sealed());
    assert_eq!(reader.manifest_count(), 2);
    assert_eq!(reader.conversation_entry_count(), 2);
    assert_eq!(reader.generation_count(), 1);
    assert_eq!(reader.turn_view_count(), 1);
    assert!(reader.turn_view_by_trace(b"golden-dag-trace-v1").is_some());
}

#[test]
fn optional_exchange_metadata_golden_is_deterministic_and_indexed() {
    let frozen = include_bytes!("../testdata/v1.exchange-metadata.lar");
    assert_eq!(golden::v1_exchange_metadata_archive(), frozen);

    let reader = ArchiveReader::open(Cursor::new(frozen), Limits::default()).unwrap();
    assert_eq!(reader.open_path(), OpenPath::Footer);
    assert_eq!(reader.recovery_status(), RecoveryStatus::Clean);
    assert!(reader.is_sealed());
    assert_eq!(reader.exchange_count(), 1);
    let exchange = reader
        .exchange_by_trace(b"golden-exchange-metadata-v1")
        .unwrap();
    assert_eq!(
        exchange.data.session_id.as_deref(),
        Some(b"golden-exchange-metadata-session-v1".as_slice())
    );
    assert_eq!(
        reader.exchange_metadata(&exchange.id).unwrap().data,
        golden::v1_exchange_metadata_data()
    );
}
