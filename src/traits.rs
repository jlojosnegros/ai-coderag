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
