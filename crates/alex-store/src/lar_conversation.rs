//! Authoritative conversation-generation catalog over existing raw manifests.
//!
//! This module never parses provider payloads or infers compaction/branching
//! from body similarity. Callers register semantic labels only when their
//! capture/import metadata proves them; unknown formats become raw-only entry
//! ranges. Canonical IDs come from `alex-lar`, while SQLite supplies the paged
//! Trace Browser projection.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use alex_lar::{
    ArtifactRangeRef, ConversationEntry, ConversationEntryData, ConversationEntryId,
    ConversationEntryKind, ConversationRole, Generation, GenerationData, GenerationId,
    GenerationReason, ManifestId, TurnView, TurnViewData, SEMANTIC_SCHEMA_V1,
};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::lar_conversation_adapter::{
    adapt_wire_body, AdaptedEntry, WireDirection, MAX_ADAPTER_BODY_BYTES,
};
use crate::Store;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS lar_conversation_entries (
  entry_id          TEXT PRIMARY KEY,
  semantic_schema   INTEGER NOT NULL,
  role              TEXT NOT NULL,
  kind              TEXT NOT NULL,
  name              BLOB,
  tool_call_id      BLOB,
  created_at_ms     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS lar_conversation_entry_ranges (
  entry_id          TEXT NOT NULL,
  ordinal           INTEGER NOT NULL CHECK (ordinal >= 0),
  manifest_id       TEXT NOT NULL,
  byte_offset       INTEGER NOT NULL CHECK (byte_offset >= 0),
  byte_length       INTEGER NOT NULL CHECK (byte_length > 0),
  PRIMARY KEY (entry_id, ordinal),
  FOREIGN KEY (entry_id) REFERENCES lar_conversation_entries(entry_id),
  FOREIGN KEY (manifest_id) REFERENCES lar_manifests(manifest_id)
);
CREATE INDEX IF NOT EXISTS lar_conversation_ranges_manifest
  ON lar_conversation_entry_ranges(manifest_id, entry_id);

CREATE TABLE IF NOT EXISTS lar_conversation_entry_formats (
  entry_id          TEXT NOT NULL,
  source_format     TEXT NOT NULL,
  PRIMARY KEY (entry_id, source_format),
  FOREIGN KEY (entry_id) REFERENCES lar_conversation_entries(entry_id)
);

CREATE TABLE IF NOT EXISTS lar_conversation_entry_fingerprints (
  entry_id          TEXT PRIMARY KEY,
  content_hash      BLOB NOT NULL CHECK (length(content_hash) = 32),
  content_length    INTEGER NOT NULL CHECK (content_length > 0),
  FOREIGN KEY (entry_id) REFERENCES lar_conversation_entries(entry_id)
);

CREATE TABLE IF NOT EXISTS lar_conversation_generations (
  generation_id       TEXT PRIMARY KEY,
  parent_generation_id TEXT,
  reason              TEXT NOT NULL,
  created_at_ms       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS lar_conversation_generation_entries (
  generation_id     TEXT NOT NULL,
  ordinal           INTEGER NOT NULL CHECK (ordinal >= 0),
  entry_id          TEXT NOT NULL,
  PRIMARY KEY (generation_id, ordinal),
  FOREIGN KEY (generation_id) REFERENCES lar_conversation_generations(generation_id),
  FOREIGN KEY (entry_id) REFERENCES lar_conversation_entries(entry_id)
);

CREATE TABLE IF NOT EXISTS lar_conversation_session_generations (
  session_id          TEXT NOT NULL,
  generation_id       TEXT NOT NULL,
  evidence_source     TEXT,
  evidence_kind       TEXT,
  evidence_id         TEXT,
  created_at_ms       INTEGER NOT NULL,
  PRIMARY KEY (session_id, generation_id),
  FOREIGN KEY (generation_id) REFERENCES lar_conversation_generations(generation_id)
);
CREATE INDEX IF NOT EXISTS lar_conversation_session_generation_order
  ON lar_conversation_session_generations(session_id, created_at_ms, generation_id);

CREATE TABLE IF NOT EXISTS lar_conversation_turn_views (
  turn_view_id       TEXT PRIMARY KEY,
  trace_id           TEXT NOT NULL UNIQUE,
  session_id         TEXT NOT NULL,
  generation_id      TEXT NOT NULL,
  upto_index         INTEGER NOT NULL CHECK (upto_index >= 0),
  created_at_ms      INTEGER NOT NULL,
  FOREIGN KEY (generation_id) REFERENCES lar_conversation_generations(generation_id)
);
CREATE INDEX IF NOT EXISTS lar_conversation_turn_session
  ON lar_conversation_turn_views(session_id, trace_id);

CREATE TABLE IF NOT EXISTS lar_conversation_turn_responses (
  turn_view_id       TEXT NOT NULL,
  ordinal            INTEGER NOT NULL CHECK (ordinal >= 0),
  entry_id           TEXT NOT NULL,
  PRIMARY KEY (turn_view_id, ordinal),
  FOREIGN KEY (turn_view_id) REFERENCES lar_conversation_turn_views(turn_view_id),
  FOREIGN KEY (entry_id) REFERENCES lar_conversation_entries(entry_id)
);
"#;

pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LarConversationRole {
    System,
    User,
    Assistant,
    Tool,
}

impl LarConversationRole {
    fn core(self) -> ConversationRole {
        match self {
            Self::System => ConversationRole::System,
            Self::User => ConversationRole::User,
            Self::Assistant => ConversationRole::Assistant,
            Self::Tool => ConversationRole::Tool,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LarConversationEntryKind {
    Message,
    ToolCall,
    ToolResult,
    Summary,
}

impl LarConversationEntryKind {
    fn core(self) -> ConversationEntryKind {
        match self {
            Self::Message => ConversationEntryKind::Message,
            Self::ToolCall => ConversationEntryKind::ToolCall,
            Self::ToolResult => ConversationEntryKind::ToolResult,
            Self::Summary => ConversationEntryKind::Summary,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::Summary => "summary",
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum LarConversationSemantics {
    Known {
        source_format: String,
        role: LarConversationRole,
        kind: LarConversationEntryKind,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        tool_call_id: Option<String>,
    },
    RawOnly {
        source_format: String,
    },
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct LarConversationRawRange {
    pub manifest_id: String,
    pub byte_offset: u64,
    pub byte_length: u64,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct LarConversationEntryCapture {
    pub semantics: LarConversationSemantics,
    pub raw_ranges: Vec<LarConversationRawRange>,
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LarConversationEvidenceSource {
    Capture,
    Import,
}

impl LarConversationEvidenceSource {
    fn name(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Import => "import",
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct LarConversationEvidence {
    pub source: LarConversationEvidenceSource,
    pub kind: String,
    pub id: String,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum LarConversationGenerationEvent {
    Initial,
    Append {
        parent_generation_id: String,
    },
    Branch {
        parent_generation_id: String,
        evidence: LarConversationEvidence,
    },
    Compaction {
        parent_generation_id: String,
        evidence: LarConversationEvidence,
    },
    Mutation {
        parent_generation_id: String,
        evidence: LarConversationEvidence,
    },
    Import {
        #[serde(default)]
        parent_generation_id: Option<String>,
        evidence: LarConversationEvidence,
    },
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct LarConversationTurnCapture {
    pub trace_id: String,
    pub session_id: String,
    pub event: LarConversationGenerationEvent,
    pub generation_entry_ids: Vec<String>,
    pub upto_index: u64,
    #[serde(default)]
    pub response_entry_ids: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct LarConversationTurnIds {
    pub generation_id: String,
    pub turn_view_id: String,
}

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct LarConversationBackfillReport {
    pub populated: usize,
    pub remaining: bool,
}

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct LarConversationEntryView {
    pub entry_id: String,
    pub semantic_schema: u16,
    pub role: String,
    pub kind: String,
    pub name: Option<String>,
    pub tool_call_id: Option<String>,
    pub source_formats: Vec<String>,
    pub raw_ranges: Vec<LarConversationRawRange>,
}

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct LarConversationEventView {
    pub trace_id: String,
    pub session_id: String,
    pub ts_request_ms: i64,
    pub turn_view_id: String,
    pub generation_id: String,
    pub parent_generation_id: Option<String>,
    pub reason: String,
    pub evidence: Option<LarConversationEvidence>,
    pub upto_index: u64,
    pub entries: Vec<LarConversationEntryView>,
    pub response_entries: Vec<LarConversationEntryView>,
}

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct LarConversationEventPage {
    pub events: Vec<LarConversationEventView>,
    pub total_count: usize,
    pub has_more_after: bool,
    pub next_after: Option<(i64, String)>,
}

pub(crate) struct LarConversationArchiveClosure {
    pub entries: Vec<ConversationEntry>,
    pub generations: Vec<Generation>,
    pub turn: TurnView,
}

impl Store {
    pub fn lar_conversation_has_turn(&self, trace_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM lar_conversation_turn_views WHERE trace_id=?1)",
            [trace_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    /// Load the canonical conversation records needed to make one exported turn
    /// self-contained. Parent generations are returned before descendants and
    /// shared entries appear once. No raw body bytes are loaded here.
    pub(crate) fn lar_conversation_archive_closure(
        &self,
        trace_id: &str,
    ) -> Result<Option<LarConversationArchiveClosure>> {
        let conn = self.conn.lock().unwrap();
        let turn: Option<(String, String, i64)> = conn
            .query_row(
                "SELECT turn_view_id, generation_id, upto_index
                   FROM lar_conversation_turn_views WHERE trace_id=?1",
                [trace_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((turn_id, generation_id, upto_index)) = turn else {
            return Ok(None);
        };
        let mut entries = HashMap::<String, ConversationEntry>::new();
        let mut generations = Vec::new();
        load_archive_generation(&conn, &generation_id, &mut entries, &mut generations)?;
        let response_ids = load_ordered_ids(
            &conn,
            "lar_conversation_turn_responses",
            "turn_view_id",
            "entry_id",
            &turn_id,
        )?;
        for entry_id in &response_ids {
            load_archive_entry(&conn, entry_id, &mut entries)?;
        }
        let turn = TurnView::new(TurnViewData {
            trace_id: trace_id.as_bytes().to_vec(),
            generation_id: parse_generation_id(&generation_id)?,
            upto_index: u64::try_from(upto_index)
                .context("negative conversation turn prefix cursor")?,
            response_entry_refs: response_ids
                .iter()
                .map(|value| parse_entry_id(value))
                .collect::<Result<Vec<_>>>()?,
        });
        if turn.id.to_string() != turn_id {
            bail!("conversation turn catalog identity does not match its contents");
        }
        let mut entries = entries.into_values().collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.id.0);
        Ok(Some(LarConversationArchiveClosure {
            entries,
            generations,
            turn,
        }))
    }

    /// Register one semantic entry over already validated raw body ranges.
    /// Only IDs, labels, and ranges are written; body bytes are never copied.
    pub fn register_lar_conversation_entry(
        &self,
        capture: &LarConversationEntryCapture,
    ) -> Result<String> {
        if capture.raw_ranges.is_empty() {
            bail!("conversation entry must contain at least one raw range");
        }
        let (data, role, kind, source_format) = entry_data(capture)?;
        let entry = ConversationEntry::new(data);
        let entry_id = entry.id.to_string();
        let now = chrono::Utc::now().timestamp_millis();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_ranges(&tx, &capture.raw_ranges)?;

        let inserted = tx.execute(
            "INSERT INTO lar_conversation_entries
               (entry_id, semantic_schema, role, kind, name, tool_call_id, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(entry_id) DO NOTHING",
            params![
                entry_id,
                entry.data.semantic_schema,
                role,
                kind,
                entry.data.name,
                entry.data.tool_call_id,
                now,
            ],
        )?;
        if inserted == 1 {
            for (ordinal, range) in capture.raw_ranges.iter().enumerate() {
                tx.execute(
                    "INSERT INTO lar_conversation_entry_ranges
                       (entry_id, ordinal, manifest_id, byte_offset, byte_length)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        entry_id,
                        to_i64(ordinal as u64, "entry range ordinal")?,
                        range.manifest_id,
                        to_i64(range.byte_offset, "entry range offset")?,
                        to_i64(range.byte_length, "entry range length")?,
                    ],
                )?;
            }
        } else {
            verify_entry(&tx, &entry_id, &entry, &capture.raw_ranges, role, kind)?;
        }
        tx.execute(
            "INSERT INTO lar_conversation_entry_formats (entry_id, source_format)
             VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            params![entry_id, source_format],
        )?;
        tx.commit()?;
        Ok(entry_id)
    }

    /// Persist an explicitly classified generation and its trace turn view.
    /// Branch/compaction/mutation/import variants require non-empty evidence;
    /// this function never derives those reasons from entry or body changes.
    pub fn record_lar_conversation_turn(
        &self,
        capture: &LarConversationTurnCapture,
    ) -> Result<LarConversationTurnIds> {
        if capture.trace_id.is_empty() || capture.session_id.is_empty() {
            bail!("conversation turn trace/session IDs must not be empty");
        }
        if capture.generation_entry_ids.is_empty() {
            bail!("conversation generation must contain at least one entry");
        }
        if capture.upto_index >= capture.generation_entry_ids.len() as u64 {
            bail!("conversation turn upto_index exceeds generation entries");
        }
        let (reason, parent_text, evidence) = event_parts(&capture.event)?;
        let parent_id = parent_text
            .as_deref()
            .map(parse_generation_id)
            .transpose()?;
        let entry_ids = capture
            .generation_entry_ids
            .iter()
            .map(|value| parse_entry_id(value))
            .collect::<Result<Vec<_>>>()?;
        let response_ids = capture
            .response_entry_ids
            .iter()
            .map(|value| parse_entry_id(value))
            .collect::<Result<Vec<_>>>()?;
        let generation = Generation::new(GenerationData {
            parent_generation_id: parent_id,
            entries: entry_ids,
            reason,
        });
        let turn = TurnView::new(TurnViewData {
            trace_id: capture.trace_id.as_bytes().to_vec(),
            generation_id: generation.id,
            upto_index: capture.upto_index,
            response_entry_refs: response_ids,
        });
        let generation_id = generation.id.to_string();
        let turn_view_id = turn.id.to_string();
        let now = chrono::Utc::now().timestamp_millis();

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_trace_session(&tx, &capture.trace_id, &capture.session_id)?;
        validate_entry_ids(&tx, &capture.generation_entry_ids)?;
        validate_entry_ids(&tx, &capture.response_entry_ids)?;
        validate_parent_and_reason(
            &tx,
            &capture.session_id,
            &generation_id,
            parent_text.as_deref(),
            reason,
            &capture.generation_entry_ids,
        )?;

        let inserted_generation = tx.execute(
            "INSERT INTO lar_conversation_generations
               (generation_id, parent_generation_id, reason, created_at_ms)
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT(generation_id) DO NOTHING",
            params![generation_id, parent_text, reason_name(reason), now],
        )?;
        if inserted_generation == 1 {
            insert_ordered_ids(
                &tx,
                "lar_conversation_generation_entries",
                "generation_id",
                "entry_id",
                &generation_id,
                &capture.generation_entry_ids,
            )?;
        } else {
            verify_generation(
                &tx,
                &generation_id,
                parent_text.as_deref(),
                reason,
                &capture.generation_entry_ids,
            )?;
        }

        let evidence_values = evidence
            .as_ref()
            .map(|value| (value.source.name(), value.kind.as_str(), value.id.as_str()));
        tx.execute(
            "INSERT INTO lar_conversation_session_generations
               (session_id, generation_id, evidence_source, evidence_kind,
                evidence_id, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id, generation_id) DO NOTHING",
            params![
                capture.session_id,
                generation_id,
                evidence_values.map(|value| value.0),
                evidence_values.map(|value| value.1),
                evidence_values.map(|value| value.2),
                now,
            ],
        )?;
        verify_session_generation_evidence(
            &tx,
            &capture.session_id,
            &generation_id,
            evidence.as_ref(),
        )?;

        let existing_turn: Option<String> = tx
            .query_row(
                "SELECT turn_view_id FROM lar_conversation_turn_views WHERE trace_id=?1",
                [&capture.trace_id],
                |row| row.get(0),
            )
            .optional()?;
        if existing_turn
            .as_deref()
            .is_some_and(|value| value != turn_view_id)
        {
            bail!("trace is already bound to a different conversation turn view");
        }
        let inserted_turn = tx.execute(
            "INSERT INTO lar_conversation_turn_views
               (turn_view_id, trace_id, session_id, generation_id, upto_index, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(turn_view_id) DO NOTHING",
            params![
                turn_view_id,
                capture.trace_id,
                capture.session_id,
                generation_id,
                to_i64(capture.upto_index, "turn upto index")?,
                now,
            ],
        )?;
        if inserted_turn == 1 {
            insert_ordered_ids(
                &tx,
                "lar_conversation_turn_responses",
                "turn_view_id",
                "entry_id",
                &turn_view_id,
                &capture.response_entry_ids,
            )?;
        } else {
            verify_turn(&tx, &turn_view_id, capture, &generation_id)?;
        }
        if inserted_turn == 1 {
            tx.execute(
                "INSERT INTO lar_session_revisions (session_id, revision, updated_at_ms)
                 VALUES (?1, 1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET
                   revision=lar_session_revisions.revision+1,
                   updated_at_ms=excluded.updated_at_ms",
                params![capture.session_id, now],
            )?;
        }
        tx.commit()?;
        Ok(LarConversationTurnIds {
            generation_id,
            turn_view_id,
        })
    }

    /// Return a bounded chronological event page for the Trace Browser.
    pub fn lar_conversation_events_page(
        &self,
        session_id: &str,
        after: Option<(i64, String)>,
        limit: usize,
    ) -> Result<LarConversationEventPage> {
        self.lar_conversation_events_page_impl(session_id, after, limit, true)
    }

    /// Browser timeline projection that avoids expanding every entry in every
    /// growing generation. IDs, ancestry, reason/evidence, and entry boundary
    /// (`upto_index`) remain authoritative; callers can fetch the detailed
    /// page when they actually need raw range references.
    pub fn lar_conversation_event_summaries_page(
        &self,
        session_id: &str,
        after: Option<(i64, String)>,
        limit: usize,
    ) -> Result<LarConversationEventPage> {
        self.lar_conversation_events_page_impl(session_id, after, limit, false)
    }

    fn lar_conversation_events_page_impl(
        &self,
        session_id: &str,
        after: Option<(i64, String)>,
        limit: usize,
        include_entries: bool,
    ) -> Result<LarConversationEventPage> {
        let limit = limit.clamp(1, 500);
        let conn = self.conn.lock().unwrap();
        let total_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM lar_conversation_turn_views WHERE session_id=?1",
            [session_id],
            |row| row.get(0),
        )?;
        let mut sql = String::from(
            "SELECT tv.trace_id, tv.session_id, t.ts_request_ms, tv.turn_view_id,
                    tv.generation_id, g.parent_generation_id, g.reason,
                    sg.evidence_source, sg.evidence_kind, sg.evidence_id, tv.upto_index
               FROM lar_conversation_turn_views tv
               JOIN traces t ON t.id=tv.trace_id
               JOIN lar_conversation_generations g ON g.generation_id=tv.generation_id
               JOIN lar_conversation_session_generations sg
                 ON sg.session_id=tv.session_id AND sg.generation_id=tv.generation_id
              WHERE tv.session_id=?1",
        );
        let mut values = vec![rusqlite::types::Value::Text(session_id.to_string())];
        if let Some((timestamp, trace_id)) = after {
            sql.push_str(" AND (t.ts_request_ms > ?2 OR (t.ts_request_ms=?2 AND tv.trace_id>?3))");
            values.push(rusqlite::types::Value::Integer(timestamp));
            values.push(rusqlite::types::Value::Text(trace_id));
        }
        sql.push_str(" ORDER BY t.ts_request_ms, tv.trace_id LIMIT ?");
        values.push(rusqlite::types::Value::Integer((limit + 1) as i64));
        let mut statement = conn.prepare(&sql)?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(values), |row| {
                Ok(EventRow {
                    trace_id: row.get(0)?,
                    session_id: row.get(1)?,
                    ts_request_ms: row.get(2)?,
                    turn_view_id: row.get(3)?,
                    generation_id: row.get(4)?,
                    parent_generation_id: row.get(5)?,
                    reason: row.get(6)?,
                    evidence_source: row.get(7)?,
                    evidence_kind: row.get(8)?,
                    evidence_id: row.get(9)?,
                    upto_index: row.get::<_, i64>(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more_after = rows.len() > limit;
        let mut rows = rows;
        rows.truncate(limit);
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            events.push(load_event_view(&conn, row, include_entries)?);
        }
        let next_after = events
            .last()
            .map(|event| (event.ts_request_ms, event.trace_id.clone()));
        Ok(LarConversationEventPage {
            events,
            total_count: usize::try_from(total_count)
                .context("negative conversation event count")?,
            has_more_after,
            next_after,
        })
    }

    /// Populate one trace from its validated client request/response manifests.
    /// Provider parsing is bounded and lexical; malformed, oversized, and
    /// unknown bodies become one raw-only full-manifest entry. The method is
    /// idempotent and never writes body/chunk/archive records.
    pub fn populate_lar_conversation_for_trace(
        &self,
        trace_id: &str,
        source: LarConversationEvidenceSource,
    ) -> Result<Option<LarConversationTurnIds>> {
        let Some(trace) = load_trace_source(self, trace_id)? else {
            return Ok(None);
        };
        let request_body = bounded_manifest_body(self, &trace.request)?;
        let request_candidates = adapted_candidates(
            &trace.source_format,
            WireDirection::Request,
            &trace.request,
            request_body.as_deref(),
        )?;
        if request_candidates.is_empty() {
            return Ok(None);
        }
        let response_candidates = match &trace.response {
            Some(response) => {
                let body = bounded_manifest_body(self, response)?;
                adapted_candidates(
                    &trace.source_format,
                    WireDirection::Response,
                    response,
                    body.as_deref(),
                )?
            }
            None => Vec::new(),
        };

        let previous =
            previous_generation(self, &trace.session_id, trace.timestamp, &trace.trace_id)?;
        let common_prefix = match &previous {
            Some(previous) => reusable_prefix(self, &previous.entry_ids, &request_candidates)?,
            None => 0,
        };
        let mut request_entry_ids = Vec::with_capacity(request_candidates.len());
        if let Some(previous) = &previous {
            request_entry_ids.extend(previous.entry_ids[..common_prefix].iter().cloned());
        }
        for candidate in request_candidates.iter().skip(common_prefix) {
            request_entry_ids.push(self.register_candidate(candidate)?);
        }
        let response_entry_ids = response_candidates
            .iter()
            .map(|candidate| self.register_candidate(candidate))
            .collect::<Result<Vec<_>>>()?;

        let event = match previous {
            None => LarConversationGenerationEvent::Initial,
            Some(previous) if common_prefix == previous.entry_ids.len() => {
                LarConversationGenerationEvent::Append {
                    parent_generation_id: previous.generation_id,
                }
            }
            Some(previous) => LarConversationGenerationEvent::Mutation {
                parent_generation_id: previous.generation_id,
                evidence: LarConversationEvidence {
                    source,
                    kind: "request_history_diverged".into(),
                    // This proves only which exact request exhibited the
                    // divergence; it deliberately does not claim compaction.
                    id: trace.request.manifest_id.clone(),
                },
            },
        };
        self.record_lar_conversation_turn(&LarConversationTurnCapture {
            trace_id: trace.trace_id,
            session_id: trace.session_id,
            event,
            upto_index: request_entry_ids.len().saturating_sub(1) as u64,
            generation_entry_ids: request_entry_ids,
            response_entry_ids,
        })
        .map(Some)
    }

    /// Bounded backfill used by both foreground and startup legacy migration.
    /// Traces are processed in chronological order so initial/append decisions
    /// are deterministic even when their body files were imported in another
    /// order.
    pub fn backfill_lar_conversations(
        &self,
        limit: usize,
    ) -> Result<LarConversationBackfillReport> {
        let limit = limit.clamp(1, 4096);
        let trace_ids = conversation_backfill_candidates(self, limit)?;
        let mut populated = 0usize;
        for trace_id in &trace_ids {
            if self
                .populate_lar_conversation_for_trace(
                    trace_id,
                    LarConversationEvidenceSource::Import,
                )?
                .is_some()
            {
                populated += 1;
            }
        }
        Ok(LarConversationBackfillReport {
            populated,
            remaining: has_conversation_backfill_candidate(self)?,
        })
    }

    fn register_candidate(&self, candidate: &ConversationCandidate) -> Result<String> {
        let entry_id = self.register_lar_conversation_entry(&candidate.capture)?;
        if let Some((hash, length)) = candidate.fingerprint {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO lar_conversation_entry_fingerprints
                   (entry_id, content_hash, content_length)
                 VALUES (?1, ?2, ?3) ON CONFLICT(entry_id) DO NOTHING",
                params![
                    entry_id,
                    hash.to_vec(),
                    to_i64(length, "entry content length")?
                ],
            )?;
            let stored: (Vec<u8>, i64) = conn.query_row(
                "SELECT content_hash, content_length
                 FROM lar_conversation_entry_fingerprints WHERE entry_id=?1",
                [&entry_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if stored != (hash.to_vec(), to_i64(length, "entry content length")?) {
                bail!("conversation entry fingerprint conflicts with canonical entry ID");
            }
        }
        Ok(entry_id)
    }
}

#[derive(Clone, Debug)]
struct ManifestSource {
    manifest_id: String,
    total_length: u64,
}

#[derive(Clone, Debug)]
struct TraceConversationSource {
    trace_id: String,
    session_id: String,
    timestamp: i64,
    source_format: String,
    request: ManifestSource,
    response: Option<ManifestSource>,
}

#[derive(Clone, Debug)]
struct PreviousGeneration {
    generation_id: String,
    entry_ids: Vec<String>,
}

#[derive(Clone, Debug)]
struct ConversationCandidate {
    capture: LarConversationEntryCapture,
    fingerprint: Option<([u8; 32], u64)>,
}

fn load_trace_source(store: &Store, trace_id: &str) -> Result<Option<TraceConversationSource>> {
    let conn = store.conn.lock().unwrap();
    let trace: Option<(Option<String>, i64, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT session_id, ts_request_ms, client_format, resp_body_path
             FROM traces WHERE id=?1",
            [trace_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((Some(session_id), timestamp, source_format, response_path)) = trace else {
        return Ok(None);
    };
    if session_id.is_empty() {
        return Ok(None);
    }
    let Some(request) = trace_manifest_source(&conn, trace_id, "client_request")? else {
        return Ok(None);
    };
    let response = trace_manifest_source(&conn, trace_id, "client_response")?;
    // During legacy migration the request pointer can become visible before
    // the response pointer. Defer the turn rather than publishing an
    // incomplete TurnView that could not later retain its canonical ID.
    if response_path.is_some() && response.is_none() {
        return Ok(None);
    }
    let source_format = source_format
        .filter(|value| !value.is_empty() && value.len() <= 1024)
        .unwrap_or_else(|| "unknown".into());
    Ok(Some(TraceConversationSource {
        trace_id: trace_id.into(),
        session_id,
        timestamp,
        source_format,
        request,
        response,
    }))
}

fn trace_manifest_source(
    conn: &Connection,
    trace_id: &str,
    artifact_kind: &str,
) -> Result<Option<ManifestSource>> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT a.manifest_id, m.total_length
               FROM lar_trace_artifacts a
               JOIN lar_manifests m ON m.manifest_id=a.manifest_id AND m.state='ready'
              WHERE a.owner_kind='trace' AND a.owner_id=?1
                AND a.artifact_kind=?2 AND a.stage_id=''
              LIMIT 1",
            params![trace_id, artifact_kind],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    row.map(|(manifest_id, length)| {
        Ok(ManifestSource {
            manifest_id,
            total_length: u64::try_from(length).context("negative manifest body length")?,
        })
    })
    .transpose()
}

fn bounded_manifest_body(store: &Store, source: &ManifestSource) -> Result<Option<Vec<u8>>> {
    if source.total_length > MAX_ADAPTER_BODY_BYTES {
        return Ok(None);
    }
    let body = store.read_lar_manifest_body(&source.manifest_id)?;
    if body.len() as u64 != source.total_length {
        bail!("conversation adapter manifest length changed during read");
    }
    Ok(Some(body))
}

fn adapted_candidates(
    source_format: &str,
    direction: WireDirection,
    source: &ManifestSource,
    body: Option<&[u8]>,
) -> Result<Vec<ConversationCandidate>> {
    if source.total_length == 0 {
        return Ok(Vec::new());
    }
    if let Some(body) = body {
        if let Ok(entries) = adapt_wire_body(source_format, direction, body) {
            return entries
                .into_iter()
                .map(|entry| known_candidate(source_format, source, body, entry))
                .collect();
        }
    }
    let fingerprint = body.map(|bytes| {
        (
            *blake3::hash(bytes).as_bytes(),
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        )
    });
    Ok(vec![ConversationCandidate {
        capture: LarConversationEntryCapture {
            semantics: LarConversationSemantics::RawOnly {
                source_format: source_format.into(),
            },
            raw_ranges: vec![LarConversationRawRange {
                manifest_id: source.manifest_id.clone(),
                byte_offset: 0,
                byte_length: source.total_length,
            }],
        },
        fingerprint,
    }])
}

fn known_candidate(
    source_format: &str,
    source: &ManifestSource,
    body: &[u8],
    entry: AdaptedEntry,
) -> Result<ConversationCandidate> {
    let end = entry
        .byte_offset
        .checked_add(entry.byte_length)
        .context("adapted conversation span overflow")?;
    if entry.byte_length == 0 || end > body.len() as u64 || end > source.total_length {
        bail!("adapted conversation span exceeds its exact manifest body");
    }
    let bytes = &body[entry.byte_offset as usize..end as usize];
    Ok(ConversationCandidate {
        capture: LarConversationEntryCapture {
            semantics: LarConversationSemantics::Known {
                source_format: source_format.into(),
                role: entry.role,
                kind: entry.kind,
                name: entry.name,
                tool_call_id: entry.tool_call_id,
            },
            raw_ranges: vec![LarConversationRawRange {
                manifest_id: source.manifest_id.clone(),
                byte_offset: entry.byte_offset,
                byte_length: entry.byte_length,
            }],
        },
        fingerprint: Some((*blake3::hash(bytes).as_bytes(), entry.byte_length)),
    })
}

fn previous_generation(
    store: &Store,
    session_id: &str,
    timestamp: i64,
    trace_id: &str,
) -> Result<Option<PreviousGeneration>> {
    let conn = store.conn.lock().unwrap();
    let previous: Option<(String, i64)> = conn
        .query_row(
            "SELECT tv.generation_id, tv.upto_index
               FROM lar_conversation_turn_views tv
               JOIN traces t ON t.id=tv.trace_id
              WHERE tv.session_id=?1
                AND (t.ts_request_ms < ?2 OR (t.ts_request_ms=?2 AND t.id < ?3))
              ORDER BY t.ts_request_ms DESC, t.id DESC
              LIMIT 1",
            params![session_id, timestamp, trace_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((generation_id, upto_index)) = previous else {
        return Ok(None);
    };
    let mut entry_ids = load_ordered_ids(
        &conn,
        "lar_conversation_generation_entries",
        "generation_id",
        "entry_id",
        &generation_id,
    )?;
    let count = usize::try_from(upto_index)
        .context("negative prior conversation prefix cursor")?
        .checked_add(1)
        .context("prior conversation prefix cursor overflow")?;
    if count > entry_ids.len() {
        bail!("prior conversation prefix cursor exceeds its generation");
    }
    entry_ids.truncate(count);
    Ok(Some(PreviousGeneration {
        generation_id,
        entry_ids,
    }))
}

fn reusable_prefix(
    store: &Store,
    previous: &[String],
    current: &[ConversationCandidate],
) -> Result<usize> {
    let conn = store.conn.lock().unwrap();
    let mut matched = 0usize;
    for (entry_id, candidate) in previous.iter().zip(current) {
        let stored: Option<(
            i64,
            String,
            String,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
            Option<i64>,
        )> = conn
            .query_row(
                "SELECT e.semantic_schema, e.role, e.kind, e.name, e.tool_call_id,
                        f.content_hash, f.content_length
                   FROM lar_conversation_entries e
                   LEFT JOIN lar_conversation_entry_fingerprints f ON f.entry_id=e.entry_id
                  WHERE e.entry_id=?1",
                [entry_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some(stored) = stored else {
            break;
        };
        if !candidate_matches(&stored, candidate)? {
            break;
        }
        matched += 1;
    }
    Ok(matched)
}

fn candidate_matches(
    stored: &(
        i64,
        String,
        String,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<i64>,
    ),
    candidate: &ConversationCandidate,
) -> Result<bool> {
    let semantic = match &candidate.capture.semantics {
        LarConversationSemantics::Known {
            role,
            kind,
            name,
            tool_call_id,
            ..
        } => (
            i64::from(SEMANTIC_SCHEMA_V1),
            role.name(),
            kind.name(),
            name.as_deref().map(str::as_bytes).map(Vec::from),
            tool_call_id.as_deref().map(str::as_bytes).map(Vec::from),
        ),
        LarConversationSemantics::RawOnly { .. } => (0, "opaque", "opaque", None, None),
    };
    if (
        stored.0,
        stored.1.as_str(),
        stored.2.as_str(),
        stored.3.clone(),
        stored.4.clone(),
    ) != semantic
    {
        return Ok(false);
    }
    let Some((hash, length)) = candidate.fingerprint else {
        return Ok(false);
    };
    Ok(stored.5.as_deref() == Some(hash.as_slice())
        && stored.6 == Some(to_i64(length, "entry content length")?))
}

fn conversation_backfill_candidates(store: &Store, limit: usize) -> Result<Vec<String>> {
    let conn = store.conn.lock().unwrap();
    let mut statement = conn.prepare(
        "SELECT t.id
           FROM traces t
           JOIN lar_trace_artifacts request
             ON request.owner_kind='trace' AND request.owner_id=t.id
            AND request.artifact_kind='client_request' AND request.stage_id=''
           JOIN lar_manifests request_manifest
             ON request_manifest.manifest_id=request.manifest_id
            AND request_manifest.state='ready'
          WHERE t.session_id IS NOT NULL AND t.session_id<>''
            AND NOT EXISTS (
                SELECT 1 FROM lar_conversation_turn_views tv WHERE tv.trace_id=t.id
            )
            AND (t.resp_body_path IS NULL OR EXISTS (
                SELECT 1 FROM lar_trace_artifacts response
                JOIN lar_manifests response_manifest
                  ON response_manifest.manifest_id=response.manifest_id
                 AND response_manifest.state='ready'
                 WHERE response.owner_kind='trace' AND response.owner_id=t.id
                   AND response.artifact_kind='client_response' AND response.stage_id=''
            ))
          ORDER BY t.ts_request_ms, t.id
          LIMIT ?1",
    )?;
    let rows = statement
        .query_map(
            [to_i64(limit as u64, "conversation backfill limit")?],
            |row| row.get(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn has_conversation_backfill_candidate(store: &Store) -> Result<bool> {
    Ok(!conversation_backfill_candidates(store, 1)?.is_empty())
}

struct EventRow {
    trace_id: String,
    session_id: String,
    ts_request_ms: i64,
    turn_view_id: String,
    generation_id: String,
    parent_generation_id: Option<String>,
    reason: String,
    evidence_source: Option<String>,
    evidence_kind: Option<String>,
    evidence_id: Option<String>,
    upto_index: i64,
}

fn entry_data(
    capture: &LarConversationEntryCapture,
) -> Result<(ConversationEntryData, &'static str, &'static str, &str)> {
    let ranges = capture
        .raw_ranges
        .iter()
        .map(|range| {
            Ok(ArtifactRangeRef {
                manifest_id: range
                    .manifest_id
                    .parse::<ManifestId>()
                    .map_err(anyhow::Error::new)?,
                byte_offset: range.byte_offset,
                byte_length: range.byte_length,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    match &capture.semantics {
        LarConversationSemantics::RawOnly { source_format } => {
            validate_source_format(source_format)?;
            Ok((
                ConversationEntryData::raw_only(ranges),
                "opaque",
                "opaque",
                source_format,
            ))
        }
        LarConversationSemantics::Known {
            source_format,
            role,
            kind,
            name,
            tool_call_id,
        } => {
            validate_source_format(source_format)?;
            Ok((
                ConversationEntryData {
                    semantic_schema: SEMANTIC_SCHEMA_V1,
                    role: role.core(),
                    kind: kind.core(),
                    raw_ranges: ranges,
                    name: name.as_deref().map(str::as_bytes).map(Vec::from),
                    tool_call_id: tool_call_id.as_deref().map(str::as_bytes).map(Vec::from),
                },
                role.name(),
                kind.name(),
                source_format,
            ))
        }
    }
}

fn validate_source_format(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 1024 {
        bail!("conversation source format must contain 1..=1024 bytes");
    }
    Ok(())
}

fn validate_evidence(value: &LarConversationEvidence) -> Result<()> {
    if value.kind.is_empty() || value.id.is_empty() {
        bail!("explicit conversation generation evidence must include kind and ID");
    }
    if value.kind.len() > 1024 || value.id.len() > 4096 {
        bail!("conversation generation evidence exceeds its metadata limit");
    }
    Ok(())
}

fn event_parts(
    event: &LarConversationGenerationEvent,
) -> Result<(
    GenerationReason,
    Option<String>,
    Option<LarConversationEvidence>,
)> {
    let values = match event {
        LarConversationGenerationEvent::Initial => (GenerationReason::Initial, None, None),
        LarConversationGenerationEvent::Append {
            parent_generation_id,
        } => (
            GenerationReason::Append,
            Some(parent_generation_id.clone()),
            None,
        ),
        LarConversationGenerationEvent::Branch {
            parent_generation_id,
            evidence,
        } => (
            GenerationReason::Branch,
            Some(parent_generation_id.clone()),
            Some(evidence.clone()),
        ),
        LarConversationGenerationEvent::Compaction {
            parent_generation_id,
            evidence,
        } => (
            GenerationReason::Compaction,
            Some(parent_generation_id.clone()),
            Some(evidence.clone()),
        ),
        LarConversationGenerationEvent::Mutation {
            parent_generation_id,
            evidence,
        } => (
            GenerationReason::Mutation,
            Some(parent_generation_id.clone()),
            Some(evidence.clone()),
        ),
        LarConversationGenerationEvent::Import {
            parent_generation_id,
            evidence,
        } => (
            GenerationReason::Import,
            parent_generation_id.clone(),
            Some(evidence.clone()),
        ),
    };
    if let Some(evidence) = &values.2 {
        validate_evidence(evidence)?;
    }
    if values.1.as_deref().is_some_and(|parent| parent.is_empty()) {
        bail!("parent generation ID must not be empty");
    }
    Ok(values)
}

fn validate_ranges(tx: &Transaction<'_>, ranges: &[LarConversationRawRange]) -> Result<()> {
    for range in ranges {
        if range.byte_length == 0 {
            bail!("conversation raw range must not be empty");
        }
        let end = range
            .byte_offset
            .checked_add(range.byte_length)
            .context("conversation raw range overflow")?;
        let length: Option<i64> = tx
            .query_row(
                "SELECT total_length FROM lar_manifests
                 WHERE manifest_id=?1 AND state='ready'",
                [&range.manifest_id],
                |row| row.get(0),
            )
            .optional()?;
        let length = length.with_context(|| {
            format!(
                "conversation raw range references unavailable manifest {}",
                range.manifest_id
            )
        })?;
        if end > u64::try_from(length).context("negative manifest length")? {
            bail!("conversation raw range exceeds its manifest body");
        }
    }
    Ok(())
}

fn verify_entry(
    tx: &Transaction<'_>,
    entry_id: &str,
    entry: &ConversationEntry,
    expected_ranges: &[LarConversationRawRange],
    expected_role: &str,
    expected_kind: &str,
) -> Result<()> {
    let stored: (i64, String, String, Option<Vec<u8>>, Option<Vec<u8>>) = tx.query_row(
        "SELECT semantic_schema, role, kind, name, tool_call_id
         FROM lar_conversation_entries WHERE entry_id=?1",
        [entry_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    if stored
        != (
            i64::from(entry.data.semantic_schema),
            expected_role.to_string(),
            expected_kind.to_string(),
            entry.data.name.clone(),
            entry.data.tool_call_id.clone(),
        )
    {
        bail!("conversation entry ID is bound to incompatible semantics");
    }
    let ranges = load_range_rows(tx, entry_id)?;
    if ranges != expected_ranges {
        bail!("conversation entry ID is bound to incompatible raw ranges");
    }
    Ok(())
}

fn validate_trace_session(tx: &Transaction<'_>, trace_id: &str, session_id: &str) -> Result<()> {
    let stored: Option<Option<String>> = tx
        .query_row(
            "SELECT session_id FROM traces WHERE id=?1",
            [trace_id],
            |row| row.get(0),
        )
        .optional()?;
    match stored {
        None => bail!("conversation turn trace does not exist: {trace_id}"),
        Some(Some(stored)) if stored == session_id => Ok(()),
        _ => bail!("conversation turn session does not match its trace"),
    }
}

fn validate_entry_ids(tx: &Transaction<'_>, ids: &[String]) -> Result<()> {
    for id in ids {
        let exists = tx
            .query_row(
                "SELECT 1 FROM lar_conversation_entries WHERE entry_id=?1",
                [id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            bail!("conversation generation references unknown entry {id}");
        }
    }
    Ok(())
}

fn validate_parent_and_reason(
    tx: &Transaction<'_>,
    session_id: &str,
    generation_id: &str,
    parent: Option<&str>,
    reason: GenerationReason,
    entries: &[String],
) -> Result<()> {
    if reason == GenerationReason::Initial {
        if parent.is_some() {
            bail!("initial conversation generation cannot have a parent");
        }
        let existing: i64 = tx.query_row(
            "SELECT COUNT(*) FROM lar_conversation_session_generations
             WHERE session_id=?1 AND generation_id<>?2",
            params![session_id, generation_id],
            |row| row.get(0),
        )?;
        if existing > 0 {
            bail!("conversation session already has a generation root");
        }
        return Ok(());
    }
    if reason == GenerationReason::Import && parent.is_none() {
        return Ok(());
    }
    let parent = parent.context("derived conversation generation requires a parent")?;
    let parent_exists = tx
        .query_row(
            "SELECT 1 FROM lar_conversation_session_generations
             WHERE session_id=?1 AND generation_id=?2",
            params![session_id, parent],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !parent_exists {
        bail!("conversation generation parent is not present in this session");
    }
    if reason == GenerationReason::Append {
        let parent_entries = load_ordered_ids(
            tx,
            "lar_conversation_generation_entries",
            "generation_id",
            "entry_id",
            parent,
        )?;
        if entries.len() < parent_entries.len() || entries[..parent_entries.len()] != parent_entries
        {
            bail!("append generation must retain its parent's exact entry prefix");
        }
    }
    Ok(())
}

fn insert_ordered_ids(
    tx: &Transaction<'_>,
    table: &str,
    owner_column: &str,
    value_column: &str,
    owner_id: &str,
    values: &[String],
) -> Result<()> {
    let sql = format!(
        "INSERT INTO {table} ({owner_column}, ordinal, {value_column}) VALUES (?1, ?2, ?3)"
    );
    let mut statement = tx.prepare(&sql)?;
    for (ordinal, value) in values.iter().enumerate() {
        statement.execute(params![
            owner_id,
            to_i64(ordinal as u64, "conversation ordinal")?,
            value
        ])?;
    }
    Ok(())
}

fn verify_generation(
    tx: &Transaction<'_>,
    generation_id: &str,
    parent: Option<&str>,
    reason: GenerationReason,
    entries: &[String],
) -> Result<()> {
    let stored: (Option<String>, String) = tx.query_row(
        "SELECT parent_generation_id, reason FROM lar_conversation_generations
         WHERE generation_id=?1",
        [generation_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if stored != (parent.map(str::to_owned), reason_name(reason).to_string())
        || load_ordered_ids(
            tx,
            "lar_conversation_generation_entries",
            "generation_id",
            "entry_id",
            generation_id,
        )? != entries
    {
        bail!("conversation generation ID is bound to incompatible metadata");
    }
    Ok(())
}

fn verify_session_generation_evidence(
    tx: &Transaction<'_>,
    session_id: &str,
    generation_id: &str,
    expected: Option<&LarConversationEvidence>,
) -> Result<()> {
    let stored: (Option<String>, Option<String>, Option<String>) = tx.query_row(
        "SELECT evidence_source, evidence_kind, evidence_id
         FROM lar_conversation_session_generations
         WHERE session_id=?1 AND generation_id=?2",
        params![session_id, generation_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let expected = expected
        .map(|value| {
            (
                Some(value.source.name().to_string()),
                Some(value.kind.clone()),
                Some(value.id.clone()),
            )
        })
        .unwrap_or((None, None, None));
    if stored != expected {
        bail!("conversation generation evidence conflicts with its existing event");
    }
    Ok(())
}

fn verify_turn(
    tx: &Transaction<'_>,
    turn_view_id: &str,
    capture: &LarConversationTurnCapture,
    generation_id: &str,
) -> Result<()> {
    let stored: (String, String, String, i64) = tx.query_row(
        "SELECT trace_id, session_id, generation_id, upto_index
         FROM lar_conversation_turn_views WHERE turn_view_id=?1",
        [turn_view_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    if stored
        != (
            capture.trace_id.clone(),
            capture.session_id.clone(),
            generation_id.to_string(),
            to_i64(capture.upto_index, "turn upto index")?,
        )
        || load_ordered_ids(
            tx,
            "lar_conversation_turn_responses",
            "turn_view_id",
            "entry_id",
            turn_view_id,
        )? != capture.response_entry_ids
    {
        bail!("conversation turn view ID is bound to incompatible metadata");
    }
    Ok(())
}

fn load_ordered_ids(
    conn: &Connection,
    table: &str,
    owner_column: &str,
    value_column: &str,
    owner_id: &str,
) -> Result<Vec<String>> {
    let sql =
        format!("SELECT {value_column} FROM {table} WHERE {owner_column}=?1 ORDER BY ordinal");
    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map([owner_id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn load_range_rows(conn: &Connection, entry_id: &str) -> Result<Vec<LarConversationRawRange>> {
    let mut statement = conn.prepare(
        "SELECT manifest_id, byte_offset, byte_length
         FROM lar_conversation_entry_ranges WHERE entry_id=?1 ORDER BY ordinal",
    )?;
    let rows = statement
        .query_map([entry_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(manifest_id, offset, length)| {
            Ok(LarConversationRawRange {
                manifest_id,
                byte_offset: u64::try_from(offset)
                    .context("negative conversation raw range offset")?,
                byte_length: u64::try_from(length)
                    .context("negative conversation raw range length")?,
            })
        })
        .collect()
}

fn load_entry_view(conn: &Connection, entry_id: &str) -> Result<LarConversationEntryView> {
    let row: (i64, String, String, Option<Vec<u8>>, Option<Vec<u8>>) = conn.query_row(
        "SELECT semantic_schema, role, kind, name, tool_call_id
         FROM lar_conversation_entries WHERE entry_id=?1",
        [entry_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let mut formats = conn.prepare(
        "SELECT source_format FROM lar_conversation_entry_formats
         WHERE entry_id=?1 ORDER BY source_format",
    )?;
    let source_formats = formats
        .query_map([entry_id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(LarConversationEntryView {
        entry_id: entry_id.to_string(),
        semantic_schema: u16::try_from(row.0).context("invalid conversation semantic schema")?,
        role: row.1,
        kind: row.2,
        name: row
            .3
            .map(|value| String::from_utf8_lossy(&value).into_owned()),
        tool_call_id: row
            .4
            .map(|value| String::from_utf8_lossy(&value).into_owned()),
        source_formats,
        raw_ranges: load_range_rows(conn, entry_id)?,
    })
}

fn load_event_view(
    conn: &Connection,
    row: EventRow,
    include_entries: bool,
) -> Result<LarConversationEventView> {
    let (entry_ids, response_ids) = if include_entries {
        (
            load_ordered_ids(
                conn,
                "lar_conversation_generation_entries",
                "generation_id",
                "entry_id",
                &row.generation_id,
            )?,
            load_ordered_ids(
                conn,
                "lar_conversation_turn_responses",
                "turn_view_id",
                "entry_id",
                &row.turn_view_id,
            )?,
        )
    } else {
        (vec![], vec![])
    };
    let evidence = match (
        row.evidence_source.as_deref(),
        row.evidence_kind,
        row.evidence_id,
    ) {
        (None, None, None) => None,
        (Some(source), Some(kind), Some(id)) => Some(LarConversationEvidence {
            source: match source {
                "capture" => LarConversationEvidenceSource::Capture,
                "import" => LarConversationEvidenceSource::Import,
                other => bail!("unknown conversation evidence source {other}"),
            },
            kind,
            id,
        }),
        _ => bail!("incomplete conversation generation evidence"),
    };
    Ok(LarConversationEventView {
        trace_id: row.trace_id,
        session_id: row.session_id,
        ts_request_ms: row.ts_request_ms,
        turn_view_id: row.turn_view_id,
        generation_id: row.generation_id,
        parent_generation_id: row.parent_generation_id,
        reason: row.reason,
        evidence,
        upto_index: u64::try_from(row.upto_index).context("negative turn upto index")?,
        entries: if include_entries {
            entry_ids
                .iter()
                .map(|id| load_entry_view(conn, id))
                .collect::<Result<Vec<_>>>()?
        } else {
            vec![]
        },
        response_entries: if include_entries {
            response_ids
                .iter()
                .map(|id| load_entry_view(conn, id))
                .collect::<Result<Vec<_>>>()?
        } else {
            vec![]
        },
    })
}

fn load_archive_generation(
    conn: &Connection,
    generation_id: &str,
    entries: &mut HashMap<String, ConversationEntry>,
    generations: &mut Vec<Generation>,
) -> Result<()> {
    let mut ancestry = Vec::<(String, Option<String>, String)>::new();
    let mut visited = HashSet::new();
    let mut current = Some(generation_id.to_owned());
    while let Some(id) = current {
        if !visited.insert(id.clone()) {
            bail!("conversation generation catalog contains a cycle");
        }
        let (parent, reason): (Option<String>, String) = conn
            .query_row(
                "SELECT parent_generation_id, reason FROM lar_conversation_generations
                 WHERE generation_id=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .with_context(|| format!("loading conversation generation {id}"))?;
        current = parent.clone();
        ancestry.push((id, parent, reason));
    }

    for (id, parent, reason) in ancestry.into_iter().rev() {
        let entry_ids = load_ordered_ids(
            conn,
            "lar_conversation_generation_entries",
            "generation_id",
            "entry_id",
            &id,
        )?;
        for entry_id in &entry_ids {
            load_archive_entry(conn, entry_id, entries)?;
        }
        let generation = Generation::new(GenerationData {
            parent_generation_id: parent.as_deref().map(parse_generation_id).transpose()?,
            entries: entry_ids
                .iter()
                .map(|value| parse_entry_id(value))
                .collect::<Result<Vec<_>>>()?,
            reason: parse_generation_reason(&reason)?,
        });
        if generation.id.to_string() != id {
            bail!("conversation generation catalog identity does not match its contents");
        }
        generations.push(generation);
    }
    Ok(())
}

fn load_archive_entry(
    conn: &Connection,
    entry_id: &str,
    loaded: &mut HashMap<String, ConversationEntry>,
) -> Result<()> {
    if loaded.contains_key(entry_id) {
        return Ok(());
    }
    let (semantic_schema, role, kind, name, tool_call_id): (
        i64,
        String,
        String,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    ) = conn
        .query_row(
            "SELECT semantic_schema, role, kind, name, tool_call_id
               FROM lar_conversation_entries WHERE entry_id=?1",
            [entry_id],
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
        .with_context(|| format!("loading conversation entry {entry_id}"))?;
    let mut statement = conn.prepare(
        "SELECT manifest_id, byte_offset, byte_length
           FROM lar_conversation_entry_ranges WHERE entry_id=?1 ORDER BY ordinal",
    )?;
    let ranges = statement
        .query_map([entry_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .map(|row| {
            let (manifest_id, byte_offset, byte_length) = row?;
            Ok(ArtifactRangeRef {
                manifest_id: ManifestId::from_str(&manifest_id)?,
                byte_offset: u64::try_from(byte_offset)
                    .context("negative conversation entry byte offset")?,
                byte_length: u64::try_from(byte_length)
                    .context("negative conversation entry byte length")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let semantic_schema =
        u16::try_from(semantic_schema).context("conversation semantic schema is outside u16")?;
    let entry = ConversationEntry::new(ConversationEntryData {
        semantic_schema,
        role: parse_conversation_role(&role)?,
        kind: parse_conversation_kind(&kind)?,
        raw_ranges: ranges,
        name,
        tool_call_id,
    });
    if entry.id.to_string() != entry_id {
        bail!("conversation entry catalog identity does not match its contents");
    }
    loaded.insert(entry_id.to_owned(), entry);
    Ok(())
}

fn parse_conversation_role(value: &str) -> Result<ConversationRole> {
    Ok(match value {
        "opaque" => ConversationRole::Opaque,
        "system" => ConversationRole::System,
        "user" => ConversationRole::User,
        "assistant" => ConversationRole::Assistant,
        "tool" => ConversationRole::Tool,
        value => ConversationRole::Unknown(parse_unknown_code(value, "conversation role")?),
    })
}

fn parse_conversation_kind(value: &str) -> Result<ConversationEntryKind> {
    Ok(match value {
        "opaque" => ConversationEntryKind::Opaque,
        "message" => ConversationEntryKind::Message,
        "tool_call" => ConversationEntryKind::ToolCall,
        "tool_result" => ConversationEntryKind::ToolResult,
        "summary" => ConversationEntryKind::Summary,
        value => {
            ConversationEntryKind::Unknown(parse_unknown_code(value, "conversation entry kind")?)
        }
    })
}

fn parse_generation_reason(value: &str) -> Result<GenerationReason> {
    Ok(match value {
        "initial" => GenerationReason::Initial,
        "append" => GenerationReason::Append,
        "compaction" => GenerationReason::Compaction,
        "branch" => GenerationReason::Branch,
        "mutation" => GenerationReason::Mutation,
        "import" => GenerationReason::Import,
        value => {
            GenerationReason::Unknown(parse_unknown_code(value, "conversation generation reason")?)
        }
    })
}

fn parse_unknown_code(value: &str, label: &str) -> Result<u16> {
    value
        .strip_prefix("unknown:")
        .with_context(|| format!("unknown {label} value {value:?}"))?
        .parse::<u16>()
        .with_context(|| format!("invalid {label} code {value:?}"))
}

fn reason_name(reason: GenerationReason) -> &'static str {
    match reason {
        GenerationReason::Initial => "initial",
        GenerationReason::Append => "append",
        GenerationReason::Compaction => "compaction",
        GenerationReason::Branch => "branch",
        GenerationReason::Mutation => "mutation",
        GenerationReason::Import => "import",
        GenerationReason::Unknown(_) => unreachable!("store does not emit unknown reasons"),
    }
}

fn parse_entry_id(value: &str) -> Result<ConversationEntryId> {
    Ok(ConversationEntryId(parse_id(value, "conversation entry")?))
}

fn parse_generation_id(value: &str) -> Result<GenerationId> {
    Ok(GenerationId(parse_id(value, "conversation generation")?))
}

fn parse_id(value: &str, label: &str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        bail!("{label} ID must contain exactly 64 hexadecimal characters");
    }
    let mut output = [0u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .with_context(|| format!("{label} ID contains non-hexadecimal characters"))?;
    }
    Ok(output)
}

fn to_i64(value: u64, label: &str) -> Result<i64> {
    i64::try_from(value).with_context(|| format!("{label} exceeds SQLite range"))
}
