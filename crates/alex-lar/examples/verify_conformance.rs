//! Verify the published LAR v1 corpus hashes and semantic expectations.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use alex_lar::{ArchiveReader, Limits, OpenPath, RecoveryStatus};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("conformance fixture is missing string field {field}"))
}

fn number(value: &Value, field: &str) -> Result<u64, String> {
    value[field]
        .as_u64()
        .ok_or_else(|| format!("conformance fixture is missing integer field {field}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn assert_number(value: &Value, field: &str, actual: u64) -> Result<(), String> {
    let expected = number(value, field)?;
    if actual != expected {
        return Err(format!(
            "{}: expected {field}={expected}, got {actual}",
            string(value, "path")?
        ));
    }
    Ok(())
}

fn verify_lar(path: &Path, fixture: &Value, bytes: &[u8]) -> Result<(), String> {
    let mut reader = ArchiveReader::open(Cursor::new(bytes), Limits::default())
        .map_err(|error| format!("opening {}: {error}", path.display()))?;
    assert_number(
        fixture,
        "container_minor",
        reader.header().container_minor as u64,
    )?;
    if hex(&reader.header().file_uuid) != string(fixture, "file_uuid")? {
        return Err(format!("{}: file UUID mismatch", path.display()));
    }
    let expected_sealed = fixture["sealed"]
        .as_bool()
        .ok_or("conformance fixture is missing sealed")?;
    if reader.is_sealed() != expected_sealed {
        return Err(format!("{}: sealed state mismatch", path.display()));
    }
    let open_path = match reader.open_path() {
        OpenPath::Footer => "footer",
        OpenPath::Checkpoint => "checkpoint",
        OpenPath::ForwardScan => "forward_scan",
    };
    if open_path != string(fixture, "open_path")? {
        return Err(format!("{}: open path mismatch", path.display()));
    }
    let recovery = match reader.recovery_status() {
        RecoveryStatus::Clean => "clean",
        RecoveryStatus::TruncatedTail { .. } => "truncated_tail",
        RecoveryStatus::CorruptIndexFallback { .. } => "corrupt_index_fallback",
    };
    if recovery != string(fixture, "recovery")? {
        return Err(format!("{}: recovery state mismatch", path.display()));
    }
    for (field, actual) in [
        ("record_count", reader.record_count()),
        ("chunk_count", reader.chunk_count()),
        ("manifest_count", reader.manifest_count()),
        ("header_block_count", reader.header_block_count()),
        ("stream_index_count", reader.stream_index_count()),
        ("stage_count", reader.stage_count()),
        ("exchange_count", reader.exchange_count()),
    ] {
        assert_number(fixture, field, actual as u64)?;
    }
    for (field, actual) in [
        (
            "conversation_entry_count",
            reader.conversation_entry_count(),
        ),
        ("generation_count", reader.generation_count()),
        ("turn_view_count", reader.turn_view_count()),
    ] {
        if !fixture[field].is_null() {
            assert_number(fixture, field, actual as u64)?;
        }
    }
    if let Some(trace_id) = fixture["trace_id"].as_str() {
        let exchange = reader
            .exchange_by_trace(trace_id.as_bytes())
            .ok_or_else(|| format!("{}: trace index miss", path.display()))?;
        if !fixture["session_id"].is_null()
            && exchange.data.session_id.as_deref()
                != fixture["session_id"].as_str().map(str::as_bytes)
        {
            return Err(format!("{}: session index mismatch", path.display()));
        }
        if fixture["has_turn_view"].as_bool() == Some(true)
            && reader.turn_view_by_trace(trace_id.as_bytes()).is_none()
        {
            return Err(format!("{}: turn-view trace index miss", path.display()));
        }
        if fixture["has_exchange_metadata"].as_bool() == Some(true) {
            let metadata = reader.exchange_metadata(&exchange.id).ok_or_else(|| {
                format!("{}: exchange-metadata companion missing", path.display())
            })?;
            if metadata.data.harness.as_deref()
                != fixture["metadata_harness_utf8"].as_str().map(str::as_bytes)
            {
                return Err(format!("{}: metadata harness mismatch", path.display()));
            }
            if metadata.data.streamed != fixture["metadata_streamed"].as_bool() {
                return Err(format!("{}: metadata streamed mismatch", path.display()));
            }
            if metadata.data.status != fixture["metadata_status"].as_i64() {
                return Err(format!("{}: metadata status mismatch", path.display()));
            }
            if metadata.data.cost_usd_bits != fixture["metadata_cost_usd_bits"].as_u64() {
                return Err(format!("{}: metadata cost bits mismatch", path.display()));
            }
            let expected_key = fixture["metadata_unknown_key_utf8"]
                .as_str()
                .map(str::as_bytes);
            let expected_value = fixture["metadata_unknown_value_utf8"]
                .as_str()
                .map(str::as_bytes);
            if !metadata.data.unknown_attributes.iter().any(|attribute| {
                Some(attribute.key.as_slice()) == expected_key
                    && Some(attribute.value.as_slice()) == expected_value
            }) {
                return Err(format!(
                    "{}: metadata unknown optional attribute mismatch",
                    path.display()
                ));
            }
        }
    }
    if let Some(expected) = fixture["body_utf8"].as_str() {
        let manifest = *reader
            .manifest_ids()
            .next()
            .ok_or_else(|| format!("{}: expected a body manifest", path.display()))?;
        let body = reader
            .read_body(&manifest)
            .map_err(|error| format!("{}: reading body: {error}", path.display()))?;
        if body != expected.as_bytes() {
            return Err(format!("{}: reconstructed body mismatch", path.display()));
        }
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let manifest_path = root.join("conformance-v1.json");
    let manifest: Value = serde_json::from_slice(
        &fs::read(&manifest_path)
            .map_err(|error| format!("reading {}: {error}", manifest_path.display()))?,
    )
    .map_err(|error| format!("parsing {}: {error}", manifest_path.display()))?;
    if manifest["schema"] != "alex-lar-conformance-corpus-v1" || manifest["container_major"] != 1 {
        return Err("unsupported conformance manifest schema/version".into());
    }
    let fixtures = manifest["fixtures"]
        .as_array()
        .ok_or("conformance manifest has no fixtures")?;
    for fixture in fixtures {
        let relative = string(fixture, "path")?;
        let path = root.join(relative);
        let bytes =
            fs::read(&path).map_err(|error| format!("reading {}: {error}", path.display()))?;
        assert_number(fixture, "length", bytes.len() as u64)?;
        let digest = hex(&Sha256::digest(&bytes));
        if digest != string(fixture, "sha256")? {
            return Err(format!("{}: SHA-256 mismatch", path.display()));
        }
        match string(fixture, "kind")? {
            "lar" => verify_lar(&path, fixture, &bytes)?,
            "hex-vector" => {
                let decoded = bytes
                    .iter()
                    .copied()
                    .filter(|byte| !byte.is_ascii_whitespace())
                    .collect::<Vec<_>>();
                if decoded.len() % 2 != 0 || !decoded.iter().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(format!("{}: invalid hexadecimal vector", path.display()));
                }
            }
            other => return Err(format!("{relative}: unknown fixture kind {other}")),
        }
        println!(
            "verified {relative} ({} bytes, sha256:{digest})",
            bytes.len()
        );
    }
    println!("verified {} LAR conformance fixtures", fixtures.len());
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("LAR conformance: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn published_corpus_verifies() {
        super::run().unwrap();
    }
}
