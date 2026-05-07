//! Error types. See `rust-core-api.md` §1.8.

pub type Result<T> = std::result::Result<T, OtterzipError>;

#[derive(Debug, thiserror::Error)]
pub enum OtterzipError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),

    #[error("unsupported format: {0:?}")]
    UnsupportedFormat(Option<String>),

    #[error("corrupted archive: {reason}")]
    Corrupted { reason: String, entry: Option<String> },

    #[error("wrong password")]
    WrongPassword,

    #[error("missing volume {index}")]
    MissingVolume { index: u32, expected_name: Option<String> },

    #[error("operation canceled")]
    Canceled,

    #[error("feature disabled: {0}")]
    FeatureDisabled(&'static str),

    #[error("entry not found: {0}")]
    EntryNotFound(String),

    #[error("path traversal rejected: {0}")]
    PathTraversalBlocked(String),

    #[error("zip-bomb suspected: ratio {ratio} exceeds limit {limit} for entry {entry}")]
    ZipBombSuspected {
        entry: String,
        ratio: u64,
        limit: u32,
    },

    #[error("backend error: {0}")]
    BackendError(String),
}
