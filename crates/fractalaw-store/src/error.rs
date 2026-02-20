use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("parquet file not found: {0}")]
    ParquetNotFound(std::path::PathBuf),

    #[error("no results for query")]
    NoResults,

    #[cfg(feature = "duckdb")]
    #[error("duckdb error: {0}")]
    DuckDb(#[from] ::duckdb::Error),

    #[error("arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[cfg(feature = "datafusion")]
    #[error("datafusion error: {0}")]
    DataFusion(#[from] datafusion::error::DataFusionError),

    #[cfg(feature = "lancedb")]
    #[error("lancedb error: {0}")]
    LanceDb(#[from] lancedb::error::Error),

    #[cfg(feature = "lancedb")]
    #[error("parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}
