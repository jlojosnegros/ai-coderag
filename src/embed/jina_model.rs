// Implementation of JunaBERT's forward pass for janaai/jina-embeddings-v2-base-code
// candle-transformers/jina_bert.rs was writting for jina-embeddings-v2-base-en
// code model has three main arch differences
// 1. QK layer norms after Q and K projections
// 2. Split GLU: first part = value, second = gate (GELU)
// 3. layer_norm_1 between attention output and FFN input

use candle_core::{D, DType, Device, Result, Tensor};
use candle_nn::{LayerNorm, Module, VarBuilder, embedding, layer_norm, linear, linear_no_bias};
use candle_transformers::models::jina_bert::Config;

/// BertEmbeddings same as candle
///
/// Transform token IDs to initial vectors
struct BertEmbeddings {
    word_embeddings: candle_nn::Embedding,
    token_type_embeddings: candle_nn::Embedding,
    layer_norm: LayerNorm,
    span: tracing::Span,
}

impl BertEmbeddings {
    fn new(vb: VarBuilder, cfg: &Config) -> Result<Self> {
        let word_embeddings = embedding(cfg.vocab_size, cfg.hidden_size, vb.pp("word_embeddings"))?;
        let token_type_embeddings = embedding(cfg.vocab_size, cfg.hidden_size, vb.pp("token_type_embeddings"))?;
        let layer_norm = layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("LayerNorm"))?;
        Ok(Self {
            word_embeddings,
            token_type_embeddings,
            layer_norm,
            span: tracing::span!(tracing::Level::TRACE, "embeddings"),
        })
    }
}

impl Module for BertEmbeddings {
    fn forward(&self, input_ids: &Tensor) -> Result<Tensor> {
        let _enter = self.span.enter();
        let (b_size, seq_len) = input_ids.dims2()?;
        let input_embeddings = self.word_embeddings.forward(input_ids)?;

        let token_type_ids = Tensor::zeros(seq_len, DType::U32, input_ids.device())?.broadcast_left(b_size)?;

        let token_type_embeddings = self.token_type_embeddings.forward(&token_type_ids)?;
        let embeddings = (input_embeddings + token_type_embeddings)?;
        self.layer_norm.forward(&embeddings)
    }
}

// ---------------------------------------------------------------------------
// BertSelfAttention — WITH QK layer norms (fix 1)
//
// Projects each token's vectors into Q, K, V and computes attention.
//
// FIX vs candle: after the Q and K projections, we apply layer_norm_q and
// layer_norm_k respectively. This normalizes Q and K before computing the
// Q×K^T scores, preventing the scores from exploding.
//
// Tensor paths in the safetensors file:
//   encoder.layer.N.attention.self.query.{weight,bias}
//   encoder.layer.N.attention.self.key.{weight,bias}
//   encoder.layer.N.attention.self.value.{weight,bias}
//   encoder.layer.N.attention.self.layer_norm_q.{weight,bias}  ← new
//   encoder.layer.N.attention.self.layer_norm_k.{weight,bias}  ← new
// ---------------------------------------------------------------------------
struct BertSelfAttention {
    query: candle_nn::Linear,
    key: candle_nn::Linear,
    value: candle_nn::Linear,
    layer_norm_q: LayerNorm,
    layer_norm_k: LayerNorm,
    num_attention_heads: usize,
    attention_head_size: usize,
    span: tracing::Span,
    span_softmax: tracing::Span,
}

impl BertSelfAttention {
    fn new(vb: VarBuilder, cfg: &Config) -> Result<Self> {
        let attention_head_size = cfg.hidden_size / cfg.num_attention_heads;
        let all_head_size = cfg.num_attention_heads * attention_head_size;
        let query = linear(cfg.hidden_size, all_head_size, vb.pp("query"))?;
        let key = linear(cfg.hidden_size, all_head_size, vb.pp("key"))?;
        let value = linear(cfg.hidden_size, all_head_size, vb.pp("value"))?;

        // LayerNorm sobre all_head_size == hidden_size
        let layer_norm_q = layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("layer_norm_q"))?;
        let layer_norm_k = layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("layer_norm_k"))?;

        Ok(Self {
            query,
            key,
            value,
            layer_norm_q,
            layer_norm_k,
            num_attention_heads: cfg.num_attention_heads,
            attention_head_size,
            span: tracing::span!(tracing::Level::TRACE, "self-attn"),
            span_softmax: tracing::span!(tracing::Level::TRACE, "softmax"),
        })
    }

    /// Reroganize a Tensor of [batch, seq, all_head_size] to
    /// [batch, num_heads, seq, head_size] to each head could process
    /// its own token "view"  in parallel
    fn transpose_for_scores(&self, xs: &Tensor) -> Result<Tensor> {
        let mut shape = xs.dims().to_vec();
        shape.pop();
        shape.push(self.num_attention_heads);
        shape.push(self.attention_head_size);
        xs.reshape(shape)?.transpose(1, 2)?.contiguous()
    }

    fn forward(&self, xs: &Tensor, bias: &Tensor) -> Result<Tensor> {
        let _enter = self.span.enter();
        // Proyectar y aplicar QK norms
        let query_layer = self.layer_norm_q.forward(&self.query.forward(xs)?)?;
        let key_layer = self.layer_norm_k.forward(&self.key.forward(xs)?)?;
        let value_layer = self.value.forward(xs)?;

        let query_layer = self.transpose_for_scores(&query_layer)?;
        let key_layer = self.transpose_for_scores(&key_layer)?;
        let value_layer = self.transpose_for_scores(&value_layer)?;

        // Scores de atencion: Q x K^T / sqrt(d_head)
        // El bias es el AliBi: añade un sesgo lineal según la distancia entre tokens
        let scores = query_layer.matmul(&key_layer.t()?)?;
        let scores = (scores / (self.attention_head_size as f64).sqrt())?;
        let scores = scores.broadcast_add(bias)?;
        let probs = {
            let _sm = self.span_softmax.enter();
            candle_nn::ops::softmax_last_dim(&scores)?
        };
        // Media ponderada de Values según las probabilidades de atención
        let ctx = probs.matmul(&value_layer)?;
        let ctx = ctx.transpose(1, 2)?.contiguous()?;
        ctx.flatten_from(D::Minus2)
    }
}

/// BertSelfOutput — identical to candle (no correction needed)
///
/// Post-attention: linearly projects the attention result
/// and applies LayerNorm with residual (input_tensor = original input to attention).
///
/// Paths: encoder.layer.N.attention.output.dense.{weight,bias}
///        encoder.layer.N.attention.output.LayerNorm.{weight,bias}
struct BertSelfOutput {
    dense: candle_nn::Linear,
    layer_norm: LayerNorm,
    span: tracing::Span,
}

impl BertSelfOutput {
    fn new(vb: VarBuilder, cfg: &Config) -> Result<Self> {
        let dense = linear(cfg.hidden_size, cfg.hidden_size, vb.pp("dense"))?;
        let layer_norm = layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("LayerNorm"))?;
        Ok(Self {
            dense,
            layer_norm,
            span: tracing::span!(tracing::Level::TRACE, "self-out"),
        })
    }

    fn forward(&self, xs: &Tensor, input_tensor: &Tensor) -> Result<Tensor> {
        let _enter = self.span.enter();
        let xs = self.dense.forward(xs)?;
        // Residual:: attention_output = LayerNorm(dense(attn) + input)
        self.layer_norm.forward(&(xs + input_tensor)?)
    }
}
/// BertAttention: Combines SelfAttention and SelfOutput
struct BertAttention {
    self_attention: BertSelfAttention,
    self_output: BertSelfOutput,
    span: tracing::Span,
}

impl BertAttention {
    fn new(vb: VarBuilder, cfg: &Config) -> Result<Self> {
        let self_attention = BertSelfAttention::new(vb.pp("self"), cfg)?;
        let self_output = BertSelfOutput::new(vb.pp("output"), cfg)?;
        Ok(Self {
            self_attention,
            self_output,
            span: tracing::span!(tracing::Level::TRACE, "attn"),
        })
    }

    fn forward(&self, xs: &Tensor, bias: &Tensor) -> Result<Tensor> {
        let _enter = self.span.enter();
        let self_out = self.self_attention.forward(xs, bias)?;
        self.self_output.forward(&self_out, xs)
    }
}

/// JinaGLUMLP — GLU-MLP with correct split (correction 2) and no internal residual
///
/// CORRECTION vs candle:
///   candle:  GELU(first_half) × second_half
///   correct: first_half × GELU(second_half)
///
/// Also, this module does NOT perform the final residual nor LayerNorm.
/// That happens in JinaBertLayer (correction 3).
///
/// Paths:
///   encoder.layer.N.mlp.up_gated_layer.weight  (no bias: linear_no_bias)
///   encoder.layer.N.mlp.down_layer.{weight,bias}
struct JinaGLUMLP {
    up_gated_layer: candle_nn::Linear,
    down_layer: candle_nn::Linear,
    act: candle_nn::Activation,
    intermediate_size: usize,
}

impl JinaGLUMLP {
    fn new(vb: VarBuilder, cfg: &Config) -> Result<Self> {
        let up_gated_layer = linear_no_bias(cfg.hidden_size, cfg.intermediate_size * 2, vb.pp("up_gated_layer"))?;

        let down_layer = linear(cfg.intermediate_size, cfg.hidden_size, vb.pp("down_layer"))?;

        Ok(Self {
            up_gated_layer,
            down_layer,
            act: candle_nn::Activation::Gelu,
            intermediate_size: cfg.intermediate_size,
        })
    }
}

impl Module for JinaGLUMLP {
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        // Projection up: 768 -> 6144
        let projected = xs.apply(&self.up_gated_layer)?;

        // Split: first part = value (without activation), second part = gate(GELU)
        let up = projected.narrow(D::Minus1, 0, self.intermediate_size)?;
        let gated = projected.narrow(D::Minus1, self.intermediate_size, self.intermediate_size)?;

        // Result: value x GELU(gate)
        // Projection down: 3072 -> 768
        (up * gated.apply(&self.act))?.apply(&self.down_layer)
    }
}

/// JinaBertLayer — a complete layer with layer_norm_1 and layer_norm_2 (correction 3)
///
/// CORRECTION vs candle: we add layer_norm_1 between attention and FFN.
/// The correct sequence is:
///
///   attention_output = BertAttention(xs)     [includes attention.output.LayerNorm]
///   residual = layer_norm_1(xs + attention_output)   ← second post-attention norm
///   ffn_output = JinaGLUMLP(residual)       [no internal residual]
///   output = layer_norm_2(residual + ffn_output)
///
/// Paths:
///   encoder.layer.N.layer_norm_1.{weight,bias}  ← new (not in candle)
///   encoder.layer.N.layer_norm_2.{weight,bias}  ← new (not in candle)
struct JinaBertLayer {
    attention: BertAttention,
    mlp: JinaGLUMLP,
    layer_norm_1: LayerNorm,
    layer_norm_2: LayerNorm,
    span: tracing::Span,
}

impl JinaBertLayer {
    fn new(vb: VarBuilder, cfg: &Config) -> Result<Self> {
        let attention = BertAttention::new(vb.pp("attention"), cfg)?;
        let mlp = JinaGLUMLP::new(vb.pp("mlp"), cfg)?;
        let layer_norm_1 = layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("layer_norm_1"))?;
        let layer_norm_2 = layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("layer_norm_2"))?;
        Ok(Self {
            attention,
            mlp,
            layer_norm_1,
            layer_norm_2,
            span: tracing::span!(tracing::Level::TRACE, "layer"),
        })
    }

    fn forward(&self, xs: &Tensor, bias: &Tensor) -> Result<Tensor> {
        let _enter = self.span.enter();
        // 1 Attention
        let attention_output = self.attention.forward(xs, bias)?;
        // 2 layer_norm_1
        let residual = self.layer_norm_1.forward(&(xs + attention_output)?)?;
        // 3 FFN
        let mlp_output = self.mlp.forward(&residual)?;
        // 4 layer_norm_2
        self.layer_norm_2.forward(&(&residual + mlp_output)?)
    }
}


// ---------------------------------------------------------------------------
// ALiBi bias — computed per-inference, NOT pre-allocated for max_seq_len
//
// ALiBi (Attention with Linear Biases, Press et al. 2021) replaces learned positional
// embeddings. Instead of adding a position vector to each token, ALiBi adds a negative
// linear bias directly to the attention scores:
//   bias[h, i, j] = -slope_h × |i - j|
// where slope_h is a fixed, head-specific scalar.
//
// Effect: tokens far apart get a larger negative score before softmax, reducing the
// probability of attending across long distances. Heads with a larger slope specialize
// in short-range dependencies; smaller slopes allow long-range attention.
//
// Advantage over learned positional embeddings:
//   - No parameters to train; slopes are closed-form formulas.
//   - Extrapolates to sequences longer than seen during training: the linear penalty extends naturally to any distance
//     without needing a learned embedding for that position.
//
// Memory optimization vs candle:
//   Candle pre-allocates bias for max_position_embeddings=8192:
//     [1, 12, 8192, 8192] × 4 bytes = 3 GB resident in RAM at all times.
//   Here, bias is computed for the actual seq_len on each forward pass:
//     [1, 12, 512, 512] × 4 bytes = 12 MB, freed immediately after forward().
//   Only alibi_slopes (the 12 scalars) is stored permanently: [1, 12, 1, 1] = 48 bytes.
// ---------------------------------------------------------------------------

/// Computes the per-head slope scalars from the model config.
///
/// The ALiBi paper defines slopes as a geometric sequence: 2^(-8k/n) for k=1..n,
/// where n is the next power of 2 ≥ num_attention_heads. If num_heads is not a power
/// of 2, slopes are interleaved (odd indices first, then even) to maintain even coverage
/// of the geometric range across heads.
///
/// Returns shape [1, n_heads, 1, 1] for broadcasting against [batch, n_heads, seq, seq].
fn build_alibi_slopes(cfg: &Config) -> Result<Tensor> {
    // calculate the slop of each attention head.
    // slope means how much token distance hinders
    let n_heads = cfg.num_attention_heads;
    let mut n2 = 1usize;

    while n2 < n_heads {
        n2 *= 2;
    }

    let slopes = (1..=n2)
        .map(|v| -1f32.powf((v * 8) as f32 / n2 as f32))
        .collect::<Vec<_>>();

    // If num_heads is not a power of 2, interleave odd-indexed then even-indexed slopes
    // to distribute the geometric range evenly (matches the reference implementation).
    let slopes = if n2 == n_heads {
        slopes
    } else {
        slopes
            .iter()
            .skip(1)
            .step_by(2)
            .chain(slopes.iter().step_by(2))
            .copied()
            .collect()
    };

    // Shape [1, n_heads, 1, 1]: the batch and seq_len dims broadcast to the full bias shape
    Tensor::new(slopes.as_slice(), &Device::Cpu)?.reshape((1, n_heads, 1, 1))
}

/// Computes the full ALiBi bias matrix for a given sequence length.
///
/// Builds a position distance matrix dist[i,j] = |j - i| for positions 0..seq_len,
/// then multiplies each row by the corresponding head's slope (via broadcasting).
///
/// Called once per encoder forward pass in JinaBertEncoder, shared across all 12 layers.
/// The returned tensor [1, n_heads, seq_len, seq_len] is temporary: freed after forward().
fn compute_alibi_bias(slopes: &Tensor, seq_len: usize, device: &Device) -> Result<Tensor> {
    // Position indices as F32: [0.0, 1.0, ..., seq_len-1]
    let positions = Tensor::arange(0i64, seq_len as i64, device)?.to_dtype(DType::F32)?;

    // Outer subtraction then abs: dist[i, j] = |j - i|
    // positions as row [1, seq] minus positions as column [seq, 1] → [seq, seq]
    let dist = positions
        .reshape((1, seq_len))?
        .broadcast_sub(&positions.reshape((seq_len, 1))?)?
        .abs()?;

    // Expand dist from [seq, seq] to [n_heads, seq, seq], then scale each head's slice
    // by its slope scalar. broadcast_mul applies slopes [1, n_heads, 1, 1] independently
    // to each head, producing [1, n_heads, seq, seq].
    let n_heads = slopes.dim(1)?;
    dist.broadcast_left(n_heads)?.broadcast_mul(slopes)
}
