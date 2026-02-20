//! AI inference layer: ONNX Runtime for embeddings/classification, LLM for generative tasks.

#[cfg(feature = "onnx")]
mod embedder;
#[cfg(feature = "onnx")]
pub use embedder::Embedder;

pub mod classifier;
pub mod labels;
pub use classifier::{
    CentroidSummary, Classification, ClassificationStatus, Classifier, aggregate_law_embeddings,
};
pub use labels::{EXCLUDE_FAMILIES, LabelSet, LabelSummary};
