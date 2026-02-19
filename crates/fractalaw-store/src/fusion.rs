//! DataFusion unified query layer over DuckDB tables.
//!
//! `FusionStore` wraps a `DuckStore` and registers its tables as DataFusion
//! `TableProvider`s, enabling standard SQL queries across both `legislation`
//! and `law_edges` through a single `SessionContext`.

use std::any::Any;
use std::sync::{Arc, Mutex};

use arrow::array::{ArrayRef, StringArray};
use arrow::datatypes::{DataType, SchemaRef};
use arrow::record_batch::RecordBatch;
use datafusion::catalog::{Session, TableProvider};
use datafusion::datasource::memory::MemTable;
use datafusion::error::DataFusionError;
use datafusion::logical_expr::TableType;
use datafusion::logical_expr::{Expr, Volatility};
use datafusion::prelude::SessionContext;
use duckdb::Connection;
use tracing::info;

use crate::{DuckStore, StoreError};

// ── DuckTableProvider ──

/// A DataFusion `TableProvider` backed by a DuckDB table.
///
/// On each `scan()`, executes a SQL query against DuckDB with projection and
/// limit pushdown, collects the results as Arrow RecordBatches, and returns
/// them as a MemTable-backed execution plan.
#[derive(Debug)]
struct DuckTableProvider {
    table_name: String,
    schema: SchemaRef,
    conn: Mutex<Connection>,
}

impl DuckTableProvider {
    /// Create a provider for the given table, introspecting DuckDB for the schema.
    fn new(table_name: &str, conn: Connection) -> Result<Self, StoreError> {
        let schema = {
            let sql = format!("SELECT * FROM {table_name} LIMIT 0");
            let mut stmt = conn.prepare(&sql)?;
            let arrow = stmt.query_arrow([])?;
            arrow.get_schema()
        };
        Ok(Self {
            table_name: table_name.to_string(),
            schema,
            conn: Mutex::new(conn),
        })
    }
}

#[async_trait::async_trait]
impl TableProvider for DuckTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn datafusion::physical_plan::ExecutionPlan>> {
        // Build column list for projection pushdown.
        // Non-empty projection: push down to DuckDB for efficiency.
        // Empty or no projection: fetch all columns, let MemTable project.
        let (push_projection, columns) = match projection {
            Some(indices) if !indices.is_empty() => {
                let names: Vec<&str> = indices
                    .iter()
                    .map(|&i| self.schema.field(i).name().as_str())
                    .collect();
                (true, names.join(", "))
            }
            _ => (false, "*".to_string()),
        };

        let mut sql = format!("SELECT {columns} FROM {}", self.table_name);

        // Limit pushdown.
        if let Some(n) = limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }

        // Execute against DuckDB under the mutex.
        let batches = {
            let conn = self.conn.lock().map_err(|e| {
                DataFusionError::External(Box::new(StoreError::Other(format!(
                    "mutex poisoned: {e}"
                ))))
            })?;
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| DataFusionError::External(Box::new(e)))?;
            let result: Vec<RecordBatch> = stmt
                .query_arrow([])
                .map_err(|e| DataFusionError::External(Box::new(e)))?
                .collect();
            result
        };

        // Use the full table schema when we fetched all columns,
        // or the batch schema when we pushed projection to DuckDB.
        let mem_schema = if push_projection {
            batches
                .first()
                .map(|b| b.schema())
                .unwrap_or_else(|| Arc::clone(&self.schema))
        } else {
            Arc::clone(&self.schema)
        };

        // Wrap in MemTable. When projection was not pushed to DuckDB,
        // pass it through so MemTable applies it (handles empty projection for count(*)).
        let mem = MemTable::try_new(mem_schema, vec![batches])?;
        let mem_projection = if push_projection { None } else { projection };
        mem.scan(state, mem_projection, filters, None).await
    }
}

// ── FusionStore ──

/// Unified DataFusion query layer over DuckDB tables.
///
/// Registers `legislation` and `law_edges` as DataFusion table providers,
/// enabling standard SQL queries through a `SessionContext`. Includes
/// `law_status()` and `edge_type_label()` scalar UDFs.
pub struct FusionStore {
    ctx: SessionContext,
}

impl FusionStore {
    /// Create a `FusionStore` from a loaded `DuckStore`.
    ///
    /// Clones the DuckDB connection for each table provider and registers
    /// both `legislation` and `law_edges` as DataFusion tables.
    pub fn new(store: &DuckStore) -> Result<Self, StoreError> {
        let ctx = SessionContext::new();

        // Register legislation table.
        let leg_conn = store.connection().try_clone()?;
        let leg_provider = DuckTableProvider::new("legislation", leg_conn)?;
        ctx.register_table("legislation", Arc::new(leg_provider))
            .map_err(|e| StoreError::Other(format!("register legislation: {e}")))?;

        // Register law_edges table.
        let edges_conn = store.connection().try_clone()?;
        let edges_provider = DuckTableProvider::new("law_edges", edges_conn)?;
        ctx.register_table("law_edges", Arc::new(edges_provider))
            .map_err(|e| StoreError::Other(format!("register law_edges: {e}")))?;

        // Register UDFs.
        register_udfs(&ctx);

        info!("DataFusion context ready with legislation + law_edges tables");
        Ok(Self { ctx })
    }

    /// Execute a SQL query and collect all result batches.
    pub async fn query(&self, sql: &str) -> Result<Vec<RecordBatch>, StoreError> {
        let df = self.ctx.sql(sql).await?;
        let batches = df.collect().await?;
        Ok(batches)
    }

    /// Access the underlying `SessionContext` for advanced use.
    pub fn context(&self) -> &SessionContext {
        &self.ctx
    }
}

// ── UDFs ──

fn register_udfs(ctx: &SessionContext) {
    ctx.register_udf(law_status_udf());
    ctx.register_udf(edge_type_label_udf());
}

/// Maps status codes to display labels.
fn law_status_udf() -> datafusion::logical_expr::ScalarUDF {
    datafusion::logical_expr::create_udf(
        "law_status",
        vec![DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(|args: &[datafusion::logical_expr::ColumnarValue]| {
            use datafusion::logical_expr::ColumnarValue;

            let arg = &args[0];
            match arg {
                ColumnarValue::Array(array) => {
                    let input = array
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or_else(|| {
                            DataFusionError::Internal("law_status: expected Utf8 array".into())
                        })?;
                    let result: StringArray =
                        input.iter().map(|opt| opt.map(map_law_status)).collect();
                    Ok(ColumnarValue::Array(Arc::new(result) as ArrayRef))
                }
                ColumnarValue::Scalar(scalar) => {
                    let value = scalar.to_string();
                    let trimmed = value.trim_matches('"');
                    let mapped = map_law_status(trimmed);
                    Ok(ColumnarValue::Scalar(
                        datafusion::common::ScalarValue::Utf8(Some(mapped.to_string())),
                    ))
                }
            }
        }),
    )
}

fn map_law_status(code: &str) -> &str {
    match code {
        "in_force" => "In Force",
        "not_yet_in_force" => "Not Yet In Force",
        "repealed" => "Repealed",
        "revoked" => "Revoked",
        "draft" => "Draft",
        other => other,
    }
}

/// Maps edge type codes to display labels.
fn edge_type_label_udf() -> datafusion::logical_expr::ScalarUDF {
    datafusion::logical_expr::create_udf(
        "edge_type_label",
        vec![DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(|args: &[datafusion::logical_expr::ColumnarValue]| {
            use datafusion::logical_expr::ColumnarValue;

            let arg = &args[0];
            match arg {
                ColumnarValue::Array(array) => {
                    let input = array
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or_else(|| {
                            DataFusionError::Internal("edge_type_label: expected Utf8 array".into())
                        })?;
                    let result: StringArray = input
                        .iter()
                        .map(|opt| opt.map(map_edge_type_label))
                        .collect();
                    Ok(ColumnarValue::Array(Arc::new(result) as ArrayRef))
                }
                ColumnarValue::Scalar(scalar) => {
                    let value = scalar.to_string();
                    let trimmed = value.trim_matches('"');
                    let mapped = map_edge_type_label(trimmed);
                    Ok(ColumnarValue::Scalar(
                        datafusion::common::ScalarValue::Utf8(Some(mapped.to_string())),
                    ))
                }
            }
        }),
    )
}

fn map_edge_type_label(code: &str) -> &str {
    match code {
        "amends" => "Amends",
        "amended_by" => "Amended By",
        "enacted_by" => "Enacted By",
        "rescinds" => "Rescinds",
        "rescinded_by" => "Rescinded By",
        "linked_amends" => "Amends (Linked)",
        "linked_amended_by" => "Amended By (Linked)",
        "linked_enacted_by" => "Enacted By (Linked)",
        "linked_rescinds" => "Rescinds (Linked)",
        "linked_rescinded_by" => "Rescinded By (Linked)",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("data")
    }

    fn require_data() -> PathBuf {
        let dir = data_dir();
        let leg = dir.join("legislation.parquet");
        let edges = dir.join("law_edges.parquet");
        if !leg.exists() || !edges.exists() {
            panic!(
                "Test data not found. Run: duckdb < data/export_legislation.sql\n  Expected: {:?}",
                dir
            );
        }
        dir
    }

    fn loaded_store() -> DuckStore {
        let dir = require_data();
        let store = DuckStore::open().unwrap();
        store.load_all(&dir).unwrap();
        store
    }

    #[tokio::test]
    async fn register_tables() {
        let store = loaded_store();
        let fusion = FusionStore::new(&store).unwrap();
        // Verify both tables are queryable.
        let leg = fusion.query("SELECT name FROM legislation LIMIT 1").await;
        assert!(leg.is_ok(), "legislation table not registered");
        let edges = fusion
            .query("SELECT source_name FROM law_edges LIMIT 1")
            .await;
        assert!(edges.is_ok(), "law_edges table not registered");
    }

    #[tokio::test]
    async fn count_legislation() {
        let store = loaded_store();
        let fusion = FusionStore::new(&store).unwrap();
        let batches = fusion
            .query("SELECT count(*) AS cnt FROM legislation")
            .await
            .unwrap();
        let cnt = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .unwrap()
            .value(0);
        assert!(cnt > 10_000, "expected >10K legislation rows, got {cnt}");
    }

    #[tokio::test]
    async fn count_law_edges() {
        let store = loaded_store();
        let fusion = FusionStore::new(&store).unwrap();
        let batches = fusion
            .query("SELECT count(*) AS cnt FROM law_edges")
            .await
            .unwrap();
        let cnt = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .unwrap()
            .value(0);
        assert!(cnt > 100_000, "expected >100K edges, got {cnt}");
    }

    #[tokio::test]
    async fn projection_pushdown() {
        let store = loaded_store();
        let fusion = FusionStore::new(&store).unwrap();
        let batches = fusion
            .query("SELECT name, year FROM legislation LIMIT 5")
            .await
            .unwrap();
        assert_eq!(batches[0].num_columns(), 2);
        assert_eq!(batches[0].num_rows(), 5);
    }

    #[tokio::test]
    async fn where_filter() {
        let store = loaded_store();
        let fusion = FusionStore::new(&store).unwrap();
        let batches = fusion
            .query("SELECT name FROM legislation WHERE year = 2024 AND status = 'in_force'")
            .await
            .unwrap();
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert!(total_rows > 0, "expected some 2024 in-force laws");
    }

    #[tokio::test]
    async fn limit_pushdown() {
        let store = loaded_store();
        let fusion = FusionStore::new(&store).unwrap();
        let batches = fusion
            .query("SELECT * FROM legislation LIMIT 10")
            .await
            .unwrap();
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 10);
    }

    #[tokio::test]
    async fn cross_table_join() {
        let store = loaded_store();
        let fusion = FusionStore::new(&store).unwrap();
        let batches = fusion
            .query(
                "SELECT l.name, e.edge_type
                 FROM legislation l
                 JOIN law_edges e ON l.name = e.source_name
                 LIMIT 10",
            )
            .await
            .unwrap();
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 10);
        assert_eq!(batches[0].num_columns(), 2);
    }

    #[tokio::test]
    async fn udf_law_status() {
        let store = loaded_store();
        let fusion = FusionStore::new(&store).unwrap();
        let batches = fusion
            .query("SELECT law_status('in_force') AS label")
            .await
            .unwrap();
        let col = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(col.value(0), "In Force");
    }

    #[tokio::test]
    async fn udf_edge_type_label() {
        let store = loaded_store();
        let fusion = FusionStore::new(&store).unwrap();
        let batches = fusion
            .query("SELECT edge_type_label('amended_by') AS label")
            .await
            .unwrap();
        let col = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(col.value(0), "Amended By");
    }

    #[tokio::test]
    async fn udf_passthrough() {
        let store = loaded_store();
        let fusion = FusionStore::new(&store).unwrap();
        let batches = fusion
            .query("SELECT law_status('unknown_code') AS label")
            .await
            .unwrap();
        let col = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(col.value(0), "unknown_code");
    }
}
