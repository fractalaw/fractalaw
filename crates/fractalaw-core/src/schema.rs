/// Arrow schema definitions for ESH regulatory data.
pub mod esh {
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    /// Schema for site compliance records.
    pub fn site_compliance_schema() -> Schema {
        Schema::new(vec![
            Field::new("site_id", DataType::Utf8, false),
            Field::new("site_name", DataType::Utf8, false),
            Field::new("region", DataType::Utf8, false),
            Field::new("compliance_score", DataType::Float64, true),
            Field::new("last_audit_date", DataType::Date32, true),
            Field::new(
                "active_permits",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
        ])
    }

    /// Schema for the immutable audit log.
    pub fn audit_log_schema() -> Schema {
        Schema::new(vec![
            Field::new("entry_id", DataType::UInt64, false),
            Field::new("timestamp", DataType::Timestamp(arrow::datatypes::TimeUnit::Nanosecond, Some("UTC".into())), false),
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

    #[test]
    fn site_compliance_schema_has_expected_fields() {
        let schema = esh::site_compliance_schema();
        assert_eq!(schema.fields().len(), 6);
        assert!(schema.field_with_name("site_id").is_ok());
        assert!(schema.field_with_name("compliance_score").is_ok());
    }

    #[test]
    fn audit_log_schema_has_expected_fields() {
        let schema = esh::audit_log_schema();
        assert_eq!(schema.fields().len(), 10);
        assert!(schema.field_with_name("prev_hash").is_ok());
    }
}
