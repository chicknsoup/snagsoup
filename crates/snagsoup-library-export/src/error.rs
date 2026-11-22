use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Model error: {0}")]
    Model(#[from] snagsoup_model::ModelError),

    #[error("Canvas error: {0}")]
    Canvas(#[from] snagsoup_canvas::CanvasError),

    #[error("Image codec error: {0}")]
    Image(#[from] image::ImageError),

    #[error("GIF encoding error: {0}")]
    Gif(#[from] gif::EncodingError),

    #[error("SQLite error: {0}")]
    Sqlite(String),

    #[error("File watcher error: {0}")]
    Notify(#[from] notify::Error),

    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, ExportError>;
