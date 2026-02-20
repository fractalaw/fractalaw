//! ONNX Runtime embedding pipeline for sentence-transformers models.
//!
//! Implements mean-pooled embeddings using all-MiniLM-L6-v2 (384 dimensions).
//! The model directory must contain `model.onnx` and `tokenizer.json`.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;
use tracing::info;

/// Sentence embedding generator using ONNX Runtime.
///
/// Loads a sentence-transformers model (e.g., all-MiniLM-L6-v2) and produces
/// 384-dimensional normalized embeddings suitable for cosine similarity search.
pub struct Embedder {
    session: Session,
    tokenizer: Tokenizer,
    dim: usize,
}

impl Embedder {
    /// Load an embedding model from a directory containing `model.onnx` and `tokenizer.json`.
    pub fn load(model_dir: &Path) -> anyhow::Result<Self> {
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        anyhow::ensure!(model_path.exists(), "model.onnx not found in {model_dir:?}");
        anyhow::ensure!(
            tokenizer_path.exists(),
            "tokenizer.json not found in {model_dir:?}"
        );

        let session = Session::builder()?.commit_from_file(&model_path)?;

        // Infer embedding dimension from model output shape.
        let dim = infer_dim(session.outputs()[0].dtype()).unwrap_or(384);

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("load tokenizer: {e}"))?;

        // Configure truncation to model's max length (256 for MiniLM).
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: 256,
                ..Default::default()
            }))
            .map_err(|e| anyhow::anyhow!("set truncation: {e}"))?;

        // Configure padding to pad all inputs in a batch to the same length.
        tokenizer.with_padding(Some(tokenizers::PaddingParams {
            ..Default::default()
        }));

        info!(dim, model = %model_path.display(), "loaded embedding model");
        Ok(Self {
            session,
            tokenizer,
            dim,
        })
    }

    /// Embedding dimensionality (384 for all-MiniLM-L6-v2).
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Embed a single text string, returning a normalized vector.
    pub fn embed(&mut self, text: &str) -> anyhow::Result<Vec<f32>> {
        let results = self.embed_batch(&[text])?;
        Ok(results.into_iter().next().unwrap())
    }

    /// Embed a batch of texts, returning one normalized vector per input.
    pub fn embed_batch(&mut self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let batch_size = texts.len();

        // Tokenize all texts.
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;

        let seq_len = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(0);

        // Build flat input tensors: [batch_size, seq_len].
        let mut input_ids = vec![0i64; batch_size * seq_len];
        let mut attention_mask = vec![0i64; batch_size * seq_len];
        let mut token_type_ids = vec![0i64; batch_size * seq_len];

        for (i, encoding) in encodings.iter().enumerate() {
            let offset = i * seq_len;
            for (j, &id) in encoding.get_ids().iter().enumerate() {
                input_ids[offset + j] = id as i64;
            }
            for (j, &mask) in encoding.get_attention_mask().iter().enumerate() {
                attention_mask[offset + j] = mask as i64;
            }
            for (j, &tid) in encoding.get_type_ids().iter().enumerate() {
                token_type_ids[offset + j] = tid as i64;
            }
        }

        let shape = [batch_size as i64, seq_len as i64];

        let ids_tensor = Tensor::from_array((shape, input_ids.into_boxed_slice()))?;
        let mask_tensor = Tensor::from_array((shape, attention_mask.clone().into_boxed_slice()))?;
        let type_tensor = Tensor::from_array((shape, token_type_ids.into_boxed_slice()))?;

        // Run inference.
        let outputs = self.session.run(ort::inputs![
            "input_ids" => ids_tensor,
            "attention_mask" => mask_tensor,
            "token_type_ids" => type_tensor,
        ])?;

        // Extract token embeddings: [batch_size, seq_len, dim].
        let (output_shape, output_data) = outputs[0].try_extract_tensor::<f32>()?;
        let dims: &[i64] = output_shape;
        anyhow::ensure!(
            dims.len() == 3 && dims[0] as usize == batch_size && dims[2] as usize == self.dim,
            "unexpected output shape: {dims:?}, expected [{batch_size}, {seq_len}, {}]",
            self.dim
        );

        let actual_seq_len = dims[1] as usize;

        // Mean pooling with attention mask.
        let mut embeddings = Vec::with_capacity(batch_size);
        for i in 0..batch_size {
            let mut pooled = vec![0.0f32; self.dim];
            let mut token_count = 0.0f32;

            for j in 0..actual_seq_len {
                let mask_val = attention_mask[i * seq_len + j] as f32;
                if mask_val > 0.0 {
                    let offset = (i * actual_seq_len + j) * self.dim;
                    for (d, p) in pooled.iter_mut().enumerate() {
                        *p += output_data[offset + d] * mask_val;
                    }
                    token_count += mask_val;
                }
            }

            // Average and normalize to unit length.
            if token_count > 0.0 {
                for p in &mut pooled {
                    *p /= token_count;
                }
            }
            normalize(&mut pooled);
            embeddings.push(pooled);
        }

        Ok(embeddings)
    }
}

/// L2-normalize a vector in place.
fn normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Try to infer the embedding dimension from the ONNX model output type.
fn infer_dim(output_type: &ort::value::ValueType) -> Option<usize> {
    match output_type {
        ort::value::ValueType::Tensor { shape, .. } => {
            // Last dimension is the embedding dim.
            shape
                .last()
                .and_then(|&d| if d > 0 { Some(d as usize) } else { None })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn model_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("models")
            .join("all-MiniLM-L6-v2")
    }

    fn require_model() -> PathBuf {
        let dir = model_dir();
        if !dir.join("model.onnx").exists() {
            panic!(
                "Model not found. Download from HuggingFace:\n  \
                 curl -L -o models/all-MiniLM-L6-v2/model.onnx \
                 https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx"
            );
        }
        dir
    }

    #[test]
    fn load_model() {
        let dir = require_model();
        let embedder = Embedder::load(&dir).unwrap();
        assert_eq!(embedder.dim(), 384);
    }

    #[test]
    fn embed_single_text() {
        let dir = require_model();
        let mut embedder = Embedder::load(&dir).unwrap();
        let vec = embedder.embed("Health and safety at work").unwrap();
        assert_eq!(vec.len(), 384);

        // Vector should be normalized (L2 norm ≈ 1.0).
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "expected unit norm, got {norm}");
    }

    #[test]
    fn embed_batch() {
        let dir = require_model();
        let mut embedder = Embedder::load(&dir).unwrap();
        let texts = &[
            "Chemical exposure limits in the workplace",
            "Fire safety regulations for commercial buildings",
            "Environmental protection and waste disposal",
        ];
        let vecs = embedder.embed_batch(texts).unwrap();
        assert_eq!(vecs.len(), 3);
        for (i, v) in vecs.iter().enumerate() {
            assert_eq!(v.len(), 384, "text {i} has wrong dimension");
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!(
                (norm - 1.0).abs() < 1e-4,
                "text {i}: expected unit norm, got {norm}"
            );
        }
    }

    #[test]
    fn similar_texts_closer() {
        let dir = require_model();
        let mut embedder = Embedder::load(&dir).unwrap();

        let v_safety = embedder.embed("workplace health and safety").unwrap();
        let v_coshh = embedder
            .embed("control of substances hazardous to health")
            .unwrap();
        let v_tax = embedder.embed("income tax legislation").unwrap();

        let sim_safety_coshh = cosine_sim(&v_safety, &v_coshh);
        let sim_safety_tax = cosine_sim(&v_safety, &v_tax);

        assert!(
            sim_safety_coshh > sim_safety_tax,
            "safety↔COSHH ({sim_safety_coshh:.4}) should be more similar than safety↔tax ({sim_safety_tax:.4})"
        );
    }

    #[test]
    fn embed_empty_batch() {
        let dir = require_model();
        let mut embedder = Embedder::load(&dir).unwrap();
        let vecs = embedder.embed_batch(&[]).unwrap();
        assert!(vecs.is_empty());
    }

    fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }
}
