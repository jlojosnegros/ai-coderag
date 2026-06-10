use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::{CoderagError, EmbeddingProvider, Result};

pub struct FastembedProvider {
    model: Arc<Mutex<TextEmbedding>>,
    dimension: usize,
    model_id: String,
}

impl FastembedProvider {
    /// Initialize the embedding model. Downloads the model on first run (500Mb).
    /// This is intentionally synchronous-looking at the call site, it blocks
    /// until the model is ready, which can take several seconds.
    pub async fn new() -> Result<Self> {
        let model_name = EmbeddingModel::JinaEmbeddingsV2BaseCode;
        let model_id = model_name.to_string();

        tracing::info!("Loading embedding model {} (may download on first run) ...", model_id);

        // textEmbedding::try_new is synchronous and potentially slow (downloads model)
        // spawn_blocking moves it off the async runtime thread pool
        let mut model = tokio::task::spawn_blocking(move || {
            TextEmbedding::try_new(InitOptions::new(model_name)).map_err(|err| CoderagError::Embedding(err.to_string()))
        })
        .await
        .map_err(|err| CoderagError::Embedding(format!("spawn_blocking join error: {err}")))??;

        // fastembed v5 removed get_embedding_dimension()
        // derive dimension by embedding one test string
        let test_embedding = model
            .embed(vec!["dimension_probe"], None)
            .map_err(|err| CoderagError::Embedding(format!("dimension probe failed: {err}")))?;

        let dimension = test_embedding[0].len();

        tracing::info!("Model loaded. Embedding dimension: {}", dimension);

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            dimension,
            model_id,
        })
    }

    /// Normalize a vector to unit norm in place.
    fn normalize(v: &mut [f32]) {
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm > 1e-10 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    }
}

#[async_trait]
impl EmbeddingProvider for FastembedProvider {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let texts_owned = texts.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let model_arc = Arc::clone(&self.model);

        // fastembed::TextEmbedding::embed() takes &mut self and is CPU-bound
        // Move the Arc into spawn_blocking so the work happens in the thread pool
        let mut embeddings = tokio::task::spawn_blocking(move || {
            let mut guard = model_arc
                .lock()
                .map_err(|err| CoderagError::Embedding(format!("mutex poisoned: {err}")))?;

            guard
                .embed(texts_owned, None)
                .map_err(|err| CoderagError::Embedding(err.to_string()))
        })
        .await
        .map_err(|err| CoderagError::Embedding(format!("spawn_blocking join error: {err}")))??;

        // Normalize all output vectors to unit form
        for v in &mut embeddings {
            Self::normalize(v);
        }
        Ok(embeddings)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}
