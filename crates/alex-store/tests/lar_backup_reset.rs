use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use alex_core::TraceRecord;
use alex_lar::{ArchiveWriter, ChunkerConfig, FileHeader, Limits};
use alex_store::{LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, Store};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "alex-lar-backup-reset-{name}-{}-{}",
        std::process::id(),
        TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn lar_config() -> LarBodyStoreConfig {
    LarBodyStoreConfig {
        mode: LarBodyStoreMode::LarWithFallback,
        ..LarBodyStoreConfig::default()
    }
}

#[test]
fn reset_reclaims_owned_lar_storage_and_restarts_the_writer() {
    let data_dir = tmpdir("owned-storage");
    let store = Store::open_with_lar_body_store(data_dir.clone(), lar_config()).unwrap();
    for id in ["shared-a", "shared-b"] {
        store
            .insert_trace(&TraceRecord {
                id: id.into(),
                ts_request_ms: 1_000,
                ..Default::default()
            })
            .unwrap();
        store
            .write_body_artifact(
                &LarBodyArtifact::trace(id, "client_request"),
                "request.json",
                b"the same shared body",
            )
            .unwrap();
    }
    let before = store.plan_lar_gc().unwrap();
    assert_eq!(before.reachable_manifests, 1);
    assert!(data_dir.join("lar").is_dir());

    store.clear_traces_and_bodies().unwrap();
    assert!(!data_dir.join("lar").exists());
    assert!(!data_dir.join("bodies").exists());
    assert_eq!(store.reset_counts().unwrap().traces, 0);
    let after = store.plan_lar_gc().unwrap();
    assert_eq!(after.reachable_manifests, 0);
    assert_eq!(after.unreachable_manifests, 0);

    store
        .insert_trace(&TraceRecord {
            id: "after-reset".into(),
            ts_request_ms: 2_000,
            ..Default::default()
        })
        .unwrap();
    let written = store
        .write_body_artifact(
            &LarBodyArtifact::trace("after-reset", "client_request"),
            "request.json",
            b"new body",
        )
        .unwrap();
    assert!(written.manifest_id.is_some(), "LAR writer did not restart");
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "after-reset", "client_request", None)
            .unwrap()
            .as_deref(),
        Some(b"new body".as_slice())
    );
}

#[test]
fn reset_detaches_but_never_deletes_an_external_standalone_archive() {
    let data_dir = tmpdir("external-catalog");
    let external_dir = tmpdir("external-source");
    let archive_path = external_dir.join("attached.lar");
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&archive_path)
        .unwrap();
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone([9; 16], 1_000_000, b"reset-test".to_vec()),
        ChunkerConfig::default(),
        Limits::default(),
    )
    .unwrap();
    writer.append_body(b"external retained bytes").unwrap();
    writer.seal().unwrap();
    writer.into_inner().unwrap().sync_all().unwrap();

    let store = Store::open(data_dir).unwrap();
    store
        .import_sealed_lar_archive(&archive_path, &Default::default())
        .unwrap();
    store.clear_traces_and_bodies().unwrap();
    assert!(archive_path.is_file());
    let gc = store.plan_lar_gc().unwrap();
    assert_eq!(gc.reachable_manifests, 0);
    assert_eq!(gc.unreachable_manifests, 0);
}
