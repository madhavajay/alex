use crate::{Error, HeaderBlockId, Limits, ManifestId, Result};
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

content_id!(StreamIndexId);
content_id!(StageId);
content_id!(ExchangeId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamRead {
    pub byte_offset: u64,
    pub byte_length: u64,
    pub delta_from_first_byte_ns: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamParser {
    Opaque,
    Sse,
    Ndjson,
    Unknown(u16),
}

impl StreamParser {
    fn code(self) -> u16 {
        match self {
            Self::Opaque => 0,
            Self::Sse => 1,
            Self::Ndjson => 2,
            Self::Unknown(code) => code,
        }
    }

    fn from_code(code: u16) -> Self {
        match code {
            0 => Self::Opaque,
            1 => Self::Sse,
            2 => Self::Ndjson,
            other => Self::Unknown(other),
        }
    }

    fn is_canonical(self) -> bool {
        !matches!(self, Self::Unknown(0..=2))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamFrameKind {
    Opaque,
    SseEvent,
    NdjsonRecord,
    Unknown(u16),
}

impl StreamFrameKind {
    fn code(self) -> u16 {
        match self {
            Self::Opaque => 0,
            Self::SseEvent => 1,
            Self::NdjsonRecord => 2,
            Self::Unknown(code) => code,
        }
    }

    fn from_code(code: u16) -> Self {
        match code {
            0 => Self::Opaque,
            1 => Self::SseEvent,
            2 => Self::NdjsonRecord,
            other => Self::Unknown(other),
        }
    }

    fn is_canonical(self) -> bool {
        !matches!(self, Self::Unknown(0..=2))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedFrame {
    pub byte_offset: u64,
    pub byte_length: u64,
    pub delta_from_first_byte_ns: u64,
    pub parser: StreamParser,
    pub frame_kind: StreamFrameKind,
}

/// Timing and parse metadata for one raw streamed body. The record never owns
/// stream bytes: every range addresses `raw_body_manifest_id`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamIndex {
    pub id: StreamIndexId,
    pub raw_body_manifest_id: ManifestId,
    pub reads: Vec<StreamRead>,
    pub frames: Vec<ParsedFrame>,
}

impl StreamIndex {
    pub fn new(
        raw_body_manifest_id: ManifestId,
        reads: Vec<StreamRead>,
        frames: Vec<ParsedFrame>,
    ) -> Self {
        let mut value = Self {
            id: StreamIndexId([0; 32]),
            raw_body_manifest_id,
            reads,
            frames,
        };
        value.id = StreamIndexId(*blake3::hash(&value.canonical_bytes()).as_bytes());
        value
    }

    pub(crate) fn validate(&self, body_length: u64, limits: &Limits) -> Result<()> {
        check_count(
            "stream read count",
            self.reads.len(),
            limits.max_stream_reads,
        )?;
        check_count(
            "parsed stream frame count",
            self.frames.len(),
            limits.max_stream_frames,
        )?;

        let mut expected_offset = 0u64;
        let mut previous_delta = 0u64;
        for (index, read) in self.reads.iter().enumerate() {
            if read.byte_length == 0 {
                return Err(Error::Invalid("stream reads must not be empty"));
            }
            if read.byte_offset != expected_offset {
                return Err(Error::Invalid(
                    "stream reads must be contiguous and start at zero",
                ));
            }
            if index > 0 && read.delta_from_first_byte_ns < previous_delta {
                return Err(Error::Invalid("stream read timing must be monotonic"));
            }
            expected_offset = expected_offset
                .checked_add(read.byte_length)
                .ok_or(Error::Invalid("stream read range overflow"))?;
            previous_delta = read.delta_from_first_byte_ns;
        }
        if expected_offset != body_length {
            return Err(Error::Invalid(
                "stream reads must cover the referenced raw body",
            ));
        }

        let mut previous_end = 0u64;
        let mut previous_delta = 0u64;
        for (index, frame) in self.frames.iter().enumerate() {
            if !frame.parser.is_canonical() || !frame.frame_kind.is_canonical() {
                return Err(Error::Invalid("non-canonical stream enum code"));
            }
            if frame.byte_length == 0 {
                return Err(Error::Invalid("parsed stream frames must not be empty"));
            }
            if frame.byte_offset < previous_end {
                return Err(Error::Invalid(
                    "parsed stream frames must be ordered and non-overlapping",
                ));
            }
            let end = frame
                .byte_offset
                .checked_add(frame.byte_length)
                .ok_or(Error::Invalid("parsed stream frame range overflow"))?;
            if end > body_length {
                return Err(Error::Invalid(
                    "parsed stream frame exceeds the referenced raw body",
                ));
            }
            if index > 0 && frame.delta_from_first_byte_ns < previous_delta {
                return Err(Error::Invalid(
                    "parsed stream frame timing must be monotonic",
                ));
            }
            previous_end = end;
            previous_delta = frame.delta_from_first_byte_ns;
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = self.raw_body_manifest_id.0.to_vec();
        put_var_u64(&mut out, self.reads.len() as u64);
        let mut previous_offset = 0u64;
        let mut previous_delta = 0u64;
        for read in &self.reads {
            put_var_u64(&mut out, read.byte_offset.saturating_sub(previous_offset));
            put_var_u64(&mut out, read.byte_length);
            put_var_u64(
                &mut out,
                read.delta_from_first_byte_ns.saturating_sub(previous_delta),
            );
            previous_offset = read.byte_offset;
            previous_delta = read.delta_from_first_byte_ns;
        }
        put_var_u64(&mut out, self.frames.len() as u64);
        previous_offset = 0;
        previous_delta = 0;
        for frame in &self.frames {
            put_var_u64(&mut out, frame.byte_offset.saturating_sub(previous_offset));
            put_var_u64(&mut out, frame.byte_length);
            put_var_u64(
                &mut out,
                frame
                    .delta_from_first_byte_ns
                    .saturating_sub(previous_delta),
            );
            put_var_u64(&mut out, frame.parser.code() as u64);
            put_var_u64(&mut out, frame.frame_kind.code() as u64);
            previous_offset = frame.byte_offset;
            previous_delta = frame.delta_from_first_byte_ns;
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
        let id = StreamIndexId(read_array(&mut input)?);
        let raw_body_manifest_id = ManifestId(read_array(&mut input)?);
        let reads_len = read_count(&mut input, "stream read count", limits.max_stream_reads)?;
        let mut reads = Vec::with_capacity(reads_len);
        let mut previous_offset = 0u64;
        let mut previous_delta = 0u64;
        for _ in 0..reads_len {
            let byte_offset = checked_delta(&mut input, previous_offset, "stream read offset")?;
            let byte_length = read_var_u64(&mut input)?;
            let delta_from_first_byte_ns =
                checked_delta(&mut input, previous_delta, "stream read time")?;
            reads.push(StreamRead {
                byte_offset,
                byte_length,
                delta_from_first_byte_ns,
            });
            previous_offset = byte_offset;
            previous_delta = delta_from_first_byte_ns;
        }
        let frame_count = read_count(
            &mut input,
            "parsed stream frame count",
            limits.max_stream_frames,
        )?;
        let mut frames = Vec::with_capacity(frame_count);
        previous_offset = 0;
        previous_delta = 0;
        for _ in 0..frame_count {
            let byte_offset = checked_delta(&mut input, previous_offset, "stream frame offset")?;
            let byte_length = read_var_u64(&mut input)?;
            let delta_from_first_byte_ns =
                checked_delta(&mut input, previous_delta, "stream frame time")?;
            let parser = StreamParser::from_code(read_u16_var(&mut input, "stream parser")?);
            let frame_kind =
                StreamFrameKind::from_code(read_u16_var(&mut input, "stream frame kind")?);
            frames.push(ParsedFrame {
                byte_offset,
                byte_length,
                delta_from_first_byte_ns,
                parser,
                frame_kind,
            });
            previous_offset = byte_offset;
            previous_delta = delta_from_first_byte_ns;
        }
        ensure_end(&input, bytes)?;
        let value = Self {
            id,
            raw_body_manifest_id,
            reads,
            frames,
        };
        if StreamIndexId(*blake3::hash(&value.canonical_bytes()).as_bytes()) != value.id {
            return Err(Error::Invalid("stream index ID does not match contents"));
        }
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageKind {
    ClientRequest,
    NormalizedRequest,
    RouterDecision,
    RetryDecision,
    FailoverDecision,
    UpstreamRequest,
    UpstreamResponse,
    UpstreamFailure,
    ClientResponse,
    ClientTrailers,
    ToolCall,
    ToolResult,
    AuthRefresh,
    AccountRouting,
    DarioRequest,
    DarioResponse,
    InjectedResponse,
    Cancellation,
    Unknown(u16),
}

impl StageKind {
    fn code(self) -> u16 {
        match self {
            Self::ClientRequest => 1,
            Self::NormalizedRequest => 2,
            Self::RouterDecision => 3,
            Self::RetryDecision => 4,
            Self::FailoverDecision => 5,
            Self::UpstreamRequest => 6,
            Self::UpstreamResponse => 7,
            Self::UpstreamFailure => 8,
            Self::ClientResponse => 9,
            Self::ClientTrailers => 10,
            Self::ToolCall => 11,
            Self::ToolResult => 12,
            Self::AuthRefresh => 13,
            Self::AccountRouting => 14,
            Self::DarioRequest => 15,
            Self::DarioResponse => 16,
            Self::InjectedResponse => 17,
            Self::Cancellation => 18,
            Self::Unknown(code) => code,
        }
    }

    fn from_code(code: u16) -> Self {
        match code {
            1 => Self::ClientRequest,
            2 => Self::NormalizedRequest,
            3 => Self::RouterDecision,
            4 => Self::RetryDecision,
            5 => Self::FailoverDecision,
            6 => Self::UpstreamRequest,
            7 => Self::UpstreamResponse,
            8 => Self::UpstreamFailure,
            9 => Self::ClientResponse,
            10 => Self::ClientTrailers,
            11 => Self::ToolCall,
            12 => Self::ToolResult,
            13 => Self::AuthRefresh,
            14 => Self::AccountRouting,
            15 => Self::DarioRequest,
            16 => Self::DarioResponse,
            17 => Self::InjectedResponse,
            18 => Self::Cancellation,
            other => Self::Unknown(other),
        }
    }

    fn requires_attempt(self) -> bool {
        matches!(
            self,
            Self::UpstreamRequest | Self::UpstreamResponse | Self::UpstreamFailure
        )
    }

    fn is_canonical(self) -> bool {
        !matches!(self, Self::Unknown(1..=18))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageData {
    pub kind: StageKind,
    pub attempt_number: Option<u32>,
    pub wall_time_ns: u64,
    pub monotonic_delta_ns: Option<u64>,
    pub first_byte_delta_ns: Option<u64>,
    pub last_byte_delta_ns: Option<u64>,
    pub request_headers_ref: Option<HeaderBlockId>,
    pub request_body_manifest_ref: Option<ManifestId>,
    pub response_headers_ref: Option<HeaderBlockId>,
    pub response_body_manifest_ref: Option<ManifestId>,
    pub trailers_ref: Option<HeaderBlockId>,
    pub stream_index_ref: Option<StreamIndexId>,
    pub provider: Option<Vec<u8>>,
    pub requested_model: Option<Vec<u8>>,
    pub routed_model: Option<Vec<u8>>,
    pub account_id: Option<Vec<u8>>,
    pub routing_reason: Option<Vec<u8>>,
    pub status_code: Option<u16>,
    pub usage: Option<TokenUsage>,
    pub cost_nanos: Option<u64>,
    pub cost_currency: Option<Vec<u8>>,
    pub error_class: Option<Vec<u8>>,
    pub error_message: Option<Vec<u8>>,
}

impl StageData {
    pub fn new(kind: StageKind, wall_time_ns: u64) -> Self {
        Self {
            kind,
            attempt_number: None,
            wall_time_ns,
            monotonic_delta_ns: None,
            first_byte_delta_ns: None,
            last_byte_delta_ns: None,
            request_headers_ref: None,
            request_body_manifest_ref: None,
            response_headers_ref: None,
            response_body_manifest_ref: None,
            trailers_ref: None,
            stream_index_ref: None,
            provider: None,
            requested_model: None,
            routed_model: None,
            account_id: None,
            routing_reason: None,
            status_code: None,
            usage: None,
            cost_nanos: None,
            cost_currency: None,
            error_class: None,
            error_message: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage {
    pub id: StageId,
    pub data: StageData,
}

impl Stage {
    pub fn new(data: StageData) -> Self {
        let mut value = Self {
            id: StageId([0; 32]),
            data,
        };
        value.id = StageId(*blake3::hash(&value.canonical_bytes()).as_bytes());
        value
    }

    pub(crate) fn validate_shape(&self, limits: &Limits) -> Result<()> {
        if !self.data.kind.is_canonical() {
            return Err(Error::Invalid("non-canonical stage kind code"));
        }
        if self.data.kind.requires_attempt() && self.data.attempt_number.is_none() {
            return Err(Error::Invalid("upstream stage is missing attempt number"));
        }
        if let (Some(first), Some(last)) =
            (self.data.first_byte_delta_ns, self.data.last_byte_delta_ns)
        {
            if last < first {
                return Err(Error::Invalid("last-byte time precedes first-byte time"));
            }
        }
        for field in [
            &self.data.provider,
            &self.data.requested_model,
            &self.data.routed_model,
            &self.data.account_id,
            &self.data.routing_reason,
            &self.data.cost_currency,
            &self.data.error_class,
            &self.data.error_message,
        ] {
            let Some(value) = field else { continue };
            check_length("stage field", value.len(), limits.max_field_length)?;
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_var_u64(&mut out, self.data.kind.code() as u64);
        put_opt_u32(&mut out, self.data.attempt_number);
        put_var_u64(&mut out, self.data.wall_time_ns);
        put_opt_u64(&mut out, self.data.monotonic_delta_ns);
        put_opt_u64(&mut out, self.data.first_byte_delta_ns);
        put_opt_u64(&mut out, self.data.last_byte_delta_ns);
        put_opt_id(&mut out, self.data.request_headers_ref.map(|id| id.0));
        put_opt_id(&mut out, self.data.request_body_manifest_ref.map(|id| id.0));
        put_opt_id(&mut out, self.data.response_headers_ref.map(|id| id.0));
        put_opt_id(
            &mut out,
            self.data.response_body_manifest_ref.map(|id| id.0),
        );
        put_opt_id(&mut out, self.data.trailers_ref.map(|id| id.0));
        put_opt_id(&mut out, self.data.stream_index_ref.map(|id| id.0));
        put_opt_bytes(&mut out, self.data.provider.as_deref());
        put_opt_bytes(&mut out, self.data.requested_model.as_deref());
        put_opt_bytes(&mut out, self.data.routed_model.as_deref());
        put_opt_bytes(&mut out, self.data.account_id.as_deref());
        put_opt_bytes(&mut out, self.data.routing_reason.as_deref());
        put_opt_u16(&mut out, self.data.status_code);
        match &self.data.usage {
            None => out.push(0),
            Some(usage) => {
                out.push(1);
                put_var_u64(&mut out, usage.input_tokens);
                put_var_u64(&mut out, usage.output_tokens);
                put_var_u64(&mut out, usage.cached_tokens);
                put_var_u64(&mut out, usage.reasoning_tokens);
            }
        }
        put_opt_u64(&mut out, self.data.cost_nanos);
        put_opt_bytes(&mut out, self.data.cost_currency.as_deref());
        put_opt_bytes(&mut out, self.data.error_class.as_deref());
        put_opt_bytes(&mut out, self.data.error_message.as_deref());
        out
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = self.id.0.to_vec();
        out.extend_from_slice(&self.canonical_bytes());
        out
    }

    pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let mut input = Cursor::new(bytes);
        let id = StageId(read_array(&mut input)?);
        let kind = StageKind::from_code(read_u16_var(&mut input, "stage kind")?);
        let attempt_number = read_opt_u32(&mut input)?;
        let wall_time_ns = read_var_u64(&mut input)?;
        let monotonic_delta_ns = read_opt_u64(&mut input)?;
        let first_byte_delta_ns = read_opt_u64(&mut input)?;
        let last_byte_delta_ns = read_opt_u64(&mut input)?;
        let request_headers_ref = read_opt_id(&mut input)?.map(HeaderBlockId);
        let request_body_manifest_ref = read_opt_id(&mut input)?.map(ManifestId);
        let response_headers_ref = read_opt_id(&mut input)?.map(HeaderBlockId);
        let response_body_manifest_ref = read_opt_id(&mut input)?.map(ManifestId);
        let trailers_ref = read_opt_id(&mut input)?.map(HeaderBlockId);
        let stream_index_ref = read_opt_id(&mut input)?.map(StreamIndexId);
        let provider = read_opt_bytes(&mut input, limits.max_field_length)?;
        let requested_model = read_opt_bytes(&mut input, limits.max_field_length)?;
        let routed_model = read_opt_bytes(&mut input, limits.max_field_length)?;
        let account_id = read_opt_bytes(&mut input, limits.max_field_length)?;
        let routing_reason = read_opt_bytes(&mut input, limits.max_field_length)?;
        let status_code = read_opt_u16(&mut input)?;
        let usage = match read_tag(&mut input)? {
            0 => None,
            1 => Some(TokenUsage {
                input_tokens: read_var_u64(&mut input)?,
                output_tokens: read_var_u64(&mut input)?,
                cached_tokens: read_var_u64(&mut input)?,
                reasoning_tokens: read_var_u64(&mut input)?,
            }),
            _ => return Err(Error::Invalid("invalid optional field tag")),
        };
        let cost_nanos = read_opt_u64(&mut input)?;
        let cost_currency = read_opt_bytes(&mut input, limits.max_field_length)?;
        let error_class = read_opt_bytes(&mut input, limits.max_field_length)?;
        let error_message = read_opt_bytes(&mut input, limits.max_field_length)?;
        ensure_end(&input, bytes)?;
        let value = Self {
            id,
            data: StageData {
                kind,
                attempt_number,
                wall_time_ns,
                monotonic_delta_ns,
                first_byte_delta_ns,
                last_byte_delta_ns,
                request_headers_ref,
                request_body_manifest_ref,
                response_headers_ref,
                response_body_manifest_ref,
                trailers_ref,
                stream_index_ref,
                provider,
                requested_model,
                routed_model,
                account_id,
                routing_reason,
                status_code,
                usage,
                cost_nanos,
                cost_currency,
                error_class,
                error_message,
            },
        };
        if StageId(*blake3::hash(&value.canonical_bytes()).as_bytes()) != value.id {
            return Err(Error::Invalid("stage ID does not match contents"));
        }
        value.validate_shape(limits)?;
        Ok(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExchangeData {
    pub trace_id: Vec<u8>,
    pub session_id: Option<Vec<u8>>,
    pub run_id: Option<Vec<u8>>,
    pub parent_trace_id: Option<Vec<u8>>,
    pub capture_sequence: u64,
    pub wall_time_ns: u64,
    pub monotonic_delta_ns: Option<u64>,
    pub clock_id: Option<Vec<u8>>,
    pub stages: Vec<StageId>,
}

impl ExchangeData {
    pub fn new(
        trace_id: impl Into<Vec<u8>>,
        capture_sequence: u64,
        wall_time_ns: u64,
        stages: Vec<StageId>,
    ) -> Self {
        Self {
            trace_id: trace_id.into(),
            session_id: None,
            run_id: None,
            parent_trace_id: None,
            capture_sequence,
            wall_time_ns,
            monotonic_delta_ns: None,
            clock_id: None,
            stages,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Exchange {
    pub id: ExchangeId,
    pub data: ExchangeData,
}

impl Exchange {
    pub fn new(data: ExchangeData) -> Self {
        let mut value = Self {
            id: ExchangeId([0; 32]),
            data,
        };
        value.id = ExchangeId(*blake3::hash(&value.canonical_bytes()).as_bytes());
        value
    }

    pub(crate) fn validate_shape(&self, limits: &Limits) -> Result<()> {
        if self.data.trace_id.is_empty() {
            return Err(Error::Invalid("exchange trace ID must not be empty"));
        }
        for id in [
            Some(&self.data.trace_id),
            self.data.session_id.as_ref(),
            self.data.run_id.as_ref(),
            self.data.parent_trace_id.as_ref(),
            self.data.clock_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            check_length(
                "exchange identifier",
                id.len(),
                limits.max_identifier_length,
            )?;
        }
        check_count(
            "exchange stage count",
            self.data.stages.len(),
            limits.max_exchange_stages,
        )?;
        Ok(())
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, &self.data.trace_id);
        put_opt_bytes(&mut out, self.data.session_id.as_deref());
        put_opt_bytes(&mut out, self.data.run_id.as_deref());
        put_opt_bytes(&mut out, self.data.parent_trace_id.as_deref());
        put_var_u64(&mut out, self.data.capture_sequence);
        put_var_u64(&mut out, self.data.wall_time_ns);
        put_opt_u64(&mut out, self.data.monotonic_delta_ns);
        put_opt_bytes(&mut out, self.data.clock_id.as_deref());
        put_var_u64(&mut out, self.data.stages.len() as u64);
        for stage in &self.data.stages {
            out.extend_from_slice(&stage.0);
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
        let id = ExchangeId(read_array(&mut input)?);
        let trace_id = read_bytes(&mut input, limits.max_identifier_length)?;
        let session_id = read_opt_bytes(&mut input, limits.max_identifier_length)?;
        let run_id = read_opt_bytes(&mut input, limits.max_identifier_length)?;
        let parent_trace_id = read_opt_bytes(&mut input, limits.max_identifier_length)?;
        let capture_sequence = read_var_u64(&mut input)?;
        let wall_time_ns = read_var_u64(&mut input)?;
        let monotonic_delta_ns = read_opt_u64(&mut input)?;
        let clock_id = read_opt_bytes(&mut input, limits.max_identifier_length)?;
        let stage_count = read_count(
            &mut input,
            "exchange stage count",
            limits.max_exchange_stages,
        )?;
        let mut stages = Vec::with_capacity(stage_count);
        for _ in 0..stage_count {
            stages.push(StageId(read_array(&mut input)?));
        }
        ensure_end(&input, bytes)?;
        let value = Self {
            id,
            data: ExchangeData {
                trace_id,
                session_id,
                run_id,
                parent_trace_id,
                capture_sequence,
                wall_time_ns,
                monotonic_delta_ns,
                clock_id,
                stages,
            },
        };
        if ExchangeId(*blake3::hash(&value.canonical_bytes()).as_bytes()) != value.id {
            return Err(Error::Invalid("exchange ID does not match contents"));
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

fn checked_delta(input: &mut Cursor<&[u8]>, base: u64, what: &'static str) -> Result<u64> {
    base.checked_add(read_var_u64(input)?)
        .ok_or(Error::Invalid(what))
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_var_u64(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn read_bytes(input: &mut Cursor<&[u8]>, max: u32) -> Result<Vec<u8>> {
    let length = read_var_u64(input)?;
    if length > max as u64 {
        return Err(Error::Limit {
            what: "event field length",
            actual: length,
            limit: max as u64,
        });
    }
    let mut value = vec![0; length as usize];
    input
        .read_exact(&mut value)
        .map_err(|_| Error::Invalid("truncated record payload"))?;
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

fn put_opt_u64(out: &mut Vec<u8>, value: Option<u64>) {
    match value {
        None => out.push(0),
        Some(value) => {
            out.push(1);
            put_var_u64(out, value);
        }
    }
}

fn read_opt_u64(input: &mut Cursor<&[u8]>) -> Result<Option<u64>> {
    match read_tag(input)? {
        0 => Ok(None),
        1 => Ok(Some(read_var_u64(input)?)),
        _ => Err(Error::Invalid("invalid optional field tag")),
    }
}

fn put_opt_u32(out: &mut Vec<u8>, value: Option<u32>) {
    put_opt_u64(out, value.map(u64::from));
}

fn read_opt_u32(input: &mut Cursor<&[u8]>) -> Result<Option<u32>> {
    read_opt_u64(input)?
        .map(|value| u32::try_from(value).map_err(|_| Error::Invalid("u32 field overflow")))
        .transpose()
}

fn put_opt_u16(out: &mut Vec<u8>, value: Option<u16>) {
    put_opt_u64(out, value.map(u64::from));
}

fn read_opt_u16(input: &mut Cursor<&[u8]>) -> Result<Option<u16>> {
    read_opt_u64(input)?
        .map(|value| u16::try_from(value).map_err(|_| Error::Invalid("u16 field overflow")))
        .transpose()
}

fn read_u16_var(input: &mut Cursor<&[u8]>, what: &'static str) -> Result<u16> {
    let value = read_var_u64(input)?;
    u16::try_from(value).map_err(|_| Error::Invalid(what))
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
    usize::try_from(count).map_err(|_| Error::Limit {
        what,
        actual: count,
        limit: usize::MAX as u64,
    })
}

fn read_tag(input: &mut Cursor<&[u8]>) -> Result<u8> {
    let mut byte = [0u8; 1];
    input
        .read_exact(&mut byte)
        .map_err(|_| Error::Invalid("truncated record payload"))?;
    Ok(byte[0])
}

fn read_array<const N: usize>(input: &mut Cursor<&[u8]>) -> Result<[u8; N]> {
    let mut value = [0u8; N];
    input
        .read_exact(&mut value)
        .map_err(|_| Error::Invalid("truncated record payload"))?;
    Ok(value)
}

fn ensure_end(input: &Cursor<&[u8]>, bytes: &[u8]) -> Result<()> {
    if input.position() != bytes.len() as u64 {
        return Err(Error::Invalid("trailing record payload bytes"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_overlong_varints() {
        assert!(matches!(
            read_var_u64(&mut Cursor::new(&[0x80, 0x00][..])),
            Err(Error::Invalid("non-canonical varint"))
        ));
    }
}
