pub mod chunker;
pub mod config;
pub mod domain;
pub mod embed;
pub mod error;
pub mod lsp;
pub mod parser;
pub mod store;
pub mod traits;

pub use chunker::LineChunker;
pub use config::CoderagConfig;
pub use domain::{Chunk, ChunkId, ChunkMetadata, ChunkType, Language, ScoredChunk};
pub use embed::CandleProvider;
pub use error::{CoderagError, Result};
pub use lsp::LspClient;
pub use parser::AstChunker;
pub use store::LanceDbStore;
pub use traits::{ChunkStore, EmbeddingProvider};
