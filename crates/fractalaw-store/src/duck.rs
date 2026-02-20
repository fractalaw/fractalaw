//! DuckDB storage layer for legislation hot path and analytical path.

use std::path::Path;

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

    // ── Escape hatch ──

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
}
