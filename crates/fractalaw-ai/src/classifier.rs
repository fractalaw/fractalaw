//! Centroid-based classification for legislation.
//!
//! Aggregates section-level embeddings into law-level embeddings, computes
//! centroids per label (family, domain, subject), and classifies laws by
//! cosine similarity to the nearest centroid.

use std::collections::HashMap;

use arrow::array::{Array, FixedSizeListArray, Float32Array, LargeStringArray, StringArray};
use arrow::record_batch::RecordBatch;

use crate::labels::LabelSet;

/// Centroid-based classifier for legislation.
///
/// Holds pre-computed centroids per label for family, domain, and subject.
/// Classify a law by computing cosine similarity between its embedding and
/// each centroid, then selecting the best match.
pub struct Classifier {
    family_centroids: HashMap<String, Vec<f32>>,
    domain_centroids: HashMap<String, Vec<f32>>,
    subject_centroids: HashMap<String, Vec<f32>>,
    dim: usize,
}

/// Agreement status between AI prediction and ground-truth label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassificationStatus {
    /// No ground truth — AI prediction is the only classification.
    Predicted,
    /// Ground truth exists and AI agrees.
    Confirmed,
    /// Ground truth exists and AI disagrees — needs human review.
    Conflict,
}

impl ClassificationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Predicted => "predicted",
            Self::Confirmed => "confirmed",
            Self::Conflict => "conflict",
        }
    }
}

/// Classification result for a single law.
pub struct Classification {
    pub law_name: String,
    /// Multi-select: all domains above threshold, with confidence.
    pub domain: Vec<(String, f32)>,
    /// Single-select: best-matching family.
    pub family: String,
    pub family_confidence: f32,
    /// Multi-select: all subjects above threshold, with confidence.
    pub subjects: Vec<(String, f32)>,
    /// Agreement with ground-truth family label.
    pub status: ClassificationStatus,
}

/// Summary of centroid computation.
pub struct CentroidSummary {
    pub family_count: usize,
    pub domain_count: usize,
    pub subject_count: usize,
    pub laws_used: usize,
}

impl Classifier {
    /// Build a classifier by computing centroids from labelled law embeddings.
    ///
    /// `law_embeddings` maps law_name → normalized 384-dim vector (from
    /// [`aggregate_law_embeddings`]). Labels come from [`LabelSet`].
    pub fn build(law_embeddings: &HashMap<String, Vec<f32>>, labels: &LabelSet) -> Self {
        let dim = law_embeddings
            .values()
            .next()
            .map(|v| v.len())
            .unwrap_or(384);

        let family_centroids = compute_family_centroids(law_embeddings, labels, dim);
        let domain_centroids = compute_domain_centroids(law_embeddings, labels, dim);
        let subject_centroids = compute_subject_centroids(law_embeddings, labels, dim);

        Self {
            family_centroids,
            domain_centroids,
            subject_centroids,
            dim,
        }
    }

    /// Summary of centroid counts.
    pub fn summary(&self, laws_used: usize) -> CentroidSummary {
        CentroidSummary {
            family_count: self.family_centroids.len(),
            domain_count: self.domain_centroids.len(),
            subject_count: self.subject_centroids.len(),
            laws_used,
        }
    }

    /// Classify a single law from its aggregated embedding.
    ///
    /// Compares the AI prediction against ground-truth labels to set
    /// [`ClassificationStatus`]: `predicted`, `confirmed`, or `conflict`.
    pub fn classify(
        &self,
        law_name: &str,
        embedding: &[f32],
        labels: &LabelSet,
        domain_threshold: f32,
        subject_threshold: f32,
    ) -> Classification {
        // Family: single-select (best match).
        let (family, family_confidence) = best_match(&self.family_centroids, embedding);

        // Domain: multi-select (all above threshold).
        let domain = above_threshold(&self.domain_centroids, embedding, domain_threshold);

        // Subjects: multi-select (all above threshold).
        let subjects = above_threshold(&self.subject_centroids, embedding, subject_threshold);

        // Compute status by comparing against ground-truth family.
        let status = match labels.law_family.get(law_name) {
            None => ClassificationStatus::Predicted,
            Some(gt_family) if gt_family == &family => ClassificationStatus::Confirmed,
            Some(_) => ClassificationStatus::Conflict,
        };

        Classification {
            law_name: law_name.to_string(),
            domain,
            family,
            family_confidence,
            subjects,
            status,
        }
    }

    /// Classify a batch of laws from their aggregated embeddings.
    pub fn classify_batch(
        &self,
        law_embeddings: &HashMap<String, Vec<f32>>,
        labels: &LabelSet,
        domain_threshold: f32,
        subject_threshold: f32,
    ) -> Vec<Classification> {
        law_embeddings
            .iter()
            .map(|(name, emb)| {
                self.classify(name, emb, labels, domain_threshold, subject_threshold)
            })
            .collect()
    }

    /// Number of family centroids.
    pub fn family_count(&self) -> usize {
        self.family_centroids.len()
    }

    /// Number of domain centroids.
    pub fn domain_count(&self) -> usize {
        self.domain_centroids.len()
    }

    /// Number of subject centroids.
    pub fn subject_count(&self) -> usize {
        self.subject_centroids.len()
    }

    /// Embedding dimensionality.
    pub fn dim(&self) -> usize {
        self.dim
    }
}

/// Aggregate section-level embeddings into one law-level embedding per law.
///
/// Input: Arrow RecordBatches from LanceDB with `law_name` (Utf8/LargeUtf8)
/// and `embedding` (FixedSizeList<Float32, 384>) columns.
///
/// For each law, computes mean of all its section embeddings, then L2-normalizes.
pub fn aggregate_law_embeddings(
    batches: &[RecordBatch],
) -> anyhow::Result<HashMap<String, Vec<f32>>> {
    // Accumulate: law_name → (sum_vector, count).
    let mut accum: HashMap<String, (Vec<f32>, usize)> = HashMap::new();

    for batch in batches {
        let name_col = batch
            .column_by_name("law_name")
            .ok_or_else(|| anyhow::anyhow!("missing 'law_name' column"))?;
        let emb_col = batch
            .column_by_name("embedding")
            .ok_or_else(|| anyhow::anyhow!("missing 'embedding' column"))?;

        let fsl = emb_col
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .ok_or_else(|| anyhow::anyhow!("embedding column is not FixedSizeList"))?;

        let dim = fsl.value_length() as usize;

        // The underlying values are a single flat Float32Array.
        let flat_values = fsl
            .values()
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| anyhow::anyhow!("embedding values are not Float32"))?;

        for row in 0..batch.num_rows() {
            if name_col.is_null(row) || emb_col.is_null(row) {
                continue;
            }

            let name = get_string(name_col.as_ref(), row).unwrap();
            let offset = row * dim;
            let emb_slice = &flat_values.values()[offset..offset + dim];

            let entry = accum.entry(name).or_insert_with(|| (vec![0.0f32; dim], 0));
            for (acc, &val) in entry.0.iter_mut().zip(emb_slice) {
                *acc += val;
            }
            entry.1 += 1;
        }
    }

    // Mean and normalize.
    let mut result = HashMap::with_capacity(accum.len());
    for (name, (mut sum, count)) in accum {
        if count > 0 {
            for v in &mut sum {
                *v /= count as f32;
            }
            normalize(&mut sum);
            result.insert(name, sum);
        }
    }

    Ok(result)
}

// ── Centroid computation ──

fn compute_family_centroids(
    law_embeddings: &HashMap<String, Vec<f32>>,
    labels: &LabelSet,
    dim: usize,
) -> HashMap<String, Vec<f32>> {
    let mut accum: HashMap<&str, (Vec<f32>, usize)> = HashMap::new();

    for (name, family) in labels.trainable_laws() {
        if let Some(emb) = law_embeddings.get(name) {
            let entry = accum
                .entry(family)
                .or_insert_with(|| (vec![0.0f32; dim], 0));
            for (acc, &val) in entry.0.iter_mut().zip(emb) {
                *acc += val;
            }
            entry.1 += 1;
        }
    }

    finalize_centroids(accum)
}

fn compute_domain_centroids(
    law_embeddings: &HashMap<String, Vec<f32>>,
    labels: &LabelSet,
    dim: usize,
) -> HashMap<String, Vec<f32>> {
    let mut accum: HashMap<&str, (Vec<f32>, usize)> = HashMap::new();

    for (name, domains) in &labels.law_domain {
        if let Some(emb) = law_embeddings.get(name.as_str()) {
            // Multi-select: contribute to each domain's centroid.
            for domain in domains {
                let entry = accum
                    .entry(domain.as_str())
                    .or_insert_with(|| (vec![0.0f32; dim], 0));
                for (acc, &val) in entry.0.iter_mut().zip(emb) {
                    *acc += val;
                }
                entry.1 += 1;
            }
        }
    }

    finalize_centroids(accum)
}

fn compute_subject_centroids(
    law_embeddings: &HashMap<String, Vec<f32>>,
    labels: &LabelSet,
    dim: usize,
) -> HashMap<String, Vec<f32>> {
    let mut accum: HashMap<&str, (Vec<f32>, usize)> = HashMap::new();

    for (name, subjects) in &labels.law_subjects {
        if let Some(emb) = law_embeddings.get(name.as_str()) {
            // Multi-select: contribute to each subject's centroid.
            for subject in subjects {
                let entry = accum
                    .entry(subject.as_str())
                    .or_insert_with(|| (vec![0.0f32; dim], 0));
                for (acc, &val) in entry.0.iter_mut().zip(emb) {
                    *acc += val;
                }
                entry.1 += 1;
            }
        }
    }

    finalize_centroids(accum)
}

fn finalize_centroids(accum: HashMap<&str, (Vec<f32>, usize)>) -> HashMap<String, Vec<f32>> {
    let mut result = HashMap::with_capacity(accum.len());
    for (label, (mut sum, count)) in accum {
        if count > 0 {
            for v in &mut sum {
                *v /= count as f32;
            }
            normalize(&mut sum);
            result.insert(label.to_string(), sum);
        }
    }
    result
}

// ── Classification helpers ──

/// Find the centroid with highest cosine similarity.
fn best_match(centroids: &HashMap<String, Vec<f32>>, embedding: &[f32]) -> (String, f32) {
    let mut best_label = String::new();
    let mut best_sim = f32::NEG_INFINITY;

    for (label, centroid) in centroids {
        let sim = cosine_sim(embedding, centroid);
        if sim > best_sim {
            best_sim = sim;
            best_label.clone_from(label);
        }
    }

    (best_label, best_sim)
}

/// Find all centroids above a similarity threshold, sorted descending.
fn above_threshold(
    centroids: &HashMap<String, Vec<f32>>,
    embedding: &[f32],
    threshold: f32,
) -> Vec<(String, f32)> {
    let mut matches: Vec<(String, f32)> = centroids
        .iter()
        .filter_map(|(label, centroid)| {
            let sim = cosine_sim(embedding, centroid);
            if sim >= threshold {
                Some((label.clone(), sim))
            } else {
                None
            }
        })
        .collect();

    matches.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    matches
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
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

fn get_string(col: &dyn Array, row: usize) -> Option<String> {
    if col.is_null(row) {
        return None;
    }
    col.as_any()
        .downcast_ref::<StringArray>()
        .map(|arr| arr.value(row).to_string())
        .or_else(|| {
            col.as_any()
                .downcast_ref::<LargeStringArray>()
                .map(|arr| arr.value(row).to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::LabelSet;
    use arrow::array::{FixedSizeListBuilder, Float32Builder, StringBuilder};
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    const DIM: i32 = 4; // Small dim for tests.

    /// Build a test batch simulating LanceDB output.
    fn text_batch(rows: &[(&str, &[f32; 4])]) -> RecordBatch {
        let mut name_builder = StringBuilder::new();
        let mut emb_builder = FixedSizeListBuilder::new(Float32Builder::new(), DIM);

        for (name, emb) in rows {
            name_builder.append_value(name);
            let values = emb_builder.values();
            for &v in *emb {
                values.append_value(v);
            }
            emb_builder.append(true);
        }

        let schema = Schema::new(vec![
            Field::new("law_name", DataType::Utf8, false),
            Field::new(
                "embedding",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), DIM),
                true,
            ),
        ]);

        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(name_builder.finish()),
                Arc::new(emb_builder.finish()),
            ],
        )
        .unwrap()
    }

    fn make_labels(
        families: &[(&str, &str)],
        domains: &[(&str, Vec<&str>)],
        subjects: &[(&str, Vec<&str>)],
    ) -> LabelSet {
        let mut law_family = HashMap::new();
        for &(name, fam) in families {
            law_family.insert(name.to_string(), fam.to_string());
        }

        let mut law_domain = HashMap::new();
        for (name, doms) in domains {
            law_domain.insert(
                name.to_string(),
                doms.iter().map(|s| s.to_string()).collect(),
            );
        }

        let mut law_subjects = HashMap::new();
        for (name, subs) in subjects {
            law_subjects.insert(
                name.to_string(),
                subs.iter().map(|s| s.to_string()).collect(),
            );
        }

        LabelSet {
            law_family,
            law_sub_family: HashMap::new(),
            law_domain,
            law_subjects,
        }
    }

    #[test]
    fn aggregate_single_section_per_law() {
        let batch = text_batch(&[
            ("law_a", &[1.0, 0.0, 0.0, 0.0]),
            ("law_b", &[0.0, 1.0, 0.0, 0.0]),
        ]);

        let agg = aggregate_law_embeddings(&[batch]).unwrap();
        assert_eq!(agg.len(), 2);

        // Single section → same direction after normalize.
        assert!((agg["law_a"][0] - 1.0).abs() < 1e-5);
        assert!((agg["law_b"][1] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn aggregate_multiple_sections_mean_pools() {
        let batch = text_batch(&[
            ("law_a", &[1.0, 0.0, 0.0, 0.0]),
            ("law_a", &[0.0, 1.0, 0.0, 0.0]),
        ]);

        let agg = aggregate_law_embeddings(&[batch]).unwrap();
        assert_eq!(agg.len(), 1);

        // Mean of [1,0,0,0] and [0,1,0,0] = [0.5, 0.5, 0, 0], normalized.
        let v = &agg["law_a"];
        assert!((v[0] - v[1]).abs() < 1e-5, "components should be equal");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "should be unit norm");
    }

    #[test]
    fn aggregate_across_batches() {
        let batch1 = text_batch(&[("law_a", &[1.0, 0.0, 0.0, 0.0])]);
        let batch2 = text_batch(&[("law_a", &[0.0, 1.0, 0.0, 0.0])]);

        let agg = aggregate_law_embeddings(&[batch1, batch2]).unwrap();
        assert_eq!(agg.len(), 1);

        let v = &agg["law_a"];
        assert!((v[0] - v[1]).abs() < 1e-5);
    }

    #[test]
    fn aggregate_empty_batches() {
        let agg = aggregate_law_embeddings(&[]).unwrap();
        assert!(agg.is_empty());
    }

    #[test]
    fn build_classifier_family_centroids() {
        // Two laws in ENERGY (pointing +x), one in WASTE (pointing +y).
        let mut law_embs = HashMap::new();
        law_embs.insert("law_a".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
        law_embs.insert("law_b".to_string(), vec![0.9, 0.1, 0.0, 0.0]);
        law_embs.insert("law_c".to_string(), vec![0.0, 1.0, 0.0, 0.0]);

        let labels = make_labels(
            &[("law_a", "ENERGY"), ("law_b", "ENERGY"), ("law_c", "WASTE")],
            &[],
            &[],
        );

        let clf = Classifier::build(&law_embs, &labels);
        assert_eq!(clf.family_count(), 2);

        // ENERGY centroid should be roughly +x direction.
        let energy = &clf.family_centroids["ENERGY"];
        assert!(energy[0] > 0.9, "ENERGY centroid should point +x");

        // WASTE centroid should be +y direction.
        let waste = &clf.family_centroids["WASTE"];
        assert!(waste[1] > 0.9, "WASTE centroid should point +y");
    }

    #[test]
    fn build_excludes_noise_families() {
        let mut law_embs = HashMap::new();
        law_embs.insert("law_a".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
        law_embs.insert("law_b".to_string(), vec![0.0, 1.0, 0.0, 0.0]);

        let labels = make_labels(&[("law_a", "ENERGY"), ("law_b", "X: No Family")], &[], &[]);

        let clf = Classifier::build(&law_embs, &labels);
        assert_eq!(clf.family_count(), 1);
        assert!(clf.family_centroids.contains_key("ENERGY"));
        assert!(!clf.family_centroids.contains_key("X: No Family"));
    }

    #[test]
    fn domain_centroids_multi_select() {
        let mut law_embs = HashMap::new();
        law_embs.insert("law_a".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
        law_embs.insert("law_b".to_string(), vec![0.0, 1.0, 0.0, 0.0]);

        let labels = make_labels(
            &[],
            &[
                ("law_a", vec!["environment", "health_safety"]),
                ("law_b", vec!["environment"]),
            ],
            &[],
        );

        let clf = Classifier::build(&law_embs, &labels);
        assert_eq!(clf.domain_count(), 2);

        // environment centroid includes both law_a and law_b.
        assert!(clf.domain_centroids.contains_key("environment"));
        // health_safety centroid includes only law_a.
        assert!(clf.domain_centroids.contains_key("health_safety"));
    }

    #[test]
    fn classify_picks_nearest_family() {
        let mut law_embs = HashMap::new();
        law_embs.insert("law_a".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
        law_embs.insert("law_b".to_string(), vec![0.0, 1.0, 0.0, 0.0]);

        let labels = make_labels(
            &[("law_a", "ENERGY"), ("law_b", "WASTE")],
            &[
                ("law_a", vec!["environment"]),
                ("law_b", vec!["environment"]),
            ],
            &[],
        );

        let clf = Classifier::build(&law_embs, &labels);

        // A new law pointing mostly +x should classify as ENERGY.
        let result = clf.classify("law_new", &[0.95, 0.05, 0.0, 0.0], &labels, 0.3, 0.3);
        assert_eq!(result.family, "ENERGY");
        assert!(result.family_confidence > 0.9);
    }

    #[test]
    fn classify_domain_multi_select() {
        let mut law_embs = HashMap::new();
        // environment centroid → +x, health_safety centroid → +y
        law_embs.insert("env_law".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
        law_embs.insert("hs_law".to_string(), vec![0.0, 1.0, 0.0, 0.0]);

        let labels = make_labels(
            &[],
            &[
                ("env_law", vec!["environment"]),
                ("hs_law", vec!["health_safety"]),
            ],
            &[],
        );

        let clf = Classifier::build(&law_embs, &labels);

        // A law at 45 degrees between environment and health_safety.
        let diag: f32 = 1.0 / 2.0f32.sqrt();
        let result = clf.classify("mixed", &[diag, diag, 0.0, 0.0], &labels, 0.5, 0.3);

        // Both domains should be above 0.5 threshold (cosine sim ≈ 0.707).
        assert!(
            result.domain.len() >= 2,
            "expected both domains, got {:?}",
            result.domain
        );
    }

    #[test]
    fn classify_subjects_threshold() {
        let mut law_embs = HashMap::new();
        law_embs.insert("law_a".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
        law_embs.insert("law_b".to_string(), vec![0.0, 1.0, 0.0, 0.0]);

        let labels = make_labels(
            &[],
            &[],
            &[("law_a", vec!["pollution"]), ("law_b", vec!["smoke"])],
        );

        let clf = Classifier::build(&law_embs, &labels);

        // A law pointing +x should match "pollution" but not "smoke" at high threshold.
        let result = clf.classify("test", &[1.0, 0.0, 0.0, 0.0], &labels, 0.3, 0.8);
        assert!(
            result.subjects.iter().any(|(s, _)| s == "pollution"),
            "should match pollution"
        );
        assert!(
            !result.subjects.iter().any(|(s, _)| s == "smoke"),
            "should not match smoke at 0.8 threshold"
        );
    }

    // ── Status tests ──

    #[test]
    fn status_predicted_when_no_ground_truth() {
        let mut law_embs = HashMap::new();
        law_embs.insert("law_a".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
        law_embs.insert("law_b".to_string(), vec![0.0, 1.0, 0.0, 0.0]);

        let labels = make_labels(&[("law_a", "ENERGY"), ("law_b", "WASTE")], &[], &[]);

        let clf = Classifier::build(&law_embs, &labels);

        // law_new has no ground truth.
        let result = clf.classify("law_new", &[0.9, 0.1, 0.0, 0.0], &labels, 0.3, 0.3);
        assert_eq!(result.status, ClassificationStatus::Predicted);
    }

    #[test]
    fn status_confirmed_when_ai_agrees() {
        let mut law_embs = HashMap::new();
        law_embs.insert("law_a".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
        law_embs.insert("law_b".to_string(), vec![0.0, 1.0, 0.0, 0.0]);

        let labels = make_labels(&[("law_a", "ENERGY"), ("law_b", "WASTE")], &[], &[]);

        let clf = Classifier::build(&law_embs, &labels);

        // law_a has ground truth ENERGY, AI should agree.
        let result = clf.classify("law_a", &[1.0, 0.0, 0.0, 0.0], &labels, 0.3, 0.3);
        assert_eq!(result.family, "ENERGY");
        assert_eq!(result.status, ClassificationStatus::Confirmed);
    }

    #[test]
    fn status_conflict_when_ai_disagrees() {
        let mut law_embs = HashMap::new();
        law_embs.insert("law_a".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
        law_embs.insert("law_b".to_string(), vec![0.0, 1.0, 0.0, 0.0]);

        // law_a is labelled ENERGY, but we'll classify with a +y embedding
        // that matches WASTE centroid.
        let labels = make_labels(&[("law_a", "ENERGY"), ("law_b", "WASTE")], &[], &[]);

        let clf = Classifier::build(&law_embs, &labels);

        // Classify law_a with embedding that points toward WASTE.
        let result = clf.classify("law_a", &[0.0, 1.0, 0.0, 0.0], &labels, 0.3, 0.3);
        assert_eq!(result.family, "WASTE");
        assert_eq!(result.status, ClassificationStatus::Conflict);
    }

    #[test]
    fn classify_batch_includes_status() {
        let mut law_embs = HashMap::new();
        law_embs.insert("law_a".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
        law_embs.insert("law_b".to_string(), vec![0.0, 1.0, 0.0, 0.0]);
        law_embs.insert("law_new".to_string(), vec![0.9, 0.1, 0.0, 0.0]);

        let labels = make_labels(&[("law_a", "ENERGY"), ("law_b", "WASTE")], &[], &[]);

        let clf = Classifier::build(&law_embs, &labels);
        let results = clf.classify_batch(&law_embs, &labels, 0.3, 0.3);

        let by_name: HashMap<&str, &Classification> =
            results.iter().map(|c| (c.law_name.as_str(), c)).collect();

        assert_eq!(by_name["law_a"].family, "ENERGY");
        assert_eq!(by_name["law_a"].status, ClassificationStatus::Confirmed);

        assert_eq!(by_name["law_b"].family, "WASTE");
        assert_eq!(by_name["law_b"].status, ClassificationStatus::Confirmed);

        assert_eq!(by_name["law_new"].family, "ENERGY");
        assert_eq!(by_name["law_new"].status, ClassificationStatus::Predicted);
    }
}
