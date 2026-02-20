//! AI inference layer: ONNX Runtime for embeddings/classification, LLM for generative tasks.

#[cfg(feature = "onnx")]
mod embedder;
#[cfg(feature = "onnx")]
pub use embedder::Embedder;
