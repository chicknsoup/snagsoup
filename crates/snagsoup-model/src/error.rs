use thiserror::Error;

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization/deserialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("ZIP archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("Invalid color string: {0}")]
    InvalidColor(String),

    #[error("Invalid coordinate string: {0}")]
    InvalidCoordinate(String),

    #[error("Missing required archive entry: {0}")]
    MissingEntry(String),

    #[error("Invalid UTF-8 in RTF or text data: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub type Result<T> = std::result::Result<T, ModelError>;
