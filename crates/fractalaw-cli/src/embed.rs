//! Embedding pipeline: reads LAT text, generates ONNX embeddings, writes to LanceDB.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use arrow::array::{
    Array, FixedSizeListBuilder, Float32Builder, LargeStringArray, ListBuilder, StringArray,
    TimestampNanosecondArray, UInt32Builder,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use fractalaw_ai::Embedder;
use fractalaw_store::LanceStore;

const EMBED_BATCH_SIZE: usize = 256;
const MODEL_NAME: &str = "all-MiniLM-L6-v2";
const EMBED_DIM: i32 = 384;

pub struct EmbedStats {
    pub total_rows: usize,
    pub elapsed_secs: f64,
}

/// Run the full embedding pipeline: read Parquet → embed text → write to LanceDB.
pub async fn run_embed_pipeline(
    lance: &LanceStore,
    embedder: &mut Embedder,
    parquet_path: &Path,
) -> anyhow::Result<EmbedStats> {
    let start = Instant::now();

    // 1. Read source Parquet.
    let source_batches =
        fractalaw_store::read_parquet(parquet_path).context("reading legislation_text.parquet")?;

    let total_rows: usize = source_batches.iter().map(|b| b.num_rows()).sum();
    eprintln!("  Read {total_rows} rows from {}", parquet_path.display());

    if source_batches.is_empty() {
        return Ok(EmbedStats {
            total_rows: 0,
            elapsed_secs: 0.0,
        });
    }

    // 2. Build output schema (fix embedding column types from DuckDB's FLOAT[] to FixedSizeList).
    let output_schema = build_embedded_schema(&source_batches[0].schema());

    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;

    // 3. Process each batch: extract text, embed, rebuild with embedding columns.
    let mut output_batches = Vec::with_capacity(source_batches.len());
    let mut processed = 0usize;

    for batch in &source_batches {
        let n = batch.num_rows();

        // Extract text column.
        let texts = extract_texts(batch);

        // Generate embeddings and token IDs in sub-batches of 256.
        let mut embeddings = Vec::with_capacity(n);
        let mut all_token_ids = Vec::with_capacity(n);
        for chunk in texts.chunks(EMBED_BATCH_SIZE) {
            let batch_embs = embedder
                .embed_batch(chunk)
                .context("generating embeddings")?;
            let batch_tokens = embedder.tokenize_batch(chunk).context("tokenizing")?;
            embeddings.extend(batch_embs);
            all_token_ids.extend(batch_tokens);
        }

        // Build output batch with embeddings and token IDs populated.
        let output = replace_embedding_columns(
            batch,
            &output_schema,
            &embeddings,
            &all_token_ids,
            now_nanos,
        )?;
        output_batches.push(output);

        processed += n;
        eprint!(
            "\r  Embedded {processed}/{total_rows} ({:.1}%)",
            processed as f64 / total_rows as f64 * 100.0
        );
    }
    eprintln!();

    // 4. Create LanceDB table from embedded batches (drop-and-recreate).
    eprintln!("  Writing to LanceDB...");
    lance
        .create_table_from_batches("legislation_text", output_batches)
        .await
        .context("writing embedded table to LanceDB")?;

    let elapsed = start.elapsed().as_secs_f64();
    Ok(EmbedStats {
        total_rows,
        elapsed_secs: elapsed,
    })
}

/// Build output schema, replacing DuckDB's `FLOAT[]` with `FixedSizeList<Float32, 384>`
/// and ensuring `embedded_at` uses nanosecond timestamps.
fn build_embedded_schema(source_schema: &Schema) -> Arc<Schema> {
    let mut fields: Vec<Field> = source_schema
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();

    let emb_idx = source_schema.index_of("embedding").unwrap();
    let ts_idx = source_schema.index_of("embedded_at").unwrap();

    fields[emb_idx] = Field::new(
        "embedding",
        DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            EMBED_DIM,
        ),
        true,
    );

    fields[ts_idx] = Field::new(
        "embedded_at",
        DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into())),
        true,
    );

    // Insert token columns after embedded_at (source Parquet doesn't have these).
    let insert_at = ts_idx + 1;
    fields.insert(
        insert_at,
        Field::new(
            "token_ids",
            DataType::List(Arc::new(Field::new("item", DataType::UInt32, false))),
            true,
        ),
    );
    fields.insert(
        insert_at + 1,
        Field::new("tokenizer_model", DataType::Utf8, true),
    );

    Arc::new(Schema::new(fields))
}

/// Replace embedding columns and insert token columns into a RecordBatch.
fn replace_embedding_columns(
    batch: &RecordBatch,
    schema: &Arc<Schema>,
    embeddings: &[Vec<f32>],
    token_ids: &[Vec<u32>],
    now_nanos: i64,
) -> anyhow::Result<RecordBatch> {
    let n = batch.num_rows();
    let source_schema = batch.schema();

    let emb_idx = source_schema.index_of("embedding").unwrap();
    let model_idx = source_schema.index_of("embedding_model").unwrap();
    let ts_idx = source_schema.index_of("embedded_at").unwrap();

    // Clone all columns (cheap Arc clones), then replace embedding ones.
    let mut columns: Vec<Arc<dyn Array>> = batch.columns().to_vec();

    // embedding: FixedSizeList<Float32, 384>
    let mut emb_builder = FixedSizeListBuilder::new(Float32Builder::new(), EMBED_DIM);
    for emb in embeddings {
        let values = emb_builder.values();
        for &val in emb {
            values.append_value(val);
        }
        emb_builder.append(true);
    }
    columns[emb_idx] = Arc::new(emb_builder.finish());

    // embedding_model: Utf8
    columns[model_idx] = Arc::new(StringArray::from(vec![MODEL_NAME; n]));

    // embedded_at: Timestamp(Nanosecond, UTC)
    columns[ts_idx] =
        Arc::new(TimestampNanosecondArray::from(vec![now_nanos; n]).with_timezone("UTC"));

    // Insert token_ids and tokenizer_model after embedded_at
    // (these columns don't exist in the source Parquet).
    let insert_at = ts_idx + 1;

    // token_ids: List<non-null UInt32>
    let mut list_builder = ListBuilder::new(UInt32Builder::new()).with_field(Field::new(
        "item",
        DataType::UInt32,
        false,
    ));
    for ids in token_ids {
        let values = list_builder.values();
        for &id in ids {
            values.append_value(id);
        }
        list_builder.append(true);
    }
    columns.insert(insert_at, Arc::new(list_builder.finish()));

    // tokenizer_model: Utf8
    columns.insert(
        insert_at + 1,
        Arc::new(StringArray::from(vec![MODEL_NAME; n])),
    );

    Ok(RecordBatch::try_new(schema.clone(), columns)?)
}

/// Extract text strings from a RecordBatch's "text" column.
///
/// Handles both `Utf8` (StringArray) and `LargeUtf8` (LargeStringArray).
fn extract_texts(batch: &RecordBatch) -> Vec<&str> {
    let col = batch.column_by_name("text").unwrap();
    if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
        (0..arr.len()).map(|i| arr.value(i)).collect()
    } else if let Some(arr) = col.as_any().downcast_ref::<LargeStringArray>() {
        (0..arr.len()).map(|i| arr.value(i)).collect()
    } else {
        panic!("unexpected text column type: {:?}", col.data_type());
    }
}
