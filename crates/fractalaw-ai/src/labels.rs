//! Ground-truth classification labels extracted from the legislation table.
//!
//! Labels are per-law metadata: family and sub_family are single-select,
//! domain and subjects are multi-select. Built from Arrow RecordBatches
//! returned by `DuckStore::query_arrow()`.

use std::collections::{HashMap, HashSet};

use arrow::array::{Array, LargeListArray, LargeStringArray, ListArray, StringArray};
use arrow::record_batch::RecordBatch;

/// Family labels to exclude from centroid computation (noise/placeholder values).
pub const EXCLUDE_FAMILIES: &[&str] = &["X: No Family", "_todo"];

/// Ground-truth classification labels keyed by law name.
///
/// Constructed from `SELECT name, domain, family, sub_family, subjects FROM legislation`.
pub struct LabelSet {
    /// law_name → family (single-select)
    pub law_family: HashMap<String, String>,
    /// law_name → sub_family (single-select)
    pub law_sub_family: HashMap<String, String>,
    /// law_name → domain values (multi-select)
    pub law_domain: HashMap<String, Vec<String>>,
    /// law_name → subject tags (multi-select)
    pub law_subjects: HashMap<String, Vec<String>>,
}

/// Summary statistics for a LabelSet.
pub struct LabelSummary {
    pub total_laws: usize,
    pub with_family: usize,
    pub with_sub_family: usize,
    pub with_domain: usize,
    pub with_subjects: usize,
    pub distinct_families: usize,
    pub distinct_sub_families: usize,
    pub distinct_domains: usize,
    pub distinct_subjects: usize,
}

impl LabelSet {
    /// Build a LabelSet from legislation table Arrow batches.
    ///
    /// Expects columns: `name`, `domain`, `family`, `sub_family`, `subjects`.
    pub fn from_legislation_batches(batches: &[RecordBatch]) -> anyhow::Result<Self> {
        let mut law_family = HashMap::new();
        let mut law_sub_family = HashMap::new();
        let mut law_domain = HashMap::new();
        let mut law_subjects = HashMap::new();

        for batch in batches {
            let name_col = batch
                .column_by_name("name")
                .ok_or_else(|| anyhow::anyhow!("missing 'name' column"))?;
            let family_col = batch.column_by_name("family");
            let sub_family_col = batch.column_by_name("sub_family");
            let domain_col = batch.column_by_name("domain");
            let subjects_col = batch.column_by_name("subjects");

            for row in 0..batch.num_rows() {
                let name = get_string(name_col.as_ref(), row)
                    .ok_or_else(|| anyhow::anyhow!("null name at row {row}"))?;

                if let Some(col) = family_col
                    && let Some(v) = get_string(col.as_ref(), row)
                {
                    law_family.insert(name.clone(), v);
                }

                if let Some(col) = sub_family_col
                    && let Some(v) = get_string(col.as_ref(), row)
                {
                    law_sub_family.insert(name.clone(), v);
                }

                if let Some(col) = domain_col
                    && let Some(values) = get_string_list(col.as_ref(), row)
                    && !values.is_empty()
                {
                    law_domain.insert(name.clone(), values);
                }

                if let Some(col) = subjects_col
                    && let Some(values) = get_string_list(col.as_ref(), row)
                    && !values.is_empty()
                {
                    law_subjects.insert(name.clone(), values);
                }
            }
        }

        Ok(Self {
            law_family,
            law_sub_family,
            law_domain,
            law_subjects,
        })
    }

    /// Summary statistics.
    pub fn summary(&self) -> LabelSummary {
        let mut all_laws = HashSet::new();
        all_laws.extend(self.law_family.keys());
        all_laws.extend(self.law_sub_family.keys());
        all_laws.extend(self.law_domain.keys());
        all_laws.extend(self.law_subjects.keys());

        let distinct_families: HashSet<&str> =
            self.law_family.values().map(|s| s.as_str()).collect();
        let distinct_sub_families: HashSet<&str> =
            self.law_sub_family.values().map(|s| s.as_str()).collect();
        let distinct_domains: HashSet<&str> = self
            .law_domain
            .values()
            .flat_map(|v| v.iter().map(|s| s.as_str()))
            .collect();
        let distinct_subjects: HashSet<&str> = self
            .law_subjects
            .values()
            .flat_map(|v| v.iter().map(|s| s.as_str()))
            .collect();

        LabelSummary {
            total_laws: all_laws.len(),
            with_family: self.law_family.len(),
            with_sub_family: self.law_sub_family.len(),
            with_domain: self.law_domain.len(),
            with_subjects: self.law_subjects.len(),
            distinct_families: distinct_families.len(),
            distinct_sub_families: distinct_sub_families.len(),
            distinct_domains: distinct_domains.len(),
            distinct_subjects: distinct_subjects.len(),
        }
    }

    /// Iterate over laws with family labels (for centroid computation).
    ///
    /// Yields `(law_name, family)` pairs. Use [`EXCLUDE_FAMILIES`] to filter
    /// noise labels like `X: No Family` and `_todo`.
    pub fn labelled_laws(&self) -> impl Iterator<Item = (&str, &str)> {
        self.law_family
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Iterate over laws with family labels, excluding noise families.
    pub fn trainable_laws(&self) -> impl Iterator<Item = (&str, &str)> {
        self.law_family
            .iter()
            .filter(|(_, v)| !EXCLUDE_FAMILIES.contains(&v.as_str()))
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

// ── Arrow extraction helpers ──

/// Extract a string value from an Arrow array (handles Utf8 and LargeUtf8).
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

/// Extract a list of strings from a List or LargeList column.
fn get_string_list(col: &dyn Array, row: usize) -> Option<Vec<String>> {
    if col.is_null(row) {
        return None;
    }

    if let Some(list) = col.as_any().downcast_ref::<ListArray>() {
        return Some(strings_from_array(list.value(row).as_ref()));
    }
    if let Some(list) = col.as_any().downcast_ref::<LargeListArray>() {
        return Some(strings_from_array(list.value(row).as_ref()));
    }

    None
}

fn strings_from_array(arr: &dyn Array) -> Vec<String> {
    let mut out = Vec::with_capacity(arr.len());
    if let Some(a) = arr.as_any().downcast_ref::<StringArray>() {
        for i in 0..a.len() {
            if !a.is_null(i) {
                out.push(a.value(i).to_string());
            }
        }
    } else if let Some(a) = arr.as_any().downcast_ref::<LargeStringArray>() {
        for i in 0..a.len() {
            if !a.is_null(i) {
                out.push(a.value(i).to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ListBuilder, StringBuilder};
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    /// Build a test batch with the expected columns.
    fn test_batch(
        names: &[&str],
        families: &[Option<&str>],
        sub_families: &[Option<&str>],
        domains: &[Option<Vec<&str>>],
        subjects: &[Option<Vec<&str>>],
    ) -> RecordBatch {
        let n = names.len();

        let name_arr = StringArray::from(names.to_vec());

        let family_arr = StringArray::from(
            families
                .iter()
                .map(|o| o.map(|s| s.to_string()))
                .collect::<Vec<_>>(),
        );

        let sub_family_arr = StringArray::from(
            sub_families
                .iter()
                .map(|o| o.map(|s| s.to_string()))
                .collect::<Vec<_>>(),
        );

        let mut domain_builder = ListBuilder::new(StringBuilder::new());
        for d in domains.iter().take(n) {
            match d {
                Some(vals) => {
                    for v in vals {
                        domain_builder.values().append_value(v);
                    }
                    domain_builder.append(true);
                }
                None => domain_builder.append(false),
            }
        }
        let domain_arr = domain_builder.finish();

        let mut subjects_builder = ListBuilder::new(StringBuilder::new());
        for s in subjects.iter().take(n) {
            match s {
                Some(vals) => {
                    for v in vals {
                        subjects_builder.values().append_value(v);
                    }
                    subjects_builder.append(true);
                }
                None => subjects_builder.append(false),
            }
        }
        let subjects_arr = subjects_builder.finish();

        let schema = Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("family", DataType::Utf8, true),
            Field::new("sub_family", DataType::Utf8, true),
            Field::new(
                "domain",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
            Field::new(
                "subjects",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
        ]);

        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(name_arr),
                Arc::new(family_arr),
                Arc::new(sub_family_arr),
                Arc::new(domain_arr),
                Arc::new(subjects_arr),
            ],
        )
        .unwrap()
    }

    #[test]
    fn extracts_family_labels() {
        let batch = test_batch(
            &["law_a", "law_b", "law_c"],
            &[Some("ENERGY"), None, Some("WASTE")],
            &[None, None, None],
            &[None, None, None],
            &[None, None, None],
        );

        let labels = LabelSet::from_legislation_batches(&[batch]).unwrap();
        assert_eq!(labels.law_family.len(), 2);
        assert_eq!(labels.law_family["law_a"], "ENERGY");
        assert_eq!(labels.law_family["law_c"], "WASTE");
        assert!(!labels.law_family.contains_key("law_b"));
    }

    #[test]
    fn extracts_sub_family_labels() {
        let batch = test_batch(
            &["law_a", "law_b"],
            &[Some("ENERGY"), Some("FIRE")],
            &[Some("RENEWABLE"), None],
            &[None, None],
            &[None, None],
        );

        let labels = LabelSet::from_legislation_batches(&[batch]).unwrap();
        assert_eq!(labels.law_sub_family.len(), 1);
        assert_eq!(labels.law_sub_family["law_a"], "RENEWABLE");
    }

    #[test]
    fn extracts_domain_multi_select() {
        let batch = test_batch(
            &["law_a", "law_b"],
            &[None, None],
            &[None, None],
            &[
                Some(vec!["environment", "health_safety"]),
                Some(vec!["governance"]),
            ],
            &[None, None],
        );

        let labels = LabelSet::from_legislation_batches(&[batch]).unwrap();
        assert_eq!(labels.law_domain.len(), 2);
        assert_eq!(
            labels.law_domain["law_a"],
            vec!["environment", "health_safety"]
        );
        assert_eq!(labels.law_domain["law_b"], vec!["governance"]);
    }

    #[test]
    fn extracts_subjects_multi_select() {
        let batch = test_batch(
            &["law_a", "law_b"],
            &[None, None],
            &[None, None],
            &[None, None],
            &[Some(vec!["pollution", "water pollution"]), None],
        );

        let labels = LabelSet::from_legislation_batches(&[batch]).unwrap();
        assert_eq!(labels.law_subjects.len(), 1);
        assert_eq!(
            labels.law_subjects["law_a"],
            vec!["pollution", "water pollution"]
        );
    }

    #[test]
    fn skips_null_and_empty_lists() {
        let batch = test_batch(
            &["law_a", "law_b"],
            &[None, None],
            &[None, None],
            &[None, Some(vec![])],
            &[Some(vec![]), None],
        );

        let labels = LabelSet::from_legislation_batches(&[batch]).unwrap();
        assert!(labels.law_domain.is_empty());
        assert!(labels.law_subjects.is_empty());
    }

    #[test]
    fn summary_counts() {
        let batch = test_batch(
            &["law_a", "law_b", "law_c"],
            &[Some("ENERGY"), Some("ENERGY"), Some("WASTE")],
            &[Some("RENEWABLE"), None, None],
            &[
                Some(vec!["environment"]),
                Some(vec!["environment", "health_safety"]),
                None,
            ],
            &[
                Some(vec!["pollution"]),
                None,
                Some(vec!["pollution", "smoke"]),
            ],
        );

        let labels = LabelSet::from_legislation_batches(&[batch]).unwrap();
        let s = labels.summary();
        assert_eq!(s.total_laws, 3);
        assert_eq!(s.with_family, 3);
        assert_eq!(s.with_sub_family, 1);
        assert_eq!(s.with_domain, 2);
        assert_eq!(s.with_subjects, 2);
        assert_eq!(s.distinct_families, 2); // ENERGY, WASTE
        assert_eq!(s.distinct_sub_families, 1); // RENEWABLE
        assert_eq!(s.distinct_domains, 2); // environment, health_safety
        assert_eq!(s.distinct_subjects, 2); // pollution, smoke
    }

    #[test]
    fn trainable_laws_excludes_noise() {
        let batch = test_batch(
            &["law_a", "law_b", "law_c", "law_d"],
            &[
                Some("ENERGY"),
                Some("X: No Family"),
                Some("_todo"),
                Some("WASTE"),
            ],
            &[None, None, None, None],
            &[None, None, None, None],
            &[None, None, None, None],
        );

        let labels = LabelSet::from_legislation_batches(&[batch]).unwrap();
        let trainable: Vec<_> = labels.trainable_laws().collect();
        assert_eq!(trainable.len(), 2);
        assert!(trainable.iter().any(|(name, _)| *name == "law_a"));
        assert!(trainable.iter().any(|(name, _)| *name == "law_d"));
    }

    #[test]
    fn multiple_batches() {
        let batch1 = test_batch(&["law_a"], &[Some("ENERGY")], &[None], &[None], &[None]);
        let batch2 = test_batch(&["law_b"], &[Some("WASTE")], &[None], &[None], &[None]);

        let labels = LabelSet::from_legislation_batches(&[batch1, batch2]).unwrap();
        assert_eq!(labels.law_family.len(), 2);
        assert_eq!(labels.law_family["law_a"], "ENERGY");
        assert_eq!(labels.law_family["law_b"], "WASTE");
    }

    #[test]
    fn empty_batches() {
        let labels = LabelSet::from_legislation_batches(&[]).unwrap();
        assert!(labels.law_family.is_empty());
        assert!(labels.law_domain.is_empty());
    }
}
