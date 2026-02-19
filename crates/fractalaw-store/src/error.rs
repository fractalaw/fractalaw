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

    #[error("{0}")]
    Other(String),
}
