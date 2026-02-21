//! DuckDB storage layer for legislation hot path and analytical path.

use std::path::Path;

use arrow::array::Array;
use arrow::record_batch::RecordBatch;
use duckdb::Connection;
use tracing::info;

use crate::StoreError;

/// DuckDB store for legislation hot path and analytical path.
///
/// The hot path (`legislation` table) stores one row per law with 78 columns
/// including `List<Struct>` relationship arrays — single-row lookups need no joins.
///
/// The analytical path (`law_edges` table) is a flattened edge table for
/// vectorised joins and multi-hop graph traversal.
///
/// Supports both in-memory (ephemeral) and persistent (file-backed) modes.
/// Use [`open`](Self::open) for in-memory and [`open_persistent`](Self::open_persistent)
/// for file-backed storage that survives across process restarts.
pub struct DuckStore {
    conn: Connection,
}

impl DuckStore {
    /// Open an in-memory DuckDB database.
    pub fn open() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Ok(Self { conn })
    }

    /// Open or create a persistent DuckDB database at the given path.
    ///
    /// If the file already exists, tables are available immediately without
    /// re-importing from Parquet. Use [`has_tables`](Self::has_tables) to check
    /// whether import is needed.
    pub fn open_persistent(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Ok(Self { conn })
    }

    /// Check whether `legislation` and `law_edges` tables exist and are non-empty.
    pub fn has_tables(&self) -> bool {
        self.legislation_count().is_ok() && self.law_edges_count().is_ok()
    }

    /// Load `legislation.parquet` into the `legislation` table.
    pub fn load_legislation(&self, path: &Path) -> Result<(), StoreError> {
        if !path.exists() {
            return Err(StoreError::ParquetNotFound(path.to_path_buf()));
        }
        let sql = format!(
            "CREATE OR REPLACE TABLE legislation AS SELECT * FROM read_parquet('{}')",
            path.display()
        );
        self.conn.execute_batch(&sql)?;
        let count = self.legislation_count()?;
        info!(count, "loaded legislation table");
        Ok(())
    }

    /// Load `law_edges.parquet` into the `law_edges` table.
    pub fn load_law_edges(&self, path: &Path) -> Result<(), StoreError> {
        if !path.exists() {
            return Err(StoreError::ParquetNotFound(path.to_path_buf()));
        }
        let sql = format!(
            "CREATE OR REPLACE TABLE law_edges AS SELECT * FROM read_parquet('{}')",
            path.display()
        );
        self.conn.execute_batch(&sql)?;
        let count = self.law_edges_count()?;
        info!(count, "loaded law_edges table");
        Ok(())
    }

    /// Load both tables from a data directory containing
    /// `legislation.parquet` and `law_edges.parquet`.
    pub fn load_all(&self, data_dir: &Path) -> Result<(), StoreError> {
        self.load_legislation(&data_dir.join("legislation.parquet"))?;
        self.load_law_edges(&data_dir.join("law_edges.parquet"))?;
        Ok(())
    }

    // ── Counts ──

    /// Number of rows in the `legislation` table.
    pub fn legislation_count(&self) -> Result<usize, StoreError> {
        self.count_table("legislation")
    }

    /// Number of rows in the `law_edges` table.
    pub fn law_edges_count(&self) -> Result<usize, StoreError> {
        self.count_table("law_edges")
    }

    fn count_table(&self, table: &str) -> Result<usize, StoreError> {
        let sql = format!("SELECT count(*)::BIGINT AS cnt FROM {table}");
        let mut stmt = self.conn.prepare(&sql)?;
        let batches: Vec<RecordBatch> = stmt.query_arrow([])?.collect();
        let batch = batches.first().ok_or(StoreError::NoResults)?;
        let col = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .ok_or_else(|| StoreError::Other("count column not i64".into()))?;
        Ok(col.value(0) as usize)
    }

    // ── Hot path ──

    /// Fetch a single legislation record by exact name match.
    ///
    /// Returns one RecordBatch with all 78 columns including `List<Struct>`
    /// relationship arrays. No joins needed.
    pub fn get_legislation(&self, name: &str) -> Result<RecordBatch, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM legislation WHERE name = ?")?;
        let batches: Vec<RecordBatch> = stmt.query_arrow([name])?.collect();
        let batch = batches.into_iter().next().ok_or(StoreError::NoResults)?;
        if batch.num_rows() == 0 {
            return Err(StoreError::NoResults);
        }
        Ok(batch)
    }

    /// Query the legislation table with a SQL WHERE clause.
    ///
    /// The `where_clause` is appended after `WHERE` — do not include the keyword.
    /// Returns all matching rows as Arrow RecordBatches.
    pub fn query_legislation_sql(
        &self,
        where_clause: &str,
    ) -> Result<Vec<RecordBatch>, StoreError> {
        let sql = format!("SELECT * FROM legislation WHERE {where_clause}");
        self.query_arrow(&sql)
    }

    // ── Analytical path ──

    /// All edges where the named law is source or target.
    pub fn edges_for_law(&self, name: &str) -> Result<Vec<RecordBatch>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM law_edges WHERE source_name = ? OR target_name = ?")?;
        let batches: Vec<RecordBatch> = stmt.query_arrow([name, name])?.collect();
        Ok(batches)
    }

    /// Find all laws reachable within `max_hops` of the named law.
    ///
    /// Returns rows with columns `(law_name VARCHAR, hop INTEGER)` ordered by hop distance.
    pub fn laws_within_hops(
        &self,
        name: &str,
        max_hops: u32,
    ) -> Result<Vec<RecordBatch>, StoreError> {
        let sql = format!(
            "WITH RECURSIVE reachable(law_name, hop) AS (
                SELECT ?::VARCHAR, 0
                UNION
                SELECT CASE
                    WHEN e.source_name = r.law_name THEN e.target_name
                    ELSE e.source_name
                END,
                r.hop + 1
                FROM reachable r
                JOIN law_edges e ON e.source_name = r.law_name OR e.target_name = r.law_name
                WHERE r.hop < {max_hops}
            )
            SELECT law_name, min(hop) AS hop
            FROM reachable
            GROUP BY law_name
            ORDER BY hop, law_name"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let batches: Vec<RecordBatch> = stmt.query_arrow([name])?.collect();
        Ok(batches)
    }

    // ── Insert ──

    /// Insert an Arrow RecordBatch into the named table.
    ///
    /// Writes the batch to a temp Parquet file and uses DuckDB's native
    /// `read_parquet()` for bulk insert. The batch schema must match the
    /// target table's columns.
    pub fn insert_batch(&self, table: &str, batch: &RecordBatch) -> Result<(), StoreError> {
        // Validate table name to prevent SQL injection (alphanumeric + underscore only).
        if !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(StoreError::Other(format!("invalid table name: {table}")));
        }

        // Write batch to a temp Parquet file, then INSERT via read_parquet().
        let tmp = tempfile::Builder::new().suffix(".parquet").tempfile()?;
        {
            let mut writer = parquet::arrow::ArrowWriter::try_new(
                tmp.as_file().try_clone()?,
                batch.schema(),
                None,
            )
            .map_err(|e| StoreError::Other(format!("parquet writer: {e}")))?;
            writer
                .write(batch)
                .map_err(|e| StoreError::Other(format!("parquet write: {e}")))?;
            writer
                .close()
                .map_err(|e| StoreError::Other(format!("parquet close: {e}")))?;
        }
        let sql = format!(
            "INSERT INTO {table} SELECT * FROM read_parquet('{}')",
            tmp.path().display()
        );
        self.conn.execute_batch(&sql)?;
        Ok(())
    }

    // ── DRRP tables ──

    /// Create the `drrp_annotations` and `polished_drrp` tables if they don't exist.
    ///
    /// Unlike legislation/law_edges (loaded from Parquet), these are empty tables
    /// populated by `fractalaw sync pull` and the drrp-polisher micro-app.
    pub fn create_drrp_tables(&self) -> Result<(), StoreError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS drrp_annotations (
                law_name       VARCHAR NOT NULL,
                provision      VARCHAR NOT NULL,
                drrp_type      VARCHAR NOT NULL,
                source_text    VARCHAR NOT NULL,
                confidence     FLOAT   NOT NULL,
                scraped_at     TIMESTAMPTZ NOT NULL,
                polished       BOOLEAN NOT NULL DEFAULT false,
                synced_at      TIMESTAMPTZ NOT NULL
            );
            CREATE TABLE IF NOT EXISTS polished_drrp (
                law_name       VARCHAR NOT NULL,
                provision      VARCHAR NOT NULL,
                drrp_type      VARCHAR NOT NULL,
                holder         VARCHAR NOT NULL,
                text           VARCHAR NOT NULL,
                qualifier      VARCHAR,
                clause_ref     VARCHAR NOT NULL,
                confidence     FLOAT   NOT NULL,
                polished_at    TIMESTAMPTZ NOT NULL,
                model          VARCHAR NOT NULL,
                pushed         BOOLEAN NOT NULL DEFAULT false
            );",
        )?;
        info!("ensured drrp_annotations and polished_drrp tables exist");
        Ok(())
    }

    /// Number of rows in the `drrp_annotations` table.
    pub fn drrp_annotations_count(&self) -> Result<usize, StoreError> {
        self.count_table("drrp_annotations")
    }

    /// Number of rows in the `polished_drrp` table.
    pub fn polished_drrp_count(&self) -> Result<usize, StoreError> {
        self.count_table("polished_drrp")
    }

    // ── Sync helpers ──

    /// Insert a batch of annotations pulled from sertantai.
    ///
    /// Each annotation is inserted with `polished = false` and `synced_at = CURRENT_TIMESTAMP`.
    /// Returns the number of rows inserted.
    pub fn insert_annotations(
        &self,
        annotations: &[fractalaw_core::Annotation],
    ) -> Result<usize, StoreError> {
        if annotations.is_empty() {
            return Ok(0);
        }
        for ann in annotations {
            let sql = format!(
                "INSERT INTO drrp_annotations VALUES ('{}', '{}', '{}', '{}', {}, '{}', false, CURRENT_TIMESTAMP)",
                sql_escape(&ann.law_name),
                sql_escape(&ann.provision),
                sql_escape(&ann.drrp_type),
                sql_escape(&ann.source_text),
                ann.confidence,
                sql_escape(&ann.scraped_at),
            );
            self.conn.execute_batch(&sql)?;
        }
        Ok(annotations.len())
    }

    /// Get all polished DRRP entries that haven't been pushed to sertantai yet.
    pub fn get_unpushed_polished(&self) -> Result<Vec<fractalaw_core::PolishedEntry>, StoreError> {
        let batches = self.query_arrow(
            "SELECT law_name, provision, drrp_type, holder, text, qualifier, \
             clause_ref, confidence, polished_at::VARCHAR AS polished_at, model \
             FROM polished_drrp WHERE pushed = false",
        )?;
        let mut entries = Vec::new();
        for batch in &batches {
            let law_name = string_col(batch, "law_name");
            let provision = string_col(batch, "provision");
            let drrp_type = string_col(batch, "drrp_type");
            let holder = string_col(batch, "holder");
            let text = string_col(batch, "text");
            let qualifier = string_col_nullable(batch, "qualifier");
            let clause_ref = string_col(batch, "clause_ref");
            let confidence = float_col(batch, "confidence");
            let polished_at = string_col(batch, "polished_at");
            let model = string_col(batch, "model");

            for i in 0..batch.num_rows() {
                entries.push(fractalaw_core::PolishedEntry {
                    law_name: law_name[i].clone(),
                    provision: provision[i].clone(),
                    drrp_type: drrp_type[i].clone(),
                    holder: holder[i].clone(),
                    text: text[i].clone(),
                    qualifier: qualifier[i].clone(),
                    clause_ref: clause_ref[i].clone(),
                    confidence: confidence[i],
                    polished_at: polished_at[i].clone(),
                    model: model[i].clone(),
                });
            }
        }
        Ok(entries)
    }

    /// Mark polished entries as pushed (by law_name + provision).
    pub fn mark_pushed(&self, law_name: &str, provision: &str) -> Result<(), StoreError> {
        let sql = format!(
            "UPDATE polished_drrp SET pushed = true \
             WHERE law_name = '{}' AND provision = '{}'",
            sql_escape(law_name),
            sql_escape(provision),
        );
        self.conn.execute_batch(&sql)?;
        Ok(())
    }

    /// Get the most recent `synced_at` timestamp from `drrp_annotations`.
    ///
    /// Returns `None` if the table is empty (no prior sync).
    pub fn get_last_sync_at(&self) -> Result<Option<String>, StoreError> {
        let batches =
            self.query_arrow("SELECT MAX(synced_at)::VARCHAR AS last_sync FROM drrp_annotations")?;
        if let Some(batch) = batches.first()
            && batch.num_rows() > 0
        {
            let col = batch
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::StringArray>();
            if let Some(arr) = col
                && !arr.is_null(0)
            {
                return Ok(Some(arr.value(0).to_string()));
            }
            // DuckDB may return LargeStringArray for VARCHAR casts.
            let col = batch
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::LargeStringArray>();
            if let Some(arr) = col
                && !arr.is_null(0)
            {
                return Ok(Some(arr.value(0).to_string()));
            }
        }
        Ok(None)
    }

    // ── Escape hatch ──

    /// Execute a DDL/DML statement that returns no result set.
    ///
    /// Use for `ALTER TABLE`, `UPDATE`, `INSERT`, `CREATE TEMP TABLE`, etc.
    pub fn execute(&self, sql: &str) -> Result<(), StoreError> {
        self.conn.execute_batch(sql)?;
        Ok(())
    }

    /// Execute arbitrary SQL and return Arrow RecordBatches.
    pub fn query_arrow(&self, sql: &str) -> Result<Vec<RecordBatch>, StoreError> {
        let mut stmt = self.conn.prepare(sql)?;
        let batches: Vec<RecordBatch> = stmt.query_arrow([])?.collect();
        Ok(batches)
    }

    /// Access the underlying DuckDB connection (for DataFusion TableProvider registration).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// Escape single quotes in a string for safe SQL interpolation.
fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// Extract a non-nullable VARCHAR column as a Vec of Strings.
fn string_col(batch: &RecordBatch, name: &str) -> Vec<String> {
    let col = batch.column_by_name(name).expect(name);
    if let Some(arr) = col.as_any().downcast_ref::<arrow::array::StringArray>() {
        (0..arr.len()).map(|i| arr.value(i).to_string()).collect()
    } else if let Some(arr) = col
        .as_any()
        .downcast_ref::<arrow::array::LargeStringArray>()
    {
        (0..arr.len()).map(|i| arr.value(i).to_string()).collect()
    } else {
        panic!("column {name} is not a string type");
    }
}

/// Extract a nullable VARCHAR column as a Vec of Option<String>.
fn string_col_nullable(batch: &RecordBatch, name: &str) -> Vec<Option<String>> {
    let col = batch.column_by_name(name).expect(name);
    if let Some(arr) = col.as_any().downcast_ref::<arrow::array::StringArray>() {
        (0..arr.len())
            .map(|i| {
                if arr.is_null(i) {
                    None
                } else {
                    Some(arr.value(i).to_string())
                }
            })
            .collect()
    } else if let Some(arr) = col
        .as_any()
        .downcast_ref::<arrow::array::LargeStringArray>()
    {
        (0..arr.len())
            .map(|i| {
                if arr.is_null(i) {
                    None
                } else {
                    Some(arr.value(i).to_string())
                }
            })
            .collect()
    } else {
        panic!("column {name} is not a string type");
    }
}

/// Extract a FLOAT column as a Vec of f32.
fn float_col(batch: &RecordBatch, name: &str) -> Vec<f32> {
    let col = batch.column_by_name(name).expect(name);
    let arr = col
        .as_any()
        .downcast_ref::<arrow::array::Float32Array>()
        .unwrap_or_else(|| panic!("column {name} is not Float32"));
    (0..arr.len()).map(|i| arr.value(i)).collect()
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

    #[test]
    fn open_in_memory() {
        let store = DuckStore::open().unwrap();
        let batches = store.query_arrow("SELECT 1 AS x").unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 1);
    }

    #[test]
    fn load_missing_file_errors() {
        let store = DuckStore::open().unwrap();
        let result = store.load_legislation(Path::new("/nonexistent/file.parquet"));
        assert!(matches!(result, Err(StoreError::ParquetNotFound(_))));
    }

    #[test]
    fn load_legislation_count() {
        let dir = require_data();
        let store = DuckStore::open().unwrap();
        store
            .load_legislation(&dir.join("legislation.parquet"))
            .unwrap();
        let count = store.legislation_count().unwrap();
        assert!(count > 10_000, "expected >10K laws, got {count}");
    }

    #[test]
    fn load_law_edges_count() {
        let dir = require_data();
        let store = DuckStore::open().unwrap();
        store
            .load_law_edges(&dir.join("law_edges.parquet"))
            .unwrap();
        let count = store.law_edges_count().unwrap();
        assert!(count > 100_000, "expected >100K edges, got {count}");
    }

    #[test]
    fn load_all_and_verify() {
        let dir = require_data();
        let store = DuckStore::open().unwrap();
        store.load_all(&dir).unwrap();
        assert!(store.legislation_count().unwrap() > 10_000);
        assert!(store.law_edges_count().unwrap() > 100_000);
    }

    #[test]
    fn hot_path_get_by_name() {
        let dir = require_data();
        let store = DuckStore::open().unwrap();
        store.load_all(&dir).unwrap();

        let batch = store.get_legislation("UK_ukpga_1974_37").unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 78);
    }

    #[test]
    fn hot_path_no_results() {
        let dir = require_data();
        let store = DuckStore::open().unwrap();
        store.load_all(&dir).unwrap();

        let result = store.get_legislation("NONEXISTENT_LAW_999");
        assert!(matches!(result, Err(StoreError::NoResults)));
    }

    #[test]
    fn hot_path_filter() {
        let dir = require_data();
        let store = DuckStore::open().unwrap();
        store.load_all(&dir).unwrap();

        let batches = store
            .query_legislation_sql("year = 2024 AND status = 'in_force'")
            .unwrap();
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert!(total_rows > 0, "expected some 2024 in-force laws");
    }

    #[test]
    fn analytical_edges_for_law() {
        let dir = require_data();
        let store = DuckStore::open().unwrap();
        store.load_all(&dir).unwrap();

        let batches = store.edges_for_law("UK_ukpga_1974_37").unwrap();
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert!(
            total_rows > 50,
            "HSWA 1974 should have many edges, got {total_rows}"
        );
    }

    #[test]
    fn analytical_two_hop_traversal() {
        let dir = require_data();
        let store = DuckStore::open().unwrap();
        store.load_all(&dir).unwrap();

        let batches = store.laws_within_hops("UK_ukpga_1974_37", 2).unwrap();
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert!(
            total_rows > 10,
            "2-hop from HSWA should reach many laws, got {total_rows}"
        );

        // Verify schema has law_name and hop columns
        let schema = batches[0].schema();
        assert_eq!(schema.field(0).name(), "law_name");
        assert_eq!(schema.field(1).name(), "hop");
    }

    #[test]
    fn query_arrow_escape_hatch() {
        let dir = require_data();
        let store = DuckStore::open().unwrap();
        store.load_all(&dir).unwrap();

        let batches = store
            .query_arrow("SELECT name, year FROM legislation ORDER BY year DESC LIMIT 5")
            .unwrap();
        assert_eq!(batches[0].num_rows(), 5);
        assert_eq!(batches[0].num_columns(), 2);
    }

    // ── Persistent storage tests ──

    #[test]
    fn open_persistent_creates_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("test.duckdb");
        assert!(!db_path.exists());

        let store = DuckStore::open_persistent(&db_path).unwrap();
        // File is created on open.
        assert!(db_path.exists());
        // No tables yet.
        assert!(!store.has_tables());
    }

    #[test]
    fn persistent_load_and_reopen() {
        let dir = require_data();
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("test.duckdb");

        // First open: import from Parquet.
        let store = DuckStore::open_persistent(&db_path).unwrap();
        assert!(!store.has_tables());
        store.load_all(&dir).unwrap();
        assert!(store.has_tables());
        let leg_count = store.legislation_count().unwrap();
        let edge_count = store.law_edges_count().unwrap();
        drop(store);

        // Second open: tables already present, no import needed.
        let store = DuckStore::open_persistent(&db_path).unwrap();
        assert!(store.has_tables());
        assert_eq!(store.legislation_count().unwrap(), leg_count);
        assert_eq!(store.law_edges_count().unwrap(), edge_count);
    }

    #[test]
    fn persistent_queries_work() {
        let dir = require_data();
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("test.duckdb");

        let store = DuckStore::open_persistent(&db_path).unwrap();
        store.load_all(&dir).unwrap();
        drop(store);

        // Reopen and run queries against persisted data.
        let store = DuckStore::open_persistent(&db_path).unwrap();
        let batch = store.get_legislation("UK_ukpga_1974_37").unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 78);

        let edges = store.edges_for_law("UK_ukpga_1974_37").unwrap();
        let total_edges: usize = edges.iter().map(|b| b.num_rows()).sum();
        assert!(total_edges > 50);
    }

    #[test]
    fn has_tables_false_for_empty_memory() {
        let store = DuckStore::open().unwrap();
        assert!(!store.has_tables());
    }

    // ── DRRP tables ──

    #[test]
    fn create_drrp_tables_empty() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();
        assert_eq!(store.drrp_annotations_count().unwrap(), 0);
        assert_eq!(store.polished_drrp_count().unwrap(), 0);
    }

    #[test]
    fn create_drrp_tables_idempotent() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();
        store.create_drrp_tables().unwrap(); // second call should not error
        assert_eq!(store.drrp_annotations_count().unwrap(), 0);
    }

    #[test]
    fn drrp_annotations_insert_and_count() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();
        store
            .execute(
                "INSERT INTO drrp_annotations VALUES (
                    'UK_ukpga_1974_37', 's.2(1)', 'duty',
                    'It shall be the duty of every employer...',
                    0.85, '2026-02-21T10:00:00Z', false, '2026-02-21T12:00:00Z'
                )",
            )
            .unwrap();
        assert_eq!(store.drrp_annotations_count().unwrap(), 1);
    }

    #[test]
    fn polished_drrp_insert_and_count() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();
        store
            .execute(
                "INSERT INTO polished_drrp VALUES (
                    'UK_ukpga_1974_37', 's.2(1)', 'duty', 'every employer',
                    'ensure health safety and welfare of employees',
                    'so far as is reasonably practicable', 's.2(1)',
                    0.95, '2026-02-21T13:00:00Z', 'claude-sonnet-4-5-20250929', false
                )",
            )
            .unwrap();
        assert_eq!(store.polished_drrp_count().unwrap(), 1);
    }

    #[test]
    fn polished_drrp_qualifier_nullable() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();
        store
            .execute(
                "INSERT INTO polished_drrp VALUES (
                    'UK_ukpga_1974_37', 's.3', 'duty', 'every employer',
                    'conduct undertaking without risk to persons',
                    NULL, 's.3',
                    0.90, '2026-02-21T13:00:00Z', 'claude-sonnet-4-5-20250929', false
                )",
            )
            .unwrap();
        assert_eq!(store.polished_drrp_count().unwrap(), 1);
    }

    // ── insert_batch ──

    #[test]
    fn insert_batch_roundtrip() {
        use arrow::array::{Float32Array, StringArray};
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;

        let store = DuckStore::open().unwrap();
        store
            .execute("CREATE TABLE test_insert (name VARCHAR, score FLOAT)")
            .unwrap();

        let schema = Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, true),
            Field::new("score", DataType::Float32, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["alice", "bob"])),
                Arc::new(Float32Array::from(vec![0.9, 0.7])),
            ],
        )
        .unwrap();

        store.insert_batch("test_insert", &batch).unwrap();

        let result = store
            .query_arrow("SELECT name, score FROM test_insert ORDER BY name")
            .unwrap();
        let total_rows: usize = result.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 2);
    }

    #[test]
    fn insert_batch_rejects_bad_table_name() {
        use arrow::array::StringArray;
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;

        let store = DuckStore::open().unwrap();
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Utf8, true)]));
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(StringArray::from(vec!["test"]))]).unwrap();

        let result = store.insert_batch("bad;table", &batch);
        assert!(result.is_err());
    }

    // ── Sync helpers ──

    #[test]
    fn insert_annotations_roundtrip() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();

        let annotations = vec![
            fractalaw_core::Annotation {
                law_name: "UK_ukpga_1974_37".into(),
                provision: "s.2(1)".into(),
                drrp_type: "duty".into(),
                source_text: "It shall be the duty of every employer...".into(),
                confidence: 0.85,
                scraped_at: "2026-02-21T10:00:00Z".into(),
            },
            fractalaw_core::Annotation {
                law_name: "UK_ukpga_1974_37".into(),
                provision: "s.7(a)".into(),
                drrp_type: "duty".into(),
                source_text: "It shall be the duty of every employee...".into(),
                confidence: 0.80,
                scraped_at: "2026-02-21T10:00:00Z".into(),
            },
        ];

        let count = store.insert_annotations(&annotations).unwrap();
        assert_eq!(count, 2);
        assert_eq!(store.drrp_annotations_count().unwrap(), 2);
    }

    #[test]
    fn insert_annotations_empty() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();

        let count = store.insert_annotations(&[]).unwrap();
        assert_eq!(count, 0);
        assert_eq!(store.drrp_annotations_count().unwrap(), 0);
    }

    #[test]
    fn insert_annotations_escapes_quotes() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();

        let annotations = vec![fractalaw_core::Annotation {
            law_name: "UK_ukpga_1974_37".into(),
            provision: "s.2(1)".into(),
            drrp_type: "duty".into(),
            source_text: "employer's duty to ensure employees' safety".into(),
            confidence: 0.85,
            scraped_at: "2026-02-21T10:00:00Z".into(),
        }];

        store.insert_annotations(&annotations).unwrap();
        assert_eq!(store.drrp_annotations_count().unwrap(), 1);
    }

    #[test]
    fn get_unpushed_polished_returns_entries() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();

        // Insert two unpushed and one pushed entry.
        store
            .execute(
                "INSERT INTO polished_drrp VALUES
                    ('UK_ukpga_1974_37', 's.2(1)', 'duty', 'every employer',
                     'ensure health safety', 'so far as is reasonably practicable', 's.2(1)',
                     0.95, '2026-02-21T13:00:00Z', 'claude-sonnet-4-5-20250929', false),
                    ('UK_ukpga_1974_37', 's.7(a)', 'duty', 'every employee',
                     'take reasonable care', NULL, 's.7(a)',
                     0.90, '2026-02-21T13:00:00Z', 'claude-sonnet-4-5-20250929', false),
                    ('UK_ukpga_1974_37', 's.3', 'duty', 'every employer',
                     'conduct undertaking', NULL, 's.3',
                     0.88, '2026-02-21T13:00:00Z', 'claude-sonnet-4-5-20250929', true)",
            )
            .unwrap();

        let entries = store.get_unpushed_polished().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].provision, "s.2(1)");
        assert_eq!(
            entries[0].qualifier.as_deref(),
            Some("so far as is reasonably practicable")
        );
        assert!(entries[1].qualifier.is_none());
    }

    #[test]
    fn mark_pushed_updates_flag() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();

        store
            .execute(
                "INSERT INTO polished_drrp VALUES
                    ('UK_ukpga_1974_37', 's.2(1)', 'duty', 'every employer',
                     'ensure health safety', NULL, 's.2(1)',
                     0.95, '2026-02-21T13:00:00Z', 'claude-sonnet-4-5-20250929', false)",
            )
            .unwrap();

        assert_eq!(store.get_unpushed_polished().unwrap().len(), 1);
        store.mark_pushed("UK_ukpga_1974_37", "s.2(1)").unwrap();
        assert_eq!(store.get_unpushed_polished().unwrap().len(), 0);
    }

    #[test]
    fn get_last_sync_at_empty() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();
        assert!(store.get_last_sync_at().unwrap().is_none());
    }

    #[test]
    fn get_last_sync_at_returns_max() {
        let store = DuckStore::open().unwrap();
        store.create_drrp_tables().unwrap();

        store
            .execute(
                "INSERT INTO drrp_annotations VALUES
                    ('UK_ukpga_1974_37', 's.2(1)', 'duty', 'text1', 0.85,
                     '2026-02-21T10:00:00Z', false, '2026-02-20T12:00:00Z'),
                    ('UK_ukpga_1974_37', 's.7(a)', 'duty', 'text2', 0.80,
                     '2026-02-21T10:00:00Z', false, '2026-02-21T12:00:00Z')",
            )
            .unwrap();

        let last = store.get_last_sync_at().unwrap();
        assert!(last.is_some());
        let ts = last.unwrap();
        assert!(
            ts.contains("2026-02-21"),
            "expected latest timestamp, got {ts}"
        );
    }
}
