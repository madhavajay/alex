use crate::{Error, Limits, ManifestId, Result};
use std::io::{Cursor, Read};

macro_rules! content_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(pub [u8; 32]);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                for byte in self.0 {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
        }
    };
}

content_id!(ConversationEntryId);
content_id!(GenerationId);
content_id!(TurnViewId);

pub const SEMANTIC_SCHEMA_RAW: u16 = 0;
pub const SEMANTIC_SCHEMA_V1: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactRangeRef {
    pub manifest_id: ManifestId,
    pub byte_offset: u64,
    pub byte_length: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationRole {
    Opaque,
    System,
    User,
    Assistant,
    Tool,
    Unknown(u16),
}

impl ConversationRole {
    fn code(self) -> u16 {
        match self {
            Self::Opaque => 0,
            Self::System => 1,
            Self::User => 2,
            Self::Assistant => 3,
            Self::Tool => 4,
            Self::Unknown(code) => code,
        }
    }

    fn from_code(code: u16) -> Self {
        match code {
            0 => Self::Opaque,
            1 => Self::System,
            2 => Self::User,
            3 => Self::Assistant,
            4 => Self::Tool,
            other => Self::Unknown(other),
        }
    }

    fn is_canonical(self) -> bool {
        !matches!(self, Self::Unknown(0..=4))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationEntryKind {
    Opaque,
    Message,
    ToolCall,
    ToolResult,
    Summary,
    Unknown(u16),
}

impl ConversationEntryKind {
    fn code(self) -> u16 {
        match self {
            Self::Opaque => 0,
            Self::Message => 1,
            Self::ToolCall => 2,
            Self::ToolResult => 3,
            Self::Summary => 4,
            Self::Unknown(code) => code,
        }
    }

    fn from_code(code: u16) -> Self {
        match code {
            0 => Self::Opaque,
            1 => Self::Message,
            2 => Self::ToolCall,
            3 => Self::ToolResult,
            4 => Self::Summary,
            other => Self::Unknown(other),
        }
    }

    fn is_canonical(self) -> bool {
        !matches!(self, Self::Unknown(0..=4))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationEntryData {
    /// Version of the normalized semantic interpretation, independent of the
    /// container record schema. Zero means raw-only/unparsed.
    pub semantic_schema: u16,
    pub role: ConversationRole,
    pub kind: ConversationEntryKind,
    pub raw_ranges: Vec<ArtifactRangeRef>,
    pub name: Option<Vec<u8>>,
    pub tool_call_id: Option<Vec<u8>>,
}

impl ConversationEntryData {
    pub fn raw_only(raw_ranges: Vec<ArtifactRangeRef>) -> Self {
        Self {
            semantic_schema: SEMANTIC_SCHEMA_RAW,
            role: ConversationRole::Opaque,
            kind: ConversationEntryKind::Opaque,
            raw_ranges,
            name: None,
            tool_call_id: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationEntry {
    pub id: ConversationEntryId,
    pub data: ConversationEntryData,
}

impl ConversationEntry {
    pub fn new(data: ConversationEntryData) -> Self {
        let mut value = Self {
            id: ConversationEntryId([0; 32]),
            data,
        };
        value.id = ConversationEntryId(*blake3::hash(&value.canonical_bytes()).as_bytes());
        value
    }

    pub(crate) fn validate_shape(&self, limits: &Limits) -> Result<()> {
        if !self.data.role.is_canonical() || !self.data.kind.is_canonical() {
            return Err(Error::Invalid("non-canonical conversation enum code"));
        }
        if self.data.raw_ranges.is_empty() {
            return Err(Error::Invalid(
                "conversation entry must reference raw artifact bytes",
            ));
        }
        check_count(
            "conversation artifact range count",
            self.data.raw_ranges.len(),
            limits.max_conversation_entry_ranges,
        )?;
        for range in &self.data.raw_ranges {
            if range.byte_length == 0 {
                return Err(Error::Invalid(
                    "conversation artifact ranges must not be empty",
                ));
            }
            range
                .byte_offset
                .checked_add(range.byte_length)
                .ok_or(Error::Invalid("conversation artifact range overflow"))?;
        }
        for field in [&self.data.name, &self.data.tool_call_id]
            .into_iter()
            .flatten()
        {
            check_length(
                "conversation semantic field",
                field.len(),
                limits.max_field_length,
            )?;
        }
        if self.data.semantic_schema == SEMANTIC_SCHEMA_RAW
            && (self.data.role != ConversationRole::Opaque
                || self.data.kind != ConversationEntryKind::Opaque
                || self.data.name.is_some()
                || self.data.tool_call_id.is_some())
        {
            return Err(Error::Invalid(
                "raw-only conversation entries cannot claim normalized semantics",
            ));
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_var_u64(&mut out, self.data.semantic_schema as u64);
        put_var_u64(&mut out, self.data.role.code() as u64);
        put_var_u64(&mut out, self.data.kind.code() as u64);
        put_var_u64(&mut out, self.data.raw_ranges.len() as u64);
        for range in &self.data.raw_ranges {
            out.extend_from_slice(&range.manifest_id.0);
            put_var_u64(&mut out, range.byte_offset);
            put_var_u64(&mut out, range.byte_length);
        }
        put_opt_bytes(&mut out, self.data.name.as_deref());
        put_opt_bytes(&mut out, self.data.tool_call_id.as_deref());
        out
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = self.id.0.to_vec();
        out.extend_from_slice(&self.canonical_bytes());
        out
    }

    pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let mut input = Cursor::new(bytes);
        let id = ConversationEntryId(read_array(&mut input)?);
        let semantic_schema = read_u16_var(&mut input, "conversation semantic schema")?;
        let role = ConversationRole::from_code(read_u16_var(&mut input, "conversation role")?);
        let kind =
            ConversationEntryKind::from_code(read_u16_var(&mut input, "conversation entry kind")?);
        let count = read_count(
            &mut input,
            "conversation artifact range count",
            limits.max_conversation_entry_ranges,
        )?;
        let mut raw_ranges = Vec::with_capacity(count);
        for _ in 0..count {
            raw_ranges.push(ArtifactRangeRef {
                manifest_id: ManifestId(read_array(&mut input)?),
                byte_offset: read_var_u64(&mut input)?,
                byte_length: read_var_u64(&mut input)?,
            });
        }
        let name = read_opt_bytes(&mut input, limits.max_field_length)?;
        let tool_call_id = read_opt_bytes(&mut input, limits.max_field_length)?;
        ensure_end(&input, bytes)?;
        let value = Self {
            id,
            data: ConversationEntryData {
                semantic_schema,
                role,
                kind,
                raw_ranges,
                name,
                tool_call_id,
            },
        };
        if ConversationEntryId(*blake3::hash(&value.canonical_bytes()).as_bytes()) != value.id {
            return Err(Error::Invalid(
                "conversation entry ID does not match contents",
            ));
        }
        value.validate_shape(limits)?;
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationReason {
    Initial,
    Append,
    Compaction,
    Branch,
    Mutation,
    Import,
    Unknown(u16),
}

impl GenerationReason {
    fn code(self) -> u16 {
        match self {
            Self::Initial => 1,
            Self::Append => 2,
            Self::Compaction => 3,
            Self::Branch => 4,
            Self::Mutation => 5,
            Self::Import => 6,
            Self::Unknown(code) => code,
        }
    }

    fn from_code(code: u16) -> Self {
        match code {
            1 => Self::Initial,
            2 => Self::Append,
            3 => Self::Compaction,
            4 => Self::Branch,
            5 => Self::Mutation,
            6 => Self::Import,
            other => Self::Unknown(other),
        }
    }

    fn is_canonical(self) -> bool {
        !matches!(self, Self::Unknown(1..=6))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationData {
    pub parent_generation_id: Option<GenerationId>,
    pub entries: Vec<ConversationEntryId>,
    pub reason: GenerationReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Generation {
    pub id: GenerationId,
    pub data: GenerationData,
}

impl Generation {
    pub fn new(data: GenerationData) -> Self {
        let mut value = Self {
            id: GenerationId([0; 32]),
            data,
        };
        value.id = GenerationId(*blake3::hash(&value.canonical_bytes()).as_bytes());
        value
    }

    pub(crate) fn validate_shape(&self, limits: &Limits) -> Result<()> {
        if !self.data.reason.is_canonical() {
            return Err(Error::Invalid("non-canonical generation reason code"));
        }
        check_count(
            "generation entry count",
            self.data.entries.len(),
            limits.max_generation_entries,
        )?;
        if self.data.reason == GenerationReason::Initial && self.data.parent_generation_id.is_some()
        {
            return Err(Error::Invalid("initial generation must not have a parent"));
        }
        if matches!(
            self.data.reason,
            GenerationReason::Append
                | GenerationReason::Compaction
                | GenerationReason::Branch
                | GenerationReason::Mutation
        ) && self.data.parent_generation_id.is_none()
        {
            return Err(Error::Invalid("derived generation is missing its parent"));
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_opt_id(&mut out, self.data.parent_generation_id.map(|id| id.0));
        put_var_u64(&mut out, self.data.reason.code() as u64);
        put_var_u64(&mut out, self.data.entries.len() as u64);
        for entry in &self.data.entries {
            out.extend_from_slice(&entry.0);
        }
        out
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = self.id.0.to_vec();
        out.extend_from_slice(&self.canonical_bytes());
        out
    }

    pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let mut input = Cursor::new(bytes);
        let id = GenerationId(read_array(&mut input)?);
        let parent_generation_id = read_opt_id(&mut input)?.map(GenerationId);
        let reason = GenerationReason::from_code(read_u16_var(&mut input, "generation reason")?);
        let count = read_count(
            &mut input,
            "generation entry count",
            limits.max_generation_entries,
        )?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(ConversationEntryId(read_array(&mut input)?));
        }
        ensure_end(&input, bytes)?;
        let value = Self {
            id,
            data: GenerationData {
                parent_generation_id,
                entries,
                reason,
            },
        };
        if GenerationId(*blake3::hash(&value.canonical_bytes()).as_bytes()) != value.id {
            return Err(Error::Invalid("generation ID does not match contents"));
        }
        value.validate_shape(limits)?;
        Ok(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnViewData {
    pub trace_id: Vec<u8>,
    pub generation_id: GenerationId,
    /// Inclusive index of the final generation entry sent in the request.
    pub upto_index: u64,
    pub response_entry_refs: Vec<ConversationEntryId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnView {
    pub id: TurnViewId,
    pub data: TurnViewData,
}

impl TurnView {
    pub fn new(data: TurnViewData) -> Self {
        let mut value = Self {
            id: TurnViewId([0; 32]),
            data,
        };
        value.id = TurnViewId(*blake3::hash(&value.canonical_bytes()).as_bytes());
        value
    }

    pub(crate) fn validate_shape(&self, limits: &Limits) -> Result<()> {
        if self.data.trace_id.is_empty() {
            return Err(Error::Invalid("turn view trace ID must not be empty"));
        }
        check_length(
            "turn view trace ID",
            self.data.trace_id.len(),
            limits.max_identifier_length,
        )?;
        check_count(
            "turn response entry count",
            self.data.response_entry_refs.len(),
            limits.max_turn_response_entries,
        )?;
        Ok(())
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, &self.data.trace_id);
        out.extend_from_slice(&self.data.generation_id.0);
        put_var_u64(&mut out, self.data.upto_index);
        put_var_u64(&mut out, self.data.response_entry_refs.len() as u64);
        for entry in &self.data.response_entry_refs {
            out.extend_from_slice(&entry.0);
        }
        out
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = self.id.0.to_vec();
        out.extend_from_slice(&self.canonical_bytes());
        out
    }

    pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let mut input = Cursor::new(bytes);
        let id = TurnViewId(read_array(&mut input)?);
        let trace_id = read_bytes(&mut input, limits.max_identifier_length)?;
        let generation_id = GenerationId(read_array(&mut input)?);
        let upto_index = read_var_u64(&mut input)?;
        let count = read_count(
            &mut input,
            "turn response entry count",
            limits.max_turn_response_entries,
        )?;
        let mut response_entry_refs = Vec::with_capacity(count);
        for _ in 0..count {
            response_entry_refs.push(ConversationEntryId(read_array(&mut input)?));
        }
        ensure_end(&input, bytes)?;
        let value = Self {
            id,
            data: TurnViewData {
                trace_id,
                generation_id,
                upto_index,
                response_entry_refs,
            },
        };
        if TurnViewId(*blake3::hash(&value.canonical_bytes()).as_bytes()) != value.id {
            return Err(Error::Invalid("turn view ID does not match contents"));
        }
        value.validate_shape(limits)?;
        Ok(value)
    }
}

fn check_count(what: &'static str, actual: usize, limit: u32) -> Result<()> {
    if actual as u64 > limit as u64 {
        return Err(Error::Limit {
            what,
            actual: actual as u64,
            limit: limit as u64,
        });
    }
    Ok(())
}

fn check_length(what: &'static str, actual: usize, limit: u32) -> Result<()> {
    if actual as u64 > limit as u64 {
        return Err(Error::Limit {
            what,
            actual: actual as u64,
            limit: limit as u64,
        });
    }
    Ok(())
}

fn put_var_u64(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_var_u64(input: &mut Cursor<&[u8]>) -> Result<u64> {
    let mut value = 0u64;
    for index in 0..10 {
        let byte = read_tag(input)?;
        if index == 9 && byte > 1 {
            return Err(Error::Invalid("varint overflow"));
        }
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            if index > 0 && byte == 0 {
                return Err(Error::Invalid("non-canonical varint"));
            }
            return Ok(value);
        }
    }
    Err(Error::Invalid("varint overflow"))
}

fn read_u16_var(input: &mut Cursor<&[u8]>, what: &'static str) -> Result<u16> {
    u16::try_from(read_var_u64(input)?).map_err(|_| Error::Invalid(what))
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_var_u64(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn read_bytes(input: &mut Cursor<&[u8]>, max: u32) -> Result<Vec<u8>> {
    let length = read_var_u64(input)?;
    if length > max as u64 {
        return Err(Error::Limit {
            what: "conversation field length",
            actual: length,
            limit: max as u64,
        });
    }
    let mut value = vec![0; length as usize];
    input
        .read_exact(&mut value)
        .map_err(|_| Error::Invalid("truncated conversation payload"))?;
    Ok(value)
}

fn put_opt_bytes(out: &mut Vec<u8>, value: Option<&[u8]>) {
    match value {
        None => out.push(0),
        Some(value) => {
            out.push(1);
            put_bytes(out, value);
        }
    }
}

fn read_opt_bytes(input: &mut Cursor<&[u8]>, max: u32) -> Result<Option<Vec<u8>>> {
    match read_tag(input)? {
        0 => Ok(None),
        1 => Ok(Some(read_bytes(input, max)?)),
        _ => Err(Error::Invalid("invalid optional field tag")),
    }
}

fn put_opt_id(out: &mut Vec<u8>, value: Option<[u8; 32]>) {
    match value {
        None => out.push(0),
        Some(value) => {
            out.push(1);
            out.extend_from_slice(&value);
        }
    }
}

fn read_opt_id(input: &mut Cursor<&[u8]>) -> Result<Option<[u8; 32]>> {
    match read_tag(input)? {
        0 => Ok(None),
        1 => Ok(Some(read_array(input)?)),
        _ => Err(Error::Invalid("invalid optional field tag")),
    }
}

fn read_count(input: &mut Cursor<&[u8]>, what: &'static str, max: u32) -> Result<usize> {
    let count = read_var_u64(input)?;
    if count > max as u64 {
        return Err(Error::Limit {
            what,
            actual: count,
            limit: max as u64,
        });
    }
    Ok(count as usize)
}

fn read_tag(input: &mut Cursor<&[u8]>) -> Result<u8> {
    let mut byte = [0; 1];
    input
        .read_exact(&mut byte)
        .map_err(|_| Error::Invalid("truncated conversation payload"))?;
    Ok(byte[0])
}

fn read_array<const N: usize>(input: &mut Cursor<&[u8]>) -> Result<[u8; N]> {
    let mut value = [0; N];
    input
        .read_exact(&mut value)
        .map_err(|_| Error::Invalid("truncated conversation payload"))?;
    Ok(value)
}

fn ensure_end(input: &Cursor<&[u8]>, bytes: &[u8]) -> Result<()> {
    if input.position() != bytes.len() as u64 {
        return Err(Error::Invalid("trailing conversation payload bytes"));
    }
    Ok(())
}
