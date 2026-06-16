use std::{fs::read_to_string, sync::Arc};

use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::{Module, VarBuilder};
use candle_transformers::models::jina_bert::{BertModel, Config};
use hf_hub::{Repo, RepoType, api::sync::Api};
use tokenizers::Tokenizer;

use crate::{CoderagError, EmbeddingProvider, Result};

/// Embedding provider backed by Candle and the Jina v2 code model.
///
/// Runst entirely in-process on the CPU. No ONNX runtime, no system libraries.
/// Weights (around 550MB) are downloaded once from HuggingFace Hub and cached
/// al $HOME/.cache/huggingface/hub
pub struct CandleProvider {
    /// BerModel is Send + Sync (candle tensors are Arc<Storage> internally)
    /// so Arc alone is enough (no Mutex needed)
    model: Arc<BertModel>,
    tokenizer: Arc<Tokenizer>,
    device: Device,
    dimension: usize,
    model_id: String,
}

impl CandleProvider {
    const CONFIG_FILENAME: &str = "config.json";
    const TOKENIZER_FILENAME: &str = "tokenizer.json";
    const WEIGHTS_FILENAME: &str = "model.safetensors";

    pub async fn new() -> Result<Self> {
        let repo_id = "jinaai/jina-embeddings-v2-base-code".to_string();
        tracing::info!("Loading embedding model {}(may download on first run) ...", repo_id);

        tokio::task::spawn_blocking(move || {
            let device = Device::Cpu;
            // Download model files from HuggingFace Hub.
            // Files are catched in $HOME/.cache/huggingface/hub/ after the first download
            let api = Api::new().map_err(|err| CoderagError::Embedding(format!("hf-hub init: {err}")))?;
            let repo = api.repo(Repo::new(repo_id.clone(), RepoType::Model));

            let tokenizer_file = repo
                .get(Self::TOKENIZER_FILENAME)
                .map_err(|err| CoderagError::Embedding(format!("{}: {err}", Self::TOKENIZER_FILENAME)))?;

            let config_file = repo
                .get(Self::CONFIG_FILENAME)
                .map_err(|err| CoderagError::Embedding(format!("{} : {err}", Self::CONFIG_FILENAME)))?;

            let weights_file = repo
                .get(Self::WEIGHTS_FILENAME)
                .map_err(|err| CoderagError::Embedding(format!("{} : {err}", Self::WEIGHTS_FILENAME)))?;

            let tokenizer = Tokenizer::from_file(tokenizer_file)
                .map_err(|err| CoderagError::Embedding(format!("tokenizer load: {err}")))?;

            let config_str = read_to_string(config_file)
                .map_err(|err| CoderagError::Embedding(format!("{} read: {err}", Self::CONFIG_FILENAME)))?;
            let config: Config = serde_json::from_str(&config_str)
                .map_err(|err| CoderagError::Embedding(format!("{} parse: {err}", Self::CONFIG_FILENAME)))?;

            let dimension = config.hidden_size;

            // VarBuilder maps the safetensors file into memory and exposes named tensors.
            // SAFETY: the file must not be modified while the program is used
            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[weights_file], DType::F32, &device)
                    .map_err(|err| CoderagError::Embedding(format!("weights load: {err}")))?
            };

            let model =
                BertModel::new(vb, &config).map_err(|err| CoderagError::Embedding(format!("model init: {err}")))?;

            tracing::info!(embedding_dimension = dimension, "Model loaded");

            Ok(CandleProvider {
                model: Arc::new(model),
                tokenizer: Arc::new(tokenizer),
                device,
                dimension,
                model_id: repo_id,
            })
        })
        .await
        .map_err(|err| CoderagError::Embedding(format!("spawn_blocking join: {err}")))?
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
impl EmbeddingProvider for CandleProvider {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let model = Arc::clone(&self.model);
        let tokenizer = Arc::clone(&self.tokenizer);
        let device = self.device.clone();
        let texts = texts.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // Inference is synchronous and CPU bound: keep off the async thread pool
        let mut embeddings = tokio::task::spawn_blocking(move || {
            texts
                .iter()
                .map(|text| embed_single(&model, &tokenizer, &device, text))
                .collect::<candle_core::Result<Vec<Vec<f32>>>>()
                .map_err(|err| CoderagError::Embedding(err.to_string()))
        })
        .await
        .map_err(|err| CoderagError::Embedding(format!("spawn_blocking join: {err}")))??;


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

/// Embed a single text. Must be called from inside spawn_blocking
///
/// Pipeline: text -> token Ids -> BERT forward pass -> mean pool -> raw vector
/// Normalization to unit norm happens in the caller
fn embed_single(
    model: &BertModel,
    tokenizer: &Tokenizer,
    device: &Device,
    text: &str,
) -> candle_core::Result<Vec<f32>> {
    // Step 1: tokenize
    // splits the text into sub-words tokens and maps each to an integer ID.
    // add_special_tokens=true prepends [CLS] (id 101) and appends [SEP] (id 102)
    let encoding = tokenizer
        .encode(text, true)
        .map_err(|err| candle_core::Error::Msg(err.to_string()))?;

    // Step 2: build input tensors
    // unsqueeze(0) adds the batch dimension: [seq_len] -> [1, seq_len]
    // BERT always processes batches, even of size 1.
    let input_ids = Tensor::new(encoding.get_ids(), device)?.unsqueeze(0)?;

    // Step 3: forward pass through the transformer.
    // Output shape: [1, seq_len, hidden_size] (hidden_size = 768 for Jina v2 base)
    let output = model.forward(&input_ids)?;

    // Step 4: mean pooling over seq_len
    // Averages all token representations into one sentence-level vector
    let seq_len = output.dim(1)? as f64;
    let embedding = (output.sum(1)? / seq_len)?.get(0)?;

    embedding.to_vec1::<f32>()
}
