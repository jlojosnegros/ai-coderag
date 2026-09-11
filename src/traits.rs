use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::{Chunk, Result, ScoredChunk};

/// Converts text into dense vector representations.
/// Implementations must normalize output vectors to unit norm.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a batch of texts. Returns one unit-norm vector per text
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// Number of dimensions in the output vectors.
    /// Must match the LanceDB schema column width.
    fn dimension(&self) -> usize;

    /// Human readable model identifier, used in log messages
    fn model_id(&self) -> &str;
}


/// Persistent storage for chunks and their embeddings
#[async_trait]
pub trait ChunkStore: Send + Sync {
    /// Insert or update chunks.
    /// Idempotent: calling upsert twice with the same chunk (same ChunkId)
    /// replaces the first with the second.
    /// All chunk must have `embedding = Some(...) ` before calling upsert.
    async fn upsert(&self, chunks: &[Chunk]) -> Result<()>;

    /// Find the k chunks whose embeddings are most similar to query_vec.
    /// query_vec must be unit-norm (same normalization as stored embeddings).
    async fn search_vector(&self, query_vec: &[f32], k: usize) -> Result<Vec<ScoredChunk>>;
}


/// Encapsulates language-specific knowledge for LSP integration.
///
/// Each language server has different requirements: where to find the project root,
/// what binary to launch, what capabilities to declare ...
/// This trait captures that knowledge so `LspClient` can remain language-agnostic.
///
/// Implementations provide sensible defaults.
/// Deployment overrides (custom binary path, extra args, timeout ...) live in
/// `coderag.toml` under `[lsp.servers.<language_id>]`
pub trait LanguageLsp: Send + Sync {
    /// LSP specification language identifier( e.g.: "rust", "cpp", "go")
    ///
    /// Used as the `languageId` in `textDocument/didOpen` and as the key in
    /// `coderag.toml`'s `[lsp.servers.<id>]`
    fn language_id(&self) -> &str;

    /// File extensions this LSP provider handles(without leading dot).
    ///
    /// Used by `Languageregistry::lsp_for_extensions()` to route files
    fn file_extensions(&self) -> &[&str];

    /// Default binary name for the language server (e.g. "rust-analyzer")
    ///
    /// Overridable via `[lsp.servers.<id>].command` in `coderag.toml`
    fn default_command(&self) -> &str;

    /// Default arguments for the language server binary.
    ///
    /// Overridable via `[lsp.servers.<id>].args` in `coderag.toml`
    fn default_args(&self) -> Vec<String> {
        Vec::new()
    }

    /// Find the project root for this language starting from `start`
    /// Returns `None` if no proyect root is found.
    fn find_project_root(&self, start: &Path) -> Option<PathBuf>;


    /// Capabilities to declare in the LSP `initialize` request.
    ///
    /// Each language may need different capabilities.
    fn initialize_capabilities(&self) -> lsp_types::ClientCapabilities;
}

/// Configuration for launching an LSP server process.
/// Produced by merging trait defaults with `coderag.toml` overrides
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub timeout_secs: u64,
}
