//! Self-describing profile for append-only harness tool timeline supplements.
//!
//! The archive records are authoritative. SQLite tables are projections that
//! can be rebuilt by parsing the versioned provenance carried by the one tool
//! stage in each immutable child exchange.

use std::io::{Read, Seek};

use alex_lar::{ArchiveReader, Exchange, Stage, StageKind};
use anyhow::{bail, Context, Result};

pub(crate) const TOOL_SUPPLEMENT_SCHEMA: &str = "alex.tool-supplement.v1";

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct ToolSupplementProvenance {
    pub schema: String,
    pub phase: String,
    pub tool_id: String,
    pub harness: String,
    pub turn_id: Option<String>,
    pub tool_call_id: String,
    pub tool_name: String,
    pub source_trace_id: Option<String>,
    pub ts_start_ms: i64,
    pub ts_end_ms: Option<i64>,
    pub is_error: Option<bool>,
    pub exit_status: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedToolSupplement {
    pub provenance: ToolSupplementProvenance,
    pub supplement_trace_id: String,
    pub session_id: String,
    pub parent_trace_id: Option<String>,
    pub stage: Stage,
}

pub(crate) fn supplement_trace_id(
    harness: &str,
    session_id: &str,
    tool_call_id: &str,
    phase: &str,
) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(b"alex-lar-tool-supplement-v1\0");
    for value in [
        harness.as_bytes(),
        session_id.as_bytes(),
        tool_call_id.as_bytes(),
        phase.as_bytes(),
    ] {
        hash.update(&(value.len() as u64).to_le_bytes());
        hash.update(value);
    }
    format!("lar-tool-{}-{phase}", &hash.finalize().to_hex()[..32])
}

/// Parse a canonical tool supplement. Ordinary exchanges return `None`.
/// Once the versioned schema marker is present, every invariant is validated
/// strictly so a look-alike trace ID or partially forged record is rejected.
pub(crate) fn parse_tool_supplement(
    exchange: &Exchange,
    stages: &[Stage],
) -> Result<Option<ParsedToolSupplement>> {
    let trace_id = std::str::from_utf8(&exchange.data.trace_id).ok();
    // Reserve the whole namespace. Unknown phases and malformed IDs must fail
    // closed instead of quietly becoming ordinary parent/child lineage.
    let reserved_profile = trace_id.is_some_and(|trace_id| trace_id.starts_with("lar-tool-"));
    let declared = stages.iter().any(|stage| {
        stage
            .data
            .routing_reason
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
            .and_then(|value| {
                value
                    .get("schema")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .as_deref()
            == Some(TOOL_SUPPLEMENT_SCHEMA)
    });
    if !declared && !reserved_profile {
        return Ok(None);
    }
    if stages.len() != 1 {
        bail!("tool supplement exchange must contain exactly one stage");
    }
    let stage = stages[0].clone();
    let routing_reason = stage
        .data
        .routing_reason
        .as_deref()
        .context("tool supplement stage is missing provenance")?;
    let provenance: ToolSupplementProvenance =
        serde_json::from_slice(routing_reason).context("tool supplement provenance is invalid")?;
    if provenance.schema != TOOL_SUPPLEMENT_SCHEMA {
        bail!("unsupported tool supplement provenance schema");
    }
    for (field, value) in [
        ("phase", provenance.phase.as_str()),
        ("tool_id", provenance.tool_id.as_str()),
        ("harness", provenance.harness.as_str()),
        ("tool_call_id", provenance.tool_call_id.as_str()),
        ("tool_name", provenance.tool_name.as_str()),
    ] {
        if value.is_empty() {
            bail!("tool supplement provenance has an empty {field}");
        }
    }
    for (field, value, maximum) in [
        ("tool_id", provenance.tool_id.as_str(), 1_024usize),
        ("harness", provenance.harness.as_str(), 200usize),
        ("tool_call_id", provenance.tool_call_id.as_str(), 1_024usize),
        ("tool_name", provenance.tool_name.as_str(), 200usize),
    ] {
        if value.len() > maximum {
            bail!("tool supplement provenance {field} exceeds its bound");
        }
    }
    for (field, value) in [
        ("turn_id", provenance.turn_id.as_deref()),
        ("source_trace_id", provenance.source_trace_id.as_deref()),
    ] {
        if value.is_some_and(|value| value.is_empty() || value.len() > 1_024) {
            bail!("tool supplement provenance has an invalid {field}");
        }
    }
    if provenance.ts_start_ms < 0
        || provenance
            .ts_end_ms
            .is_some_and(|end| end < provenance.ts_start_ms)
    {
        bail!("tool supplement provenance timestamps are invalid");
    }
    let expected_kind = match provenance.phase.as_str() {
        "start" | "arguments" => StageKind::ToolCall,
        "end" | "result" => StageKind::ToolResult,
        phase => bail!("unsupported tool supplement phase {phase}"),
    };
    if stage.data.kind != expected_kind {
        bail!("tool supplement phase does not match its stage kind");
    }
    if matches!(provenance.phase.as_str(), "start" | "arguments") && provenance.ts_end_ms.is_some()
    {
        bail!("tool start supplement unexpectedly carries an end timestamp");
    }
    if matches!(provenance.phase.as_str(), "end" | "result") && provenance.ts_end_ms.is_none() {
        bail!("tool result supplement is missing its end timestamp");
    }
    let phase_time_ms = if matches!(provenance.phase.as_str(), "end" | "result") {
        provenance
            .ts_end_ms
            .expect("result timestamp was validated")
    } else {
        provenance.ts_start_ms
    };
    let phase_time_ns = u64::try_from(phase_time_ms)
        .ok()
        .and_then(|value| value.checked_mul(1_000_000))
        .context("tool supplement phase timestamp exceeds nanoseconds")?;
    if stage.data.wall_time_ns != phase_time_ns || exchange.data.wall_time_ns != phase_time_ns {
        bail!("tool supplement wall time does not match its phase timestamp");
    }
    if exchange.data.stages.as_slice() != [stage.id] {
        bail!("tool supplement exchange does not reference its one validated stage");
    }
    let body_shape_valid = match provenance.phase.as_str() {
        "start" => stage.data.response_body_manifest_ref.is_none(),
        "arguments" => {
            stage.data.request_body_manifest_ref.is_some()
                && stage.data.response_body_manifest_ref.is_none()
        }
        "end" => stage.data.request_body_manifest_ref.is_none(),
        "result" => {
            stage.data.request_body_manifest_ref.is_none()
                && stage.data.response_body_manifest_ref.is_some()
        }
        _ => unreachable!("phase was validated"),
    };
    if !body_shape_valid {
        bail!("tool supplement body references do not match its phase");
    }
    let supplement_trace_id = std::str::from_utf8(&exchange.data.trace_id)
        .context("tool supplement trace ID is not valid UTF-8")?
        .to_string();
    let session_id = exchange
        .data
        .session_id
        .as_deref()
        .context("tool supplement exchange is missing its session ID")
        .and_then(|value| {
            std::str::from_utf8(value).context("tool supplement session ID is not valid UTF-8")
        })?
        .to_string();
    if session_id.is_empty() {
        bail!("tool supplement exchange has an empty session ID");
    }
    if session_id.len() > 1_024 {
        bail!("tool supplement session ID exceeds its bound");
    }
    let expected_trace_id = supplement_trace_id_for_provenance(&provenance, &session_id);
    if supplement_trace_id != expected_trace_id {
        bail!("tool supplement trace ID does not match its provenance identity");
    }
    let parent_trace_id = exchange
        .data
        .parent_trace_id
        .as_deref()
        .map(|value| {
            std::str::from_utf8(value)
                .context("tool supplement parent trace ID is not valid UTF-8")
                .map(str::to_string)
        })
        .transpose()?;
    if parent_trace_id
        .as_deref()
        .is_some_and(|value| value.is_empty() || value.len() > 1_024)
    {
        bail!("tool supplement parent trace ID is invalid");
    }
    if parent_trace_id.as_deref() == Some(supplement_trace_id.as_str()) {
        bail!("tool supplement cannot be its own parent");
    }
    Ok(Some(ParsedToolSupplement {
        provenance,
        supplement_trace_id,
        session_id,
        parent_trace_id,
        stage,
    }))
}

fn supplement_trace_id_for_provenance(
    provenance: &ToolSupplementProvenance,
    session_id: &str,
) -> String {
    supplement_trace_id(
        &provenance.harness,
        session_id,
        &provenance.tool_call_id,
        &provenance.phase,
    )
}

/// Return the authoritative base + validated tool-supplement exchange view.
/// Ordinary children using the general `parent_trace_id` lineage edge are
/// deliberately excluded.
pub(crate) fn canonical_exchange_timeline<'a, R: Read + Seek>(
    reader: &'a ArchiveReader<R>,
    trace_id: &[u8],
) -> Result<Vec<&'a Exchange>> {
    let Some(base) = reader.exchange_by_trace(trace_id) else {
        return Ok(Vec::new());
    };
    let mut timeline = vec![base];
    let mut supplements = Vec::new();
    for exchange in reader.exchange_lineage_by_trace(trace_id) {
        if exchange.data.trace_id == trace_id {
            continue;
        }
        let stages = exchange
            .data
            .stages
            .iter()
            .map(|id| {
                reader
                    .stage(id)
                    .cloned()
                    .with_context(|| format!("tool supplement is missing stage {id}"))
            })
            .collect::<Result<Vec<_>>>()?;
        if let Some(supplement) = parse_tool_supplement(exchange, &stages)? {
            let phase_order = match supplement.provenance.phase.as_str() {
                "start" => 0,
                "arguments" => 1,
                "end" => 2,
                "result" => 3,
                _ => unreachable!("profile parser validated phase"),
            };
            supplements.push((
                supplement.stage.data.wall_time_ns,
                exchange.data.capture_sequence,
                phase_order,
                exchange.data.trace_id.as_slice(),
                exchange,
            ));
        }
    }
    supplements.sort_by(|left, right| {
        (left.0, left.1, left.2, left.3).cmp(&(right.0, right.1, right.2, right.3))
    });
    timeline.extend(supplements.into_iter().map(|value| value.4));
    Ok(timeline)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use alex_lar::{ArchiveWriter, ChunkerConfig, ExchangeData, FileHeader, Limits, StageData};

    use super::*;

    fn provenance(phase: &str) -> ToolSupplementProvenance {
        ToolSupplementProvenance {
            schema: TOOL_SUPPLEMENT_SCHEMA.into(),
            phase: phase.into(),
            tool_id: "tool-1".into(),
            harness: "pi".into(),
            turn_id: Some("turn-1".into()),
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            source_trace_id: Some("parent".into()),
            ts_start_ms: 10,
            ts_end_ms: (phase == "end").then_some(20),
            is_error: Some(false),
            exit_status: (phase == "end").then_some(0),
        }
    }

    fn supplement_exchange(phase: &str, routing_reason: Option<Vec<u8>>) -> (Exchange, Stage) {
        let provenance = provenance(phase);
        let trace_id = supplement_trace_id(
            &provenance.harness,
            "session-1",
            &provenance.tool_call_id,
            phase,
        );
        let kind = if phase == "start" {
            StageKind::ToolCall
        } else {
            StageKind::ToolResult
        };
        let mut stage = StageData::new(
            kind,
            if matches!(phase, "start" | "arguments") {
                10_000_000
            } else {
                20_000_000
            },
        );
        stage.routing_reason = routing_reason;
        let stage = Stage::new(stage);
        let mut exchange = ExchangeData::new(
            trace_id.into_bytes(),
            if phase == "start" { 1 } else { 2 },
            stage.data.wall_time_ns,
            vec![stage.id],
        );
        exchange.session_id = Some(b"session-1".to_vec());
        exchange.parent_trace_id = Some(b"parent".to_vec());
        (Exchange::new(exchange), stage)
    }

    #[test]
    fn canonical_timeline_excludes_ordinary_lineage_children() {
        let mut writer = ArchiveWriter::create(
            Cursor::new(Vec::new()),
            FileHeader::body_pack([11; 16], 1, b"tool-profile-test".to_vec()),
            ChunkerConfig::default(),
            Limits::default(),
        )
        .unwrap();
        let base_stage = writer
            .append_stage(Stage::new(StageData::new(StageKind::RouterDecision, 100)))
            .unwrap();
        let mut base = ExchangeData::new(b"parent".to_vec(), 0, 100, vec![base_stage]);
        base.session_id = Some(b"session-1".to_vec());
        writer.append_exchange(Exchange::new(base)).unwrap();

        let ordinary_stage = writer
            .append_stage(Stage::new(StageData::new(StageKind::ClientRequest, 5)))
            .unwrap();
        let mut ordinary =
            ExchangeData::new(b"ordinary-child".to_vec(), 1, 5, vec![ordinary_stage]);
        ordinary.session_id = Some(b"session-1".to_vec());
        ordinary.parent_trace_id = Some(b"parent".to_vec());
        writer.append_exchange(Exchange::new(ordinary)).unwrap();

        let encoded = serde_json::to_vec(&provenance("start")).unwrap();
        let (supplement, stage) = supplement_exchange("start", Some(encoded));
        writer.append_stage(stage).unwrap();
        writer.append_exchange(supplement).unwrap();
        writer.seal().unwrap();
        let cursor = writer.into_inner().unwrap();
        let reader =
            ArchiveReader::open(Cursor::new(cursor.into_inner()), Limits::default()).unwrap();

        assert_eq!(reader.exchange_lineage_by_trace(b"parent").len(), 3);
        let canonical = canonical_exchange_timeline(&reader, b"parent").unwrap();
        assert_eq!(canonical.len(), 2);
        assert_eq!(canonical[0].data.trace_id, b"parent");
        assert!(canonical[1].data.trace_id.starts_with(b"lar-tool-"));
    }

    #[test]
    fn reserved_supplements_reject_missing_or_tampered_provenance() {
        let (missing, stage) = supplement_exchange("start", None);
        assert!(parse_tool_supplement(&missing, &[stage]).is_err());

        let unknown_phase_stage = Stage::new(StageData::new(StageKind::ToolCall, 1));
        let unknown_phase = Exchange::new(ExchangeData::new(
            b"lar-tool-invalid-unknown".to_vec(),
            0,
            1,
            vec![unknown_phase_stage.id],
        ));
        assert!(parse_tool_supplement(&unknown_phase, &[unknown_phase_stage]).is_err());

        let (wrong_kind, mut stage) = supplement_exchange("start", None);
        stage.data.kind = StageKind::ClientRequest;
        assert!(parse_tool_supplement(&wrong_kind, &[stage]).is_err());

        let mut tampered = provenance("start");
        tampered.tool_call_id = "different-call".into();
        let (exchange, stage) =
            supplement_exchange("start", Some(serde_json::to_vec(&tampered).unwrap()));
        assert!(parse_tool_supplement(&exchange, &[stage]).is_err());

        let ordinary_stage = Stage::new(StageData::new(StageKind::ToolCall, 1));
        let ordinary = Exchange::new(ExchangeData::new(
            b"foreign-tool-trace".to_vec(),
            0,
            1,
            vec![ordinary_stage.id],
        ));
        assert!(parse_tool_supplement(&ordinary, &[ordinary_stage])
            .unwrap()
            .is_none());
    }
}
