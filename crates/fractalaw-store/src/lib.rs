//! Storage layer: DuckDB (analytical), LanceDB (vector), DataFusion (unified query).

mod error;
pub use error::StoreError;

#[cfg(feature = "duckdb")]
mod duck;
#[cfg(feature = "duckdb")]
pub use duck::DuckStore;

#[cfg(all(feature = "duckdb", feature = "datafusion"))]
mod fusion;
#[cfg(all(feature = "duckdb", feature = "datafusion"))]
pub use fusion::FusionStore;
