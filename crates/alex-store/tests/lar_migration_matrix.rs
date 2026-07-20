use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use alex_core::TraceRecord;
use alex_store::{
    LarArtifactLocation, LarArtifactReadRequest, LarBodyArtifact, LarBodyStoreConfig,
    LarBodyStoreMode, LarLegacyImportBoundary, LarLegacyImportHook, LarLegacyImportOptions, Store,
};
use anyhow::bail;
use flate2::write::GzEncoder;
use flate2::Compression;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tmpdir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "alex-lar-migration-matrix-{name}-{}-{}",
        std::process::id(),
        TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_gzip(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = GzEncoder::new(file, Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap().sync_all().unwrap();
}

fn insert_legacy_trace(
    store: &Store,
    data_dir: &Path,
    id: &str,
    session: &str,
    timestamp: i64,
    body: &[u8],
) -> PathBuf {
    let path = data_dir
        .join("bodies/2026-07-20")
        .join(format!("{id}.request.json.gz"));
    write_gzip(&path, body);
    store
        .insert_trace(&TraceRecord {
            id: id.into(),
            ts_request_ms: timestamp,
            session_id: Some(session.into()),
            req_body_path: Some(path.to_string_lossy().into_owned()),
            ..TraceRecord::default()
        })
        .unwrap();
    path
}

fn assert_session_page_hashes(store: &Store, session: &str, expected: &HashMap<String, [u8; 32]>) {
    let mut after = None;
    let mut seen = Vec::new();
    loop {
        let page = store
            .session_traces_page(session, after.clone(), None, 3, false)
            .unwrap();
        assert_eq!(page.total_count, expected.len());
        if page.rows.is_empty() {
            break;
        }
        let requests = page
            .rows
            .iter()
            .map(|row| {
                LarArtifactReadRequest::new("trace", row["id"].as_str().unwrap(), "client_request")
            })
            .collect::<Vec<_>>();
        let bodies = store.read_lar_or_legacy_artifact_batch(&requests).unwrap();
        for (row, body) in page.rows.iter().zip(bodies) {
            let id = row["id"].as_str().unwrap();
            let body = body.unwrap_or_else(|| panic!("trace {id} has no readable request body"));
            assert_eq!(
                *blake3::hash(&body).as_bytes(),
                expected[id],
                "mixed/corrupt body returned for {id}"
            );
            seen.push(id.to_string());
        }
        let last = page.rows.last().unwrap();
        after = Some((
            last["ts_request_ms"].as_i64().unwrap(),
            last["id"].as_str().unwrap().to_string(),
        ));
        if !page.has_more_after {
            break;
        }
    }
    seen.sort();
    let mut wanted = expected.keys().cloned().collect::<Vec<_>>();
    wanted.sort();
    assert_eq!(seen, wanted);
}

#[test]
fn live_capture_and_paged_reads_remain_available_during_background_migration() {
    let data_dir = tmpdir("concurrent-browser");
    let store = Arc::new(
        Store::open_with_lar_body_store(
            data_dir.clone(),
            LarBodyStoreConfig {
                mode: LarBodyStoreMode::LarWithFallback,
                ..LarBodyStoreConfig::default()
            },
        )
        .unwrap(),
    );
    let session = "concurrent-session";
    let mut expected = HashMap::new();
    for index in 0..12 {
        let id = format!("legacy-{index:02}");
        let body = format!("legacy body {index}:{}", "x".repeat(index * 37)).into_bytes();
        insert_legacy_trace(&store, &data_dir, &id, session, 1_000 + index as i64, &body);
        expected.insert(id, *blake3::hash(&body).as_bytes());
    }

    let (appended_tx, appended_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let resume_rx = Arc::new(Mutex::new(resume_rx));
    let paused = Arc::new(AtomicBool::new(false));
    let hook = LarLegacyImportHook::new({
        let resume_rx = resume_rx.clone();
        let paused = paused.clone();
        move |boundary| {
            if boundary == LarLegacyImportBoundary::BodyAppended
                && !paused.swap(true, Ordering::SeqCst)
            {
                appended_tx.send(()).unwrap();
                resume_rx.lock().unwrap().recv().unwrap();
            }
            Ok(())
        }
    });
    let worker_store = store.clone();
    let worker = std::thread::spawn(move || {
        worker_store.run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 12,
            lease_owner: "concurrent-background-worker".into(),
            boundary_hook: Some(hook),
            ..LarLegacyImportOptions::default()
        })
    });
    appended_rx.recv().unwrap();

    // The old batch is durably appended but no pointer has switched yet. A
    // normal live capture publishes into the live writer while browser pages
    // continue resolving every old row through its legacy fallback.
    let live_id = "live-during-migration";
    let live_body = b"captured while the migrator is paused";
    let live = store
        .write_body_artifact(
            &LarBodyArtifact::trace(live_id, "client_request"),
            "request.json",
            live_body,
        )
        .unwrap();
    assert!(live.lar_error.is_none());
    store
        .insert_trace(&TraceRecord {
            id: live_id.into(),
            ts_request_ms: 2_000,
            session_id: Some(session.into()),
            req_body_path: Some(live.legacy_path),
            ..TraceRecord::default()
        })
        .unwrap();
    expected.insert(live_id.into(), *blake3::hash(live_body).as_bytes());
    assert!(matches!(
        store
            .lar_artifact_location("trace", "legacy-00", "client_request", None)
            .unwrap(),
        Some(LarArtifactLocation::Legacy { .. })
    ));
    assert!(matches!(
        store
            .lar_artifact_location("trace", live_id, "client_request", None)
            .unwrap(),
        Some(LarArtifactLocation::Lar { .. })
    ));
    assert_session_page_hashes(&store, session, &expected);

    resume_tx.send(()).unwrap();
    let report = worker.join().unwrap().unwrap();
    assert_eq!((report.migrated, report.failed), (12, 0));
    assert_eq!(report.job_state, "complete");
    assert_session_page_hashes(&store, session, &expected);
    for index in 0..12 {
        assert!(matches!(
            store
                .lar_artifact_location(
                    "trace",
                    &format!("legacy-{index:02}"),
                    "client_request",
                    None,
                )
                .unwrap(),
            Some(LarArtifactLocation::Lar { .. })
        ));
    }
}

#[test]
fn restart_at_every_committed_import_boundary_converges_exactly_once() {
    let boundaries = [
        LarLegacyImportBoundary::JobClaimed,
        LarLegacyImportBoundary::BodyAppended,
        LarLegacyImportBoundary::BodyValidated,
        LarLegacyImportBoundary::PointerSwitched,
        LarLegacyImportBoundary::JobCompleted,
    ];
    for boundary in boundaries {
        let data_dir = tmpdir(&format!("restart-{boundary:?}"));
        let body = format!("restart boundary {boundary:?}:{}", "z".repeat(4096)).into_bytes();
        let store = Store::open(data_dir.clone()).unwrap();
        let source = insert_legacy_trace(
            &store,
            &data_dir,
            "restart-trace",
            "restart-session",
            1_000,
            &body,
        );
        let fired = Arc::new(AtomicBool::new(false));
        let hook = LarLegacyImportHook::new({
            let fired = fired.clone();
            move |observed| {
                if observed == boundary && !fired.swap(true, Ordering::SeqCst) {
                    bail!("simulated process stop after {boundary:?}");
                }
                Ok(())
            }
        });
        let owner = format!("restart-owner-{boundary:?}");
        let error = store
            .run_lar_legacy_import(&LarLegacyImportOptions {
                batch_size: 1,
                lease_owner: owner.clone(),
                boundary_hook: Some(hook),
                ..LarLegacyImportOptions::default()
            })
            .unwrap_err();
        assert!(format!("{error:#}").contains("simulated process stop"));
        assert!(fired.load(Ordering::SeqCst));
        assert!(source.is_file(), "restart removed the legacy source");
        let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
        let (job_state, lease_owner): (String, Option<String>) = catalog
            .query_row(
                "SELECT state, lease_owner FROM lar_migration_jobs
                 ORDER BY created_at_ms DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let manifests: i64 = catalog
            .query_row("SELECT COUNT(*) FROM lar_manifests", [], |row| row.get(0))
            .unwrap();
        let pointers: i64 = catalog
            .query_row("SELECT COUNT(*) FROM lar_trace_artifacts", [], |row| {
                row.get(0)
            })
            .unwrap();
        let migrated_items: i64 = catalog
            .query_row(
                "SELECT COUNT(*) FROM lar_migration_items WHERE state='migrated'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        match boundary {
            LarLegacyImportBoundary::JobClaimed => {
                assert_eq!(
                    (job_state.as_str(), manifests, pointers, migrated_items),
                    ("running", 0, 0, 0)
                );
                assert_eq!(lease_owner.as_deref(), Some(owner.as_str()));
            }
            LarLegacyImportBoundary::BodyAppended => {
                assert_eq!(
                    (job_state.as_str(), manifests, pointers, migrated_items),
                    ("running", 0, 0, 0)
                );
                assert_eq!(lease_owner.as_deref(), Some(owner.as_str()));
            }
            LarLegacyImportBoundary::BodyValidated => {
                assert_eq!(
                    (job_state.as_str(), manifests, pointers, migrated_items),
                    ("running", 1, 0, 0)
                );
                assert_eq!(lease_owner.as_deref(), Some(owner.as_str()));
            }
            LarLegacyImportBoundary::PointerSwitched => {
                assert_eq!(
                    (job_state.as_str(), manifests, pointers, migrated_items),
                    ("running", 1, 1, 1)
                );
                assert_eq!(lease_owner.as_deref(), Some(owner.as_str()));
            }
            LarLegacyImportBoundary::JobCompleted => {
                assert_eq!(
                    (job_state.as_str(), manifests, pointers, migrated_items),
                    ("complete", 1, 1, 1)
                );
                assert_eq!(lease_owner, None);
            }
        }
        drop(catalog);
        assert_eq!(
            store
                .read_lar_or_legacy_artifact("trace", "restart-trace", "client_request", None,)
                .unwrap()
                .unwrap(),
            body
        );
        drop(store);

        let reopened = Store::open(data_dir.clone()).unwrap();
        let resumed = reopened
            .run_lar_legacy_import(&LarLegacyImportOptions {
                batch_size: 1,
                lease_owner: owner,
                ..LarLegacyImportOptions::default()
            })
            .unwrap();
        assert_eq!(resumed.job_state, "complete", "boundary {boundary:?}");
        assert_eq!(
            reopened
                .read_lar_or_legacy_artifact("trace", "restart-trace", "client_request", None,)
                .unwrap()
                .unwrap(),
            body,
            "boundary {boundary:?}"
        );
        assert!(matches!(
            reopened
                .lar_artifact_location("trace", "restart-trace", "client_request", None,)
                .unwrap(),
            Some(LarArtifactLocation::Lar { .. })
        ));

        let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
        let state: (String, Option<String>, i64, i64, i64) = catalog
            .query_row(
                "SELECT state, lease_owner, pending_count, migrated_count, failed_count
                 FROM lar_migration_jobs ORDER BY created_at_ms DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(state, ("complete".into(), None, 0, 1, 0));
        let pointers: i64 = catalog
            .query_row(
                "SELECT COUNT(*) FROM lar_trace_artifacts
                 WHERE owner_kind='trace' AND owner_id='restart-trace'
                   AND artifact_kind='client_request' AND validation_state='validated'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let manifests: i64 = catalog
            .query_row("SELECT COUNT(*) FROM lar_manifests", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pointers, 1);
        assert_eq!(manifests, 1, "boundary {boundary:?} duplicated the body");
    }
}

#[test]
fn failed_validation_reads_legacy_then_repaired_source_retries_successfully() {
    let data_dir = tmpdir("validation-fallback-retry");
    let store = Store::open(data_dir.clone()).unwrap();
    let source = insert_legacy_trace(
        &store,
        &data_dir,
        "fallback-trace",
        "fallback-session",
        1_000,
        b"legacy bytes remain authoritative",
    );
    let changed = Arc::new(AtomicBool::new(false));
    let hook = LarLegacyImportHook::new({
        let data_dir = data_dir.clone();
        let changed = changed.clone();
        move |boundary| {
            if boundary == LarLegacyImportBoundary::BodyValidated
                && !changed.swap(true, Ordering::SeqCst)
            {
                let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3"))?;
                catalog.execute(
                    "UPDATE lar_manifests SET state='quarantined' WHERE state='ready'",
                    [],
                )?;
            }
            Ok(())
        }
    });
    let failed = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 1,
            lease_owner: "fallback-worker".into(),
            boundary_hook: Some(hook),
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!((failed.migrated, failed.failed), (0, 1));
    assert_eq!(failed.job_state, "failed");
    assert!(matches!(
        store
            .lar_artifact_location("trace", "fallback-trace", "client_request", None)
            .unwrap(),
        Some(LarArtifactLocation::Legacy {
            migration_error: Some(error),
            ..
        }) if error.kind == "validation"
    ));
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "fallback-trace", "client_request", None,)
            .unwrap()
            .unwrap(),
        b"legacy bytes remain authoritative"
    );
    assert!(source.is_file());

    // Repairing/replacing the legacy source gives it a new durable fingerprint;
    // the old failure remains as provenance and the replacement can complete.
    let repaired = b"repaired legacy bytes imported on retry";
    write_gzip(&source, repaired);
    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    catalog
        .execute(
            "UPDATE lar_manifests SET state='ready' WHERE state='quarantined'",
            [],
        )
        .unwrap();
    drop(catalog);
    let retried = store
        .run_lar_legacy_import(&LarLegacyImportOptions {
            batch_size: 1,
            lease_owner: "fallback-worker".into(),
            ..LarLegacyImportOptions::default()
        })
        .unwrap();
    assert_eq!((retried.migrated, retried.failed), (1, 0));
    assert_eq!(retried.job_state, "complete");
    assert_eq!(
        store
            .read_lar_or_legacy_artifact("trace", "fallback-trace", "client_request", None,)
            .unwrap()
            .unwrap(),
        repaired
    );
    assert!(matches!(
        store
            .lar_artifact_location("trace", "fallback-trace", "client_request", None)
            .unwrap(),
        Some(LarArtifactLocation::Lar { .. })
    ));
    let catalog = rusqlite::Connection::open(data_dir.join("alexandria.sqlite3")).unwrap();
    let states = catalog
        .prepare(
            "SELECT state, validation_state FROM lar_migration_items
             WHERE owner_id='fallback-trace' ORDER BY created_at_ms, item_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        states,
        vec![
            ("skipped".into(), "failed".into()),
            ("migrated".into(), "validated".into())
        ]
    );
}
