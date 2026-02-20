//! LanceDB storage layer for legislation text and amendment annotations.
//!
//! The semantic path stores full legal text with embeddings for similarity search.
//! Two tables: `legislation_text` (97K structural units) and `amendment_annotations`
//! (19K change annotations).

use std::path::Path;

use arrow::array::RecordBatchIterator;
use arrow::record_batch::RecordBatch;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use tracing::info;

use crate::StoreError;

const LEGISLATION_TEXT_TABLE: &str = "legislation_text";
const AMENDMENT_ANNOTATIONS_TABLE: &str = "amendment_annotations";

/// LanceDB store for the semantic path (legislation text + annotations).
///
/// Manages two Lance tables:
/// - `legislation_text`: 97K structural units with text and embeddings
/// - `amendment_annotations`: 19K amendment footnotes linked to text sections
pub struct LanceStore {
    db: lancedb::Connection,
}

impl LanceStore {
    /// Connect to a LanceDB database at the given path.
    ///
    /// Creates the database directory if it doesn't exist.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let uri = path
            .to_str()
            .ok_or_else(|| StoreError::Other("non-UTF8 database path".into()))?;
        let db = lancedb::connect(uri).execute().await?;
        Ok(Self { db })
    }

    /// Create (or replace) the `legislation_text` table from a Parquet file.
    pub async fn create_legislation_text(&self, parquet_path: &Path) -> Result<(), StoreError> {
        self.create_table_from_parquet(LEGISLATION_TEXT_TABLE, parquet_path)
            .await
    }

    /// Create (or replace) the `amendment_annotations` table from a Parquet file.
    pub async fn create_amendment_annotations(
        &self,
        parquet_path: &Path,
    ) -> Result<(), StoreError> {
        self.create_table_from_parquet(AMENDMENT_ANNOTATIONS_TABLE, parquet_path)
            .await
    }

    /// Load both tables from a data directory containing the Parquet files.
    pub async fn load_all(&self, data_dir: &Path) -> Result<(), StoreError> {
        self.create_legislation_text(&data_dir.join("legislation_text.parquet"))
            .await?;
        self.create_amendment_annotations(&data_dir.join("amendment_annotations.parquet"))
            .await?;
        Ok(())
    }

    /// Open the `legislation_text` table.
    pub async fn legislation_text(&self) -> Result<lancedb::Table, StoreError> {
        let table = self.db.open_table(LEGISLATION_TEXT_TABLE).execute().await?;
        Ok(table)
    }

    /// Open the `amendment_annotations` table.
    pub async fn amendment_annotations(&self) -> Result<lancedb::Table, StoreError> {
        let table = self
            .db
            .open_table(AMENDMENT_ANNOTATIONS_TABLE)
            .execute()
            .await?;
        Ok(table)
    }

    /// Count rows in the `legislation_text` table.
    pub async fn legislation_text_count(&self) -> Result<usize, StoreError> {
        let table = self.legislation_text().await?;
        let count = table.count_rows(None).await?;
        Ok(count)
    }

    /// Count rows in the `amendment_annotations` table.
    pub async fn amendment_annotations_count(&self) -> Result<usize, StoreError> {
        let table = self.amendment_annotations().await?;
        let count = table.count_rows(None).await?;
        Ok(count)
    }

    /// Vector similarity search on the `legislation_text` embedding column.
    ///
    /// Returns the nearest `limit` rows to the query vector, ordered by distance.
    /// Requires embeddings to have been populated (Task 4).
    pub async fn search_text(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<RecordBatch>, StoreError> {
        let table = self.legislation_text().await?;
        let results: Vec<RecordBatch> = table
            .vector_search(query_vector)?
            .limit(limit)
            .execute()
            .await?
            .try_collect()
            .await?;
        Ok(results)
    }

    /// Query the `legislation_text` table with a SQL filter.
    pub async fn query_legislation_text(
        &self,
        filter: &str,
        limit: usize,
    ) -> Result<Vec<RecordBatch>, StoreError> {
        let table = self.legislation_text().await?;
        let results: Vec<RecordBatch> = table
            .query()
            .only_if(filter)
            .limit(limit)
            .execute()
            .await?
            .try_collect()
            .await?;
        Ok(results)
    }

    /// List table names in the database.
    pub async fn table_names(&self) -> Result<Vec<String>, StoreError> {
        let names = self.db.table_names().execute().await?;
        Ok(names)
    }

    /// Create (or replace) a table from pre-built RecordBatches.
    ///
    /// Used by the embedding pipeline to write batches with populated embedding columns.
    pub async fn create_table_from_batches(
        &self,
        table_name: &str,
        batches: Vec<RecordBatch>,
    ) -> Result<(), StoreError> {
        if batches.is_empty() {
            return Err(StoreError::Other("no record batches provided".into()));
        }

        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        let schema = batches[0].schema();
        let reader = RecordBatchIterator::new(batches.into_iter().map(Ok), schema);

        let existing = self.db.table_names().execute().await?;
        if existing.contains(&table_name.to_string()) {
            self.db.drop_table(table_name, &[]).await?;
        }

        self.db
            .create_table(table_name, Box::new(reader))
            .execute()
            .await?;

        info!(
            table = table_name,
            rows = total_rows,
            "created LanceDB table from batches"
        );
        Ok(())
    }

    // ── Internal ──

    async fn create_table_from_parquet(
        &self,
        table_name: &str,
        parquet_path: &Path,
    ) -> Result<(), StoreError> {
        if !parquet_path.exists() {
            return Err(StoreError::ParquetNotFound(parquet_path.to_path_buf()));
        }

        let batches = read_parquet(parquet_path)?;
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

        if batches.is_empty() {
            return Err(StoreError::Other(format!(
                "no record batches in {parquet_path:?}"
            )));
        }

        let schema = batches[0].schema();
        let reader = RecordBatchIterator::new(batches.into_iter().map(Ok), schema);

        // Drop existing table if it exists, then create fresh.
        let existing = self.db.table_names().execute().await?;
        if existing.contains(&table_name.to_string()) {
            self.db.drop_table(table_name, &[]).await?;
        }

        self.db
            .create_table(table_name, Box::new(reader))
            .execute()
            .await?;

        info!(
            table = table_name,
            rows = total_rows,
            "created LanceDB table"
        );
        Ok(())
    }
}

/// Read a Parquet file into Arrow RecordBatches.
pub fn read_parquet(path: &Path) -> Result<Vec<RecordBatch>, StoreError> {
    let file = std::fs::File::open(path)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
    let batches: Result<Vec<RecordBatch>, _> = reader.collect();
    Ok(batches?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("data")
    }

    fn require_lat_data() -> PathBuf {
        let dir = data_dir();
        let lat = dir.join("legislation_text.parquet");
        let ann = dir.join("amendment_annotations.parquet");
        if !lat.exists() || !ann.exists() {
            panic!(
                "LAT data not found. Run: duckdb < data/export_lat.sql\n  Expected: {:?}",
                dir
            );
        }
        dir
    }

    #[tokio::test]
    async fn open_creates_database() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test_lancedb");
        let store = LanceStore::open(&db_path).await.unwrap();
        let names = store.table_names().await.unwrap();
        assert!(names.is_empty());
    }

    #[tokio::test]
    async fn create_legislation_text() {
        let dir = require_lat_data();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test_lancedb");
        let store = LanceStore::open(&db_path).await.unwrap();

        store
            .create_legislation_text(&dir.join("legislation_text.parquet"))
            .await
            .unwrap();

        let count = store.legislation_text_count().await.unwrap();
        assert!(
            count > 90_000,
            "expected >90K legislation_text rows, got {count}"
        );

        let names = store.table_names().await.unwrap();
        assert!(names.contains(&"legislation_text".to_string()));
    }

    #[tokio::test]
    async fn create_amendment_annotations() {
        let dir = require_lat_data();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test_lancedb");
        let store = LanceStore::open(&db_path).await.unwrap();

        store
            .create_amendment_annotations(&dir.join("amendment_annotations.parquet"))
            .await
            .unwrap();

        let count = store.amendment_annotations_count().await.unwrap();
        assert!(count > 15_000, "expected >15K annotation rows, got {count}");
    }

    #[tokio::test]
    async fn load_all_creates_both_tables() {
        let dir = require_lat_data();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test_lancedb");
        let store = LanceStore::open(&db_path).await.unwrap();

        store.load_all(&dir).await.unwrap();

        let names = store.table_names().await.unwrap();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"legislation_text".to_string()));
        assert!(names.contains(&"amendment_annotations".to_string()));
    }

    #[tokio::test]
    async fn query_legislation_text_by_law() {
        let dir = require_lat_data();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test_lancedb");
        let store = LanceStore::open(&db_path).await.unwrap();
        store
            .create_legislation_text(&dir.join("legislation_text.parquet"))
            .await
            .unwrap();

        let batches = store
            .query_legislation_text("law_name = 'UK_ukpga_1974_37'", 1000)
            .await
            .unwrap();

        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert!(
            total_rows > 100,
            "HSWA 1974 should have >100 text rows, got {total_rows}"
        );
    }

    #[tokio::test]
    async fn missing_parquet_errors() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test_lancedb");
        let store = LanceStore::open(&db_path).await.unwrap();

        let result = store
            .create_legislation_text(Path::new("/nonexistent/file.parquet"))
            .await;
        assert!(matches!(result, Err(StoreError::ParquetNotFound(_))));
    }

    #[tokio::test]
    async fn reload_replaces_table() {
        let dir = require_lat_data();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test_lancedb");
        let store = LanceStore::open(&db_path).await.unwrap();

        // Load once.
        store
            .create_legislation_text(&dir.join("legislation_text.parquet"))
            .await
            .unwrap();
        let count1 = store.legislation_text_count().await.unwrap();

        // Load again — should replace, not append.
        store
            .create_legislation_text(&dir.join("legislation_text.parquet"))
            .await
            .unwrap();
        let count2 = store.legislation_text_count().await.unwrap();

        assert_eq!(count1, count2);
    }

    #[tokio::test]
    async fn legislation_text_schema_has_expected_columns() {
        let dir = require_lat_data();
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test_lancedb");
        let store = LanceStore::open(&db_path).await.unwrap();
        store
            .create_legislation_text(&dir.join("legislation_text.parquet"))
            .await
            .unwrap();

        let table = store.legislation_text().await.unwrap();
        let schema = table.schema().await.unwrap();

        // Key columns must exist.
        assert!(schema.field_with_name("section_id").is_ok());
        assert!(schema.field_with_name("law_name").is_ok());
        assert!(schema.field_with_name("text").is_ok());
        assert!(schema.field_with_name("sort_key").is_ok());
        assert!(schema.field_with_name("embedding").is_ok());
    }
}
