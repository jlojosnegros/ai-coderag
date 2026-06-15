#[derive(Debug, thiserror::Error)]
pub enum CoderagError {
    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("store error: {0}")]
    Store(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("LSP error: {0}")]
    Lsp(String),
}

/// Convinient alias for Result
pub type Result<T> = std::result::Result<T, CoderagError>;
