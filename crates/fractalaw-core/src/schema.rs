/// Arrow schema definitions for ESH regulatory data.
pub mod esh {
    use arrow::datatypes::{DataType, Field, Fields, Schema, TimeUnit};
    use std::sync::Arc;

    /// Struct fields for `RelatedLaw` — used in relationship `List<Struct>` columns.
    fn related_law_struct() -> Fields {
        Fields::from(vec![
            Field::new("name", DataType::Utf8, true),
            Field::new("title", DataType::Utf8, true),
            Field::new("year", DataType::Int32, true),
            Field::new("count", DataType::Int32, true),
            Field::new("latest_date", DataType::Date32, true),
        ])
    }

    /// Struct fields for `DRRPEntry` — used in DRRP detail `List<Struct>` columns.
    fn drrp_entry_struct() -> Fields {
        Fields::from(vec![
            Field::new("holder", DataType::Utf8, true),
            Field::new("duty_type", DataType::Utf8, true),
            Field::new("clause", DataType::Utf8, true),
            Field::new("article", DataType::Utf8, true),
        ])
    }

    /// Timestamp(Nanosecond, UTC) — the standard timestamp type for all tables.
    fn timestamp_ns_utc() -> DataType {
        DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into()))
    }

    /// Schema for the `legislation` table (LRT) — hot path.
    ///
    /// One row per law. 78 columns covering identity, classification, dates,
    /// territorial extent, document stats, status, function, denormalized
    /// relationships, DRRP taxa, annotation totals, change logs, and timestamps.
    pub fn legislation_schema() -> Schema {
        let list_utf8 = DataType::List(Arc::new(Field::new("item", DataType::Utf8, true)));
        let list_related_law = DataType::List(Arc::new(Field::new(
            "item",
            DataType::Struct(related_law_struct()),
            true,
        )));
        let list_drrp_entry = DataType::List(Arc::new(Field::new(
            "item",
            DataType::Struct(drrp_entry_struct()),
            true,
        )));

        Schema::new(vec![
            // 1.1 Identity (12)
            Field::new("name", DataType::Utf8, false),
            Field::new("jurisdiction", DataType::Utf8, false),
            Field::new("source_authority", DataType::Utf8, false),
            Field::new("source_url", DataType::Utf8, true),
            Field::new("type_code", DataType::Utf8, false),
            Field::new("type_desc", DataType::Utf8, true),
            Field::new("type_class", DataType::Utf8, true),
            Field::new("year", DataType::Int32, false),
            Field::new("number", DataType::Utf8, false),
            Field::new("old_style_number", DataType::Utf8, true),
            Field::new("title", DataType::Utf8, true),
            Field::new("language", DataType::Utf8, false),
            // 1.2 Classification (6)
            Field::new("domain", list_utf8.clone(), true),
            Field::new("family", DataType::Utf8, true),
            Field::new("sub_family", DataType::Utf8, true),
            Field::new("si_code", list_utf8.clone(), true),
            Field::new("description", DataType::Utf8, true),
            Field::new("subjects", list_utf8.clone(), true),
            // 1.3 Dates (9)
            Field::new("primary_date", DataType::Date32, true),
            Field::new("made_date", DataType::Date32, true),
            Field::new("enactment_date", DataType::Date32, true),
            Field::new("in_force_date", DataType::Date32, true),
            Field::new("valid_date", DataType::Date32, true),
            Field::new("modified_date", DataType::Date32, true),
            Field::new("restrict_start_date", DataType::Date32, true),
            Field::new("latest_amend_date", DataType::Date32, true),
            Field::new("latest_rescind_date", DataType::Date32, true),
            // 1.4 Territorial Extent (5)
            Field::new("extent_code", DataType::Utf8, true),
            Field::new("extent_regions", list_utf8.clone(), true),
            Field::new("extent_national", DataType::Boolean, true),
            Field::new("extent_detail", DataType::Utf8, true),
            Field::new("restrict_extent", DataType::Utf8, true),
            // 1.5 Document Statistics (5)
            Field::new("total_paras", DataType::Int32, true),
            Field::new("body_paras", DataType::Int32, true),
            Field::new("schedule_paras", DataType::Int32, true),
            Field::new("attachment_paras", DataType::Int32, true),
            Field::new("images", DataType::Int32, true),
            // 1.6 Status (4)
            Field::new("status", DataType::Utf8, true),
            Field::new("status_source", DataType::Utf8, true),
            Field::new("status_conflict", DataType::Boolean, true),
            Field::new("status_conflict_detail", DataType::Utf8, true),
            // 1.7 Function (6)
            Field::new("function", list_utf8.clone(), true),
            Field::new("is_making", DataType::Boolean, true),
            Field::new("is_commencing", DataType::Boolean, true),
            Field::new("is_amending", DataType::Boolean, true),
            Field::new("is_enacting", DataType::Boolean, true),
            Field::new("is_rescinding", DataType::Boolean, true),
            // 1.8 Relationships (6)
            Field::new("enacted_by", list_related_law.clone(), true),
            Field::new("enacting", list_related_law.clone(), true),
            Field::new("amending", list_related_law.clone(), true),
            Field::new("amended_by", list_related_law.clone(), true),
            Field::new("rescinding", list_related_law.clone(), true),
            Field::new("rescinded_by", list_related_law, true),
            // 1.8 Amendment Statistics (7)
            Field::new("self_affects_count", DataType::Int32, true),
            Field::new("affects_count", DataType::Int32, true),
            Field::new("affected_laws_count", DataType::Int32, true),
            Field::new("affected_by_count", DataType::Int32, true),
            Field::new("affected_by_laws_count", DataType::Int32, true),
            Field::new("rescinding_laws_count", DataType::Int32, true),
            Field::new("rescinded_by_laws_count", DataType::Int32, true),
            // 1.9 DRRP Taxa (11)
            Field::new("duty_holder", list_utf8.clone(), true),
            Field::new("rights_holder", list_utf8.clone(), true),
            Field::new("responsibility_holder", list_utf8.clone(), true),
            Field::new("power_holder", list_utf8.clone(), true),
            Field::new("duty_type", list_utf8.clone(), true),
            Field::new("role", list_utf8.clone(), true),
            Field::new("role_gvt", list_utf8, true),
            Field::new("duties", list_drrp_entry.clone(), true),
            Field::new("rights", list_drrp_entry.clone(), true),
            Field::new("responsibilities", list_drrp_entry.clone(), true),
            Field::new("powers", list_drrp_entry, true),
            // 1.10 Annotation Totals (4)
            Field::new("total_text_amendments", DataType::Int32, true),
            Field::new("total_modifications", DataType::Int32, true),
            Field::new("total_commencements", DataType::Int32, true),
            Field::new("total_extents", DataType::Int32, true),
            // 1.11 Change Logs (1)
            Field::new("change_log", DataType::Utf8, true),
            // 1.12 Timestamps (2)
            Field::new("created_at", timestamp_ns_utc(), false),
            Field::new("updated_at", timestamp_ns_utc(), false),
        ])
    }

    /// Schema for the `law_edges` table — analytical path.
    ///
    /// Flattened edge table derived from the LRT's relationship columns.
    /// One row per directional relationship between two laws.
    pub fn law_edges_schema() -> Schema {
        Schema::new(vec![
            Field::new("source_name", DataType::Utf8, false),
            Field::new("target_name", DataType::Utf8, false),
            Field::new("edge_type", DataType::Utf8, false),
            Field::new("jurisdiction", DataType::Utf8, false),
            Field::new("article_target", DataType::Utf8, true),
            Field::new("affect_type", DataType::Utf8, true),
            Field::new("applied_status", DataType::Utf8, true),
            Field::new("date", DataType::Date32, true),
        ])
    }

    /// Schema for the `legislation_text` table (LAT) — semantic path.
    ///
    /// One row per structural unit of legal text (article, section, paragraph, etc.).
    /// Stored in LanceDB for semantic search and embedding similarity.
    ///
    /// Identity uses a three-column design:
    /// - `section_id`: structural citation (`{law_name}:s.25A(1)`) — stable across amendments
    /// - `sort_key`: normalised lexicographic encoding for correct document order
    /// - `position`: snapshot integer index (1-based), reassigned on re-export
    pub fn legislation_text_schema() -> Schema {
        Schema::new(vec![
            // 3.1 Identity & Position (7)
            Field::new("law_name", DataType::Utf8, false),
            Field::new("section_id", DataType::Utf8, false),
            Field::new("sort_key", DataType::Utf8, false),
            Field::new("position", DataType::Int32, false),
            Field::new("section_type", DataType::Utf8, false),
            Field::new("hierarchy_path", DataType::Utf8, true),
            Field::new("depth", DataType::Int32, false),
            // 3.2 Structural Hierarchy (7)
            Field::new("part", DataType::Utf8, true),
            Field::new("chapter", DataType::Utf8, true),
            Field::new("heading_group", DataType::Utf8, true),
            Field::new("provision", DataType::Utf8, true),
            Field::new("paragraph", DataType::Utf8, true),
            Field::new("sub_paragraph", DataType::Utf8, true),
            Field::new("schedule", DataType::Utf8, true),
            // 3.3 Content (3)
            Field::new("text", DataType::Utf8, false),
            Field::new("language", DataType::Utf8, false),
            Field::new("extent_code", DataType::Utf8, true),
            // 3.4 Amendment Annotations (5)
            Field::new("amendment_count", DataType::Int32, true),
            Field::new("modification_count", DataType::Int32, true),
            Field::new("commencement_count", DataType::Int32, true),
            Field::new("extent_count", DataType::Int32, true),
            Field::new("editorial_count", DataType::Int32, true),
            // 3.5 Embeddings (3)
            Field::new(
                "embedding",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 384),
                true,
            ),
            Field::new("embedding_model", DataType::Utf8, true),
            Field::new("embedded_at", timestamp_ns_utc(), true),
            // 3.6 Pre-tokenized Text (2)
            Field::new(
                "token_ids",
                DataType::List(Arc::new(Field::new("item", DataType::UInt32, false))),
                true,
            ),
            Field::new("tokenizer_model", DataType::Utf8, true),
            // 3.7 Migration (1)
            Field::new("legacy_id", DataType::Utf8, true),
            // 3.8 Metadata (2)
            Field::new("created_at", timestamp_ns_utc(), false),
            Field::new("updated_at", timestamp_ns_utc(), false),
        ])
    }

    /// Schema for the `amendment_annotations` table — semantic path.
    ///
    /// One row per legislative change annotation (F/C/I/E codes).
    /// Links amendment footnotes to the LAT sections they affect.
    ///
    /// Annotation `id` is a synthetic key: `{law_name}:{code_type}:{seq}` where
    /// `seq` is a per-law, per-code_type counter assigned during export.
    pub fn amendment_annotations_schema() -> Schema {
        Schema::new(vec![
            // 4.1 Identity (5)
            Field::new("id", DataType::Utf8, false),
            Field::new("law_name", DataType::Utf8, false),
            Field::new("code", DataType::Utf8, false),
            Field::new("code_type", DataType::Utf8, false),
            Field::new("source", DataType::Utf8, false),
            // 4.2 Content (2)
            Field::new("text", DataType::Utf8, false),
            Field::new(
                "affected_sections",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
            // 4.3 Metadata (2)
            Field::new("created_at", timestamp_ns_utc(), false),
            Field::new("updated_at", timestamp_ns_utc(), false),
        ])
    }

    /// Schema for the immutable audit log (fractal:audit WIT interface).
    pub fn audit_log_schema() -> Schema {
        Schema::new(vec![
            Field::new("entry_id", DataType::UInt64, false),
            Field::new("timestamp", timestamp_ns_utc(), false),
            Field::new("node_id", DataType::Utf8, false),
            Field::new("actor_id", DataType::Utf8, false),
            Field::new("actor_role", DataType::Utf8, false),
            Field::new("event_type", DataType::Utf8, false),
            Field::new("resource", DataType::Utf8, true),
            Field::new("detail", DataType::Utf8, true),
            Field::new("prev_hash", DataType::FixedSizeBinary(32), false),
            Field::new("signature", DataType::FixedSizeBinary(64), true),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::esh;
    use arrow::datatypes::DataType;

    // ── Field counts ──

    #[test]
    fn legislation_schema_field_count() {
        assert_eq!(esh::legislation_schema().fields().len(), 78);
    }

    #[test]
    fn law_edges_schema_field_count() {
        assert_eq!(esh::law_edges_schema().fields().len(), 8);
    }

    #[test]
    fn legislation_text_schema_field_count() {
        assert_eq!(esh::legislation_text_schema().fields().len(), 30);
    }

    #[test]
    fn amendment_annotations_schema_field_count() {
        assert_eq!(esh::amendment_annotations_schema().fields().len(), 9);
    }

    #[test]
    fn audit_log_schema_field_count() {
        assert_eq!(esh::audit_log_schema().fields().len(), 10);
    }

    // ── Nullability ──

    #[test]
    fn legislation_name_not_nullable() {
        let schema = esh::legislation_schema();
        let field = schema.field_with_name("name").unwrap();
        assert!(!field.is_nullable());
    }

    #[test]
    fn legislation_extent_code_nullable() {
        let schema = esh::legislation_schema();
        let field = schema.field_with_name("extent_code").unwrap();
        assert!(field.is_nullable());
    }

    #[test]
    fn legislation_timestamps_not_nullable() {
        let schema = esh::legislation_schema();
        assert!(!schema.field_with_name("created_at").unwrap().is_nullable());
        assert!(!schema.field_with_name("updated_at").unwrap().is_nullable());
    }

    #[test]
    fn law_edges_source_target_not_nullable() {
        let schema = esh::law_edges_schema();
        assert!(!schema.field_with_name("source_name").unwrap().is_nullable());
        assert!(!schema.field_with_name("target_name").unwrap().is_nullable());
    }

    #[test]
    fn legislation_text_identity_not_nullable() {
        let schema = esh::legislation_text_schema();
        assert!(!schema.field_with_name("section_id").unwrap().is_nullable());
        assert!(!schema.field_with_name("sort_key").unwrap().is_nullable());
        assert!(!schema.field_with_name("position").unwrap().is_nullable());
        assert!(!schema.field_with_name("text").unwrap().is_nullable());
    }

    #[test]
    fn legislation_text_hierarchy_path_nullable() {
        let schema = esh::legislation_text_schema();
        assert!(
            schema
                .field_with_name("hierarchy_path")
                .unwrap()
                .is_nullable()
        );
    }

    #[test]
    fn legislation_text_legacy_id_nullable() {
        let schema = esh::legislation_text_schema();
        assert!(schema.field_with_name("legacy_id").unwrap().is_nullable());
    }

    #[test]
    fn legislation_text_has_provision_not_section_article() {
        let schema = esh::legislation_text_schema();
        assert!(schema.field_with_name("provision").is_ok());
        assert!(schema.field_with_name("section").is_err());
        assert!(schema.field_with_name("article").is_err());
    }

    #[test]
    fn legislation_text_has_heading_group_not_heading() {
        let schema = esh::legislation_text_schema();
        assert!(schema.field_with_name("heading_group").is_ok());
        assert!(schema.field_with_name("heading").is_err());
    }

    #[test]
    fn annotation_source_not_nullable() {
        let schema = esh::amendment_annotations_schema();
        assert!(!schema.field_with_name("source").unwrap().is_nullable());
    }

    // ── List<Struct> type assertions ──

    #[test]
    fn related_law_struct_has_5_fields() {
        let schema = esh::legislation_schema();
        let field = schema.field_with_name("amended_by").unwrap();
        match field.data_type() {
            DataType::List(inner) => match inner.data_type() {
                DataType::Struct(fields) => {
                    assert_eq!(fields.len(), 5);
                    assert!(fields.iter().any(|f| f.name() == "name"));
                    assert!(fields.iter().any(|f| f.name() == "title"));
                    assert!(fields.iter().any(|f| f.name() == "year"));
                    assert!(fields.iter().any(|f| f.name() == "count"));
                    assert!(fields.iter().any(|f| f.name() == "latest_date"));
                }
                other => panic!("expected Struct, got {other:?}"),
            },
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn drrp_entry_struct_has_4_fields() {
        let schema = esh::legislation_schema();
        let field = schema.field_with_name("duties").unwrap();
        match field.data_type() {
            DataType::List(inner) => match inner.data_type() {
                DataType::Struct(fields) => {
                    assert_eq!(fields.len(), 4);
                    assert!(fields.iter().any(|f| f.name() == "holder"));
                    assert!(fields.iter().any(|f| f.name() == "duty_type"));
                    assert!(fields.iter().any(|f| f.name() == "clause"));
                    assert!(fields.iter().any(|f| f.name() == "article"));
                }
                other => panic!("expected Struct, got {other:?}"),
            },
            other => panic!("expected List, got {other:?}"),
        }
    }

    // ── Embedding type ──

    #[test]
    fn embedding_is_fixed_size_list_f32_384() {
        let schema = esh::legislation_text_schema();
        let field = schema.field_with_name("embedding").unwrap();
        match field.data_type() {
            DataType::FixedSizeList(inner, 384) => {
                assert_eq!(*inner.data_type(), DataType::Float32);
            }
            other => panic!("expected FixedSizeList<Float32, 384>, got {other:?}"),
        }
    }

    // ── Timestamp type ──

    #[test]
    fn timestamps_are_nanosecond_utc() {
        let schema = esh::legislation_schema();
        let field = schema.field_with_name("created_at").unwrap();
        match field.data_type() {
            DataType::Timestamp(arrow::datatypes::TimeUnit::Nanosecond, Some(tz)) => {
                assert_eq!(tz.as_ref(), "UTC");
            }
            other => panic!("expected Timestamp(ns, UTC), got {other:?}"),
        }
    }

    // ── All six relationship columns use RelatedLaw ──

    #[test]
    fn all_relationship_columns_use_related_law() {
        let schema = esh::legislation_schema();
        for col in [
            "enacted_by",
            "enacting",
            "amending",
            "amended_by",
            "rescinding",
            "rescinded_by",
        ] {
            let field = schema.field_with_name(col).unwrap();
            match field.data_type() {
                DataType::List(inner) => match inner.data_type() {
                    DataType::Struct(fields) => {
                        assert_eq!(fields.len(), 5, "{col} should have 5 struct fields");
                    }
                    other => panic!("{col}: expected Struct, got {other:?}"),
                },
                other => panic!("{col}: expected List, got {other:?}"),
            }
        }
    }

    // ── All four DRRP detail columns use DRRPEntry ──

    #[test]
    fn all_drrp_columns_use_drrp_entry() {
        let schema = esh::legislation_schema();
        for col in ["duties", "rights", "responsibilities", "powers"] {
            let field = schema.field_with_name(col).unwrap();
            match field.data_type() {
                DataType::List(inner) => match inner.data_type() {
                    DataType::Struct(fields) => {
                        assert_eq!(fields.len(), 4, "{col} should have 4 struct fields");
                    }
                    other => panic!("{col}: expected Struct, got {other:?}"),
                },
                other => panic!("{col}: expected List, got {other:?}"),
            }
        }
    }
}
