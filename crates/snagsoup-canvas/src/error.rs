use thiserror::Error;

#[derive(Debug, Error)]
pub enum CanvasError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Model error: {0}")]
    Model(#[from] snagsoup_model::ModelError),

    #[error("PNG decode error: {0}")]
    PngDecode(#[from] png::DecodingError),

    #[error("PNG encode error: {0}")]
    PngEncode(#[from] png::EncodingError),

    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("Invalid canvas dimensions: {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },

    #[error("RGBA buffer length mismatch: expected {expected}, got {actual}")]
    BufferLengthMismatch { expected: usize, actual: usize },
}

pub type Result<T> = std::result::Result<T, CanvasError>;
