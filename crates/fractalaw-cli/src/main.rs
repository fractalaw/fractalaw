mod display;
mod embed;

use std::path::PathBuf;

use anyhow::Context;
use arrow::record_batch::RecordBatch;
use arrow::util::pretty::print_batches;
use clap::{Parser, Subcommand};
use fractalaw_store::{DuckStore, FusionStore, LanceStore, StoreError};

#[derive(Parser)]
#[command(
    name = "fractalaw",
    version,
    about = "Local-first ESH regulatory data tools"
)]
struct Cli {
    /// Path to data directory containing Parquet files
    #[arg(long, default_value = "./data", global = true)]
    data_dir: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Execute SQL via DataFusion (supports law_status() and edge_type_label() UDFs)
    Query {
        /// SQL query string
        sql: String,
    },

    /// Show a single legislation record with relationships
    Law {
        /// Legislation name (e.g., UK_ukpga_1974_37)
        name: String,
    },

    /// Show amendment/enactment graph traversal
    Graph {
        /// Legislation name to start traversal from
        name: String,

        /// Maximum hops from the starting law
        #[arg(long, default_value_t = 2)]
        hops: u32,
    },

    /// Show dataset summary statistics
    Stats,

    /// Generate embeddings for all legislation text and write to LanceDB
    Embed {
        /// Path to ONNX model directory
        #[arg(long, default_value = "./models/all-MiniLM-L6-v2")]
        model_dir: PathBuf,
    },

    /// Show legislation text sections from LanceDB
    Text {
        /// Legislation name (e.g., UK_ukpga_1974_37)
        name: String,
        /// Maximum rows to display
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },

    /// Semantic similarity search across legislation text
    Search {
        /// Natural language query
        query: String,
        /// Number of results
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Path to ONNX model directory
        #[arg(long, default_value = "./models/all-MiniLM-L6-v2")]
        model_dir: PathBuf,
    },

    /// Run validation checks across all data stores
    Validate {
        /// Path to ONNX model directory (for semantic smoke test)
        #[arg(long, default_value = "./models/all-MiniLM-L6-v2")]
        model_dir: PathBuf,
    },

    /// Import (or re-import) Parquet files into persistent DuckDB
    Import,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let data_dir = cli
        .data_dir
        .canonicalize()
        .with_context(|| format!("data directory '{}' not found", cli.data_dir.display()))?;

    match cli.command {
        // DuckDB commands — open persistent store with auto-import on first run.
        Command::Query { sql } => cmd_query(&open_duck(&data_dir)?, &sql).await,
        Command::Law { name } => cmd_law(&open_duck(&data_dir)?, &name),
        Command::Graph { name, hops } => cmd_graph(&open_duck(&data_dir)?, &name, hops),
        Command::Stats => cmd_stats(&open_duck(&data_dir)?),
        Command::Validate { model_dir } => {
            cmd_validate(&open_duck(&data_dir)?, &data_dir, &model_dir).await
        }
        Command::Import => cmd_import(&data_dir),

        // LanceDB-only commands — no DuckDB needed.
        Command::Embed { model_dir } => cmd_embed(&data_dir, &model_dir).await,
        Command::Text { name, limit } => cmd_text(&data_dir, &name, limit).await,
        Command::Search {
            query,
            limit,
            model_dir,
        } => cmd_search(&data_dir, &query, limit, &model_dir).await,
    }
}

/// Open persistent DuckDB, auto-importing from Parquet on first run.
fn open_duck(data_dir: &std::path::Path) -> anyhow::Result<DuckStore> {
    let db_path = data_dir.join("fractalaw.duckdb");
    let store = DuckStore::open_persistent(&db_path)?;
    if !store.has_tables() {
        eprintln!(
            "First run — importing Parquet into {}...",
            db_path.display()
        );
        store.load_all(data_dir)?;
    }
    Ok(store)
}

fn cmd_import(data_dir: &std::path::Path) -> anyhow::Result<()> {
    let db_path = data_dir.join("fractalaw.duckdb");
    let store = DuckStore::open_persistent(&db_path)?;
    store.load_all(data_dir)?;
    println!(
        "Imported into {}\n  Legislation: {:>8} rows\n  Law edges:   {:>8} rows",
        db_path.display(),
        fmt_num(store.legislation_count()?),
        fmt_num(store.law_edges_count()?),
    );
    Ok(())
}

async fn cmd_query(store: &DuckStore, sql: &str) -> anyhow::Result<()> {
    let fusion = FusionStore::new(store)?;
    let batches = fusion.query(sql).await?;
    if batches.is_empty() || batches.iter().all(|b| b.num_rows() == 0) {
        println!("No results.");
        return Ok(());
    }
    print_batches(&batches)?;
    Ok(())
}

fn cmd_law(store: &DuckStore, name: &str) -> anyhow::Result<()> {
    let batch = store.get_legislation(name).map_err(|e| match e {
        StoreError::NoResults => anyhow::anyhow!("legislation '{}' not found", name),
        other => anyhow::anyhow!(other),
    })?;

    display::print_law_card(&batch)?;

    let edges = store.edges_for_law(name)?;
    let total_edges: usize = edges.iter().map(|b| b.num_rows()).sum();
    if total_edges > 0 {
        println!("--- Relationships ({total_edges} edges) ---\n");
        print_batches(&edges)?;
    }

    Ok(())
}

fn cmd_graph(store: &DuckStore, name: &str, hops: u32) -> anyhow::Result<()> {
    let batches = store.laws_within_hops(name, hops)?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    if total_rows == 0 {
        println!("No laws found within {hops} hops of '{name}'.");
        return Ok(());
    }
    println!("Laws within {hops} hops of '{name}' ({total_rows} total):\n");
    print_batches(&batches)?;
    Ok(())
}

async fn cmd_embed(data_dir: &std::path::Path, model_dir: &std::path::Path) -> anyhow::Result<()> {
    let model_dir = model_dir
        .canonicalize()
        .with_context(|| format!("model directory '{}' not found", model_dir.display()))?;

    println!("=== Embedding Pipeline ===\n");

    let mut embedder =
        fractalaw_ai::Embedder::load(&model_dir).context("loading embedding model")?;
    println!("  Model: {} ({}D)", model_dir.display(), embedder.dim());

    let lance_path = data_dir.join("lancedb");
    let lance = LanceStore::open(&lance_path)
        .await
        .context("opening LanceDB")?;

    let parquet_path = data_dir.join("legislation_text.parquet");
    let stats = embed::run_embed_pipeline(&lance, &mut embedder, &parquet_path).await?;

    println!("\n=== Complete ===");
    println!("  Rows:       {:>8}", stats.total_rows);
    println!("  Time:       {:>8.1}s", stats.elapsed_secs);
    if stats.elapsed_secs > 0.0 {
        println!(
            "  Throughput: {:>8.0} rows/sec",
            stats.total_rows as f64 / stats.elapsed_secs
        );
    }

    Ok(())
}

async fn cmd_text(data_dir: &std::path::Path, name: &str, limit: usize) -> anyhow::Result<()> {
    let lance = LanceStore::open(&data_dir.join("lancedb"))
        .await
        .context("opening LanceDB")?;

    let filter = format!("law_name = '{name}'");
    let batches = lance.query_legislation_text(&filter, limit).await?;

    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    if total == 0 {
        println!("No text sections found for '{name}'.");
        return Ok(());
    }

    println!("Text sections for '{name}' ({total} rows):\n");
    let projected = project_batches(
        &batches,
        &["provision", "section_type", "heading_group", "text"],
    );
    print_batches(&projected)?;
    Ok(())
}

async fn cmd_search(
    data_dir: &std::path::Path,
    query: &str,
    limit: usize,
    model_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let model_dir = model_dir
        .canonicalize()
        .with_context(|| format!("model directory '{}' not found", model_dir.display()))?;

    let mut embedder =
        fractalaw_ai::Embedder::load(&model_dir).context("loading embedding model")?;

    let lance = LanceStore::open(&data_dir.join("lancedb"))
        .await
        .context("opening LanceDB")?;

    let query_vec = embedder.embed(query).context("embedding query")?;
    let batches = lance.search_text(&query_vec, limit).await?;

    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    if total == 0 {
        println!("No results.");
        return Ok(());
    }

    let projected = project_batches(
        &batches,
        &["law_name", "provision", "section_type", "text", "_distance"],
    );
    print_batches(&projected)?;
    Ok(())
}

async fn cmd_validate(
    store: &DuckStore,
    data_dir: &std::path::Path,
    model_dir: &std::path::Path,
) -> anyhow::Result<()> {
    use std::sync::Arc;

    use datafusion::datasource::memory::MemTable;
    use futures::TryStreamExt;
    use lancedb::query::ExecutableQuery;

    let model_dir = model_dir
        .canonicalize()
        .with_context(|| format!("model directory '{}' not found", model_dir.display()))?;

    let lance = LanceStore::open(&data_dir.join("lancedb"))
        .await
        .context("opening LanceDB")?;

    println!("=== Validation ===\n");

    let mut passed = 0u32;
    let total_checks = 4u32;

    // ── Check 1: Row counts ──
    let lance_count = lance.legislation_text_count().await?;
    let source_batches = fractalaw_store::read_parquet(&data_dir.join("legislation_text.parquet"))?;
    let source_count: usize = source_batches.iter().map(|b| b.num_rows()).sum();
    drop(source_batches);

    if lance_count == source_count {
        println!("  [PASS] Legislation text rows: {}", fmt_num(lance_count));
        passed += 1;
    } else {
        println!(
            "  [FAIL] Legislation text rows: {} in Lance vs {} in Parquet",
            fmt_num(lance_count),
            fmt_num(source_count)
        );
    }

    // ── Check 2: Embedding coverage ──
    let table = lance.legislation_text().await?;
    let embedded_count = table
        .count_rows(Some("embedded_at IS NOT NULL".to_string()))
        .await?;

    if embedded_count == lance_count {
        println!(
            "  [PASS] Embedding coverage: {} / {} (100%)",
            fmt_num(embedded_count),
            fmt_num(lance_count)
        );
        passed += 1;
    } else {
        let pct = if lance_count > 0 {
            embedded_count as f64 / lance_count as f64 * 100.0
        } else {
            0.0
        };
        println!(
            "  [FAIL] Embedding coverage: {} / {} ({pct:.1}%)",
            fmt_num(embedded_count),
            fmt_num(lance_count)
        );
    }

    // ── Check 3: Cross-store join ──
    let fusion = FusionStore::new(store)?;

    // Register only legislation_text from Lance (amendment_annotations may not exist).
    {
        let text_table = lance.legislation_text().await?;
        let text_batches: Vec<RecordBatch> = text_table
            .query()
            .execute()
            .await
            .map_err(|e| anyhow::anyhow!("lance query: {e}"))?
            .try_collect()
            .await
            .map_err(|e| anyhow::anyhow!("lance collect: {e}"))?;

        if let Some(first) = text_batches.first() {
            let schema = first.schema();
            let mem = MemTable::try_new(schema, vec![text_batches])?;
            fusion
                .context()
                .register_table("legislation_text", Arc::new(mem))
                .map_err(|e| anyhow::anyhow!("register legislation_text: {e}"))?;
        }
    }

    let batches = fusion
        .query(
            "SELECT count(DISTINCT t.law_name) AS matched \
             FROM legislation_text t \
             JOIN legislation l ON t.law_name = l.name",
        )
        .await?;
    let matched = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .unwrap()
        .value(0);

    let batches = fusion
        .query("SELECT count(DISTINCT law_name) AS total FROM legislation_text")
        .await?;
    let text_laws = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .unwrap()
        .value(0);

    if matched == text_laws {
        println!("  [PASS] Cross-store join: {matched} / {text_laws} laws matched");
        passed += 1;
    } else if matched > 0 {
        // Some LAT data covers laws not in the legislation export — expected.
        println!(
            "  [PASS] Cross-store join: {matched} / {text_laws} laws matched ({} unmatched in legislation)",
            text_laws - matched
        );
        passed += 1;
    } else {
        println!("  [FAIL] Cross-store join: {matched} / {text_laws} laws matched");
    }

    // ── Check 4: Semantic smoke test ──
    let mut embedder =
        fractalaw_ai::Embedder::load(&model_dir).context("loading embedding model")?;
    let query_text = "chemical exposure limits";
    let query_vec = embedder.embed(query_text)?;
    let results = lance.search_text(&query_vec, 5).await?;

    let mut found = false;
    let mut top_law = String::new();

    'outer: for batch in &results {
        let law_col = batch.column_by_name("law_name");
        let text_col = batch.column_by_name("text");

        for row in 0..batch.num_rows() {
            let law = law_col.and_then(|c| get_string_value(c.as_ref(), row));
            let text = text_col.and_then(|c| get_string_value(c.as_ref(), row));

            if top_law.is_empty()
                && let Some(ref l) = law
            {
                top_law.clone_from(l);
            }

            let combined = format!(
                "{} {}",
                law.as_deref().unwrap_or(""),
                text.as_deref().unwrap_or("")
            )
            .to_lowercase();

            if combined.contains("coshh")
                || combined.contains("chemical")
                || combined.contains("hazardous")
                || combined.contains("exposure")
            {
                if let Some(ref l) = law {
                    top_law.clone_from(l);
                }
                found = true;
                break 'outer;
            }
        }
    }

    if found {
        println!("  [PASS] Semantic search: \"{query_text}\" → {top_law}");
        passed += 1;
    } else {
        println!(
            "  [FAIL] Semantic search: \"{query_text}\" → no COSHH/chemical match in top 5 (top: {top_law})"
        );
    }

    // ── Summary ──
    println!("\n=== {passed}/{total_checks} checks passed ===");

    if passed < total_checks {
        anyhow::bail!("{} check(s) failed", total_checks - passed);
    }

    Ok(())
}

fn cmd_stats(store: &DuckStore) -> anyhow::Result<()> {
    let leg_count = store.legislation_count()?;
    let edge_count = store.law_edges_count()?;

    println!("=== Dataset Summary ===\n");
    println!("  Legislation:  {:>8} rows", leg_count);
    println!("  Law Edges:    {:>8} rows", edge_count);

    // Year range.
    let batches = store
        .query_arrow("SELECT min(year) AS min_year, max(year) AS max_year FROM legislation")?;
    println!();
    print_batches(&batches)?;

    // Status breakdown.
    println!("\n--- Status Breakdown ---\n");
    let batches = store.query_arrow(
        "SELECT status, count(*) AS count FROM legislation GROUP BY status ORDER BY count DESC",
    )?;
    print_batches(&batches)?;

    // Edge type breakdown.
    println!("\n--- Edge Types ---\n");
    let batches = store.query_arrow(
        "SELECT edge_type, count(*) AS count FROM law_edges GROUP BY edge_type ORDER BY count DESC",
    )?;
    print_batches(&batches)?;

    // Jurisdiction breakdown.
    println!("\n--- Jurisdictions ---\n");
    let batches = store.query_arrow(
        "SELECT jurisdiction, count(*) AS count FROM legislation GROUP BY jurisdiction ORDER BY count DESC",
    )?;
    print_batches(&batches)?;

    Ok(())
}

/// Project RecordBatches to only include the specified columns.
fn project_batches(batches: &[RecordBatch], columns: &[&str]) -> Vec<RecordBatch> {
    batches
        .iter()
        .filter_map(|batch| {
            let schema = batch.schema();
            let indices: Vec<usize> = columns
                .iter()
                .filter_map(|name| schema.index_of(name).ok())
                .collect();
            if indices.is_empty() {
                None
            } else {
                batch.project(&indices).ok()
            }
        })
        .collect()
}

/// Format a number with comma thousands separators.
fn fmt_num(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Extract a string value from an Arrow array, handling both Utf8 and LargeUtf8.
fn get_string_value(col: &dyn arrow::array::Array, i: usize) -> Option<String> {
    use arrow::array::{Array, LargeStringArray, StringArray};
    if let Some(arr) = col.as_any().downcast_ref::<StringArray>()
        && !arr.is_null(i)
    {
        return Some(arr.value(i).to_string());
    } else if let Some(arr) = col.as_any().downcast_ref::<LargeStringArray>()
        && !arr.is_null(i)
    {
        return Some(arr.value(i).to_string());
    }
    None
}
