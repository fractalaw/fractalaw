//! Storage layer: DuckDB (analytical), LanceDB (vector), DataFusion (unified query).

mod error;
pub use error::StoreError;

#[cfg(feature = "duckdb")]
mod duck;
#[cfg(feature = "duckdb")]
pub use duck::DuckStore;
