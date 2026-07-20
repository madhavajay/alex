//! Core primitives for the LAR (LLM Archive) format.
//!
//! The crate deliberately has no dependency on Alex's proxy, database, or UI.

mod archive;
mod chunker;
mod conversation;
mod event;
mod exchange_metadata;
mod format;
mod index;
mod model;
mod page;
mod range;
mod replay;
mod search;

pub use archive::{
    read_chunk_record_at, upgrade_archive, verify_upgraded_archive, ArchiveReader,
    ArchiveUpgradeReport, ArchiveWriter, CheckpointRecordDescriptor, ChunkRecordDescriptor,
    OpenPath, RecoveryStatus,
};
pub use chunker::{ChunkerConfig, StreamingChunker};
pub use conversation::{
    ArtifactRangeRef, ConversationEntry, ConversationEntryData, ConversationEntryId,
    ConversationEntryKind, ConversationRole, Generation, GenerationData, GenerationId,
    GenerationReason, TurnView, TurnViewData, TurnViewId, SEMANTIC_SCHEMA_RAW, SEMANTIC_SCHEMA_V1,
};
pub use event::{
    Exchange, ExchangeData, ExchangeId, ParsedFrame, Stage, StageData, StageId, StageKind,
    StreamFrameKind, StreamIndex, StreamIndexId, StreamParser, StreamRead, TokenUsage,
};
pub use exchange_metadata::{
    ExchangeMetadata, ExchangeMetadataData, UnknownExchangeMetadataAttribute,
};
pub use format::{
    read_file_header, write_file_header, DictionaryDescriptor, FileHeader, FileRole, FrameRead,
    FrameReader, HashAlgorithm, Limits, RecordFrame, RecordType, DEFAULT_CONTAINER_MAJOR,
    DEFAULT_CONTAINER_MINOR, REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS,
    REQUIRED_FEATURE_CONVERSATION_DAG,
};
pub use model::{
    BodyManifest, ChunkHash, ChunkRef, HeaderAtom, HeaderBlock, HeaderBlockId, HeaderFidelity,
    ManifestId,
};
pub use range::RangeMatchConfig;
pub use replay::{StreamReplay, StreamReplayEvent, StreamReplaySource, StreamReplayTiming};
pub use search::{RawBodyScanner, RawSearchLimits, RawSearchStats};

/// Errors returned for malformed, unsupported, corrupt, or incomplete archives.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Invalid(&'static str),
    InvalidDetail(String),
    Unsupported(String),
    Checksum {
        offset: u64,
    },
    Limit {
        what: &'static str,
        actual: u64,
        limit: u64,
    },
    Missing(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Invalid(message) => write!(f, "invalid LAR data: {message}"),
            Self::InvalidDetail(message) => write!(f, "invalid LAR data: {message}"),
            Self::Unsupported(message) => write!(f, "unsupported LAR feature: {message}"),
            Self::Checksum { offset } => write!(f, "checksum mismatch at byte {offset}"),
            Self::Limit {
                what,
                actual,
                limit,
            } => {
                write!(f, "{what} exceeds limit ({actual} > {limit})")
            }
            Self::Missing(id) => write!(f, "archive record not found: {id}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
