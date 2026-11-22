use thiserror::Error;

#[derive(Debug, Error)]
pub enum CvOcrError {
    #[error("Canvas error: {0}")]
    Canvas(#[from] snagsoup_canvas::CanvasError),

    #[error("Model error: {0}")]
    Model(#[from] snagsoup_model::ModelError),

    #[error("Windows OCR / WinRT error: {0}")]
    WinRt(#[from] windows::core::Error),

    #[error("OCR engine unavailable on this system (no OCR language packs installed)")]
    OcrUnavailable,

    #[error("OCR operation cancelled")]
    Cancelled,

    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, CvOcrError>;
