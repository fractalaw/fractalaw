mod display;

use std::path::PathBuf;

use anyhow::Context;
use arrow::util::pretty::print_batches;
use clap::{Parser, Subcommand};
use fractalaw_store::{DuckStore, FusionStore, StoreError};

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let data_dir = cli
        .data_dir
        .canonicalize()
        .with_context(|| format!("data directory '{}' not found", cli.data_dir.display()))?;

    let store = DuckStore::open()?;
    store.load_all(&data_dir)?;

    match cli.command {
        Command::Query { sql } => cmd_query(&store, &sql).await,
        Command::Law { name } => cmd_law(&store, &name),
        Command::Graph { name, hops } => cmd_graph(&store, &name, hops),
        Command::Stats => cmd_stats(&store),
    }
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
