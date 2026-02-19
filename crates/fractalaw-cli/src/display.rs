//! Vertical card display for legislation records.
//!
//! Renders a single-row RecordBatch as a grouped, human-readable card
//! with type-aware formatting for scalars, lists, and `List<Struct>` columns.

use arrow::array::*;
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;

const MAX_LIST_ITEMS: usize = 10;

// ── Schema section groupings ──

const IDENTITY: &[&str] = &[
    "name",
    "jurisdiction",
    "source_authority",
    "source_url",
    "type_code",
    "type_desc",
    "type_class",
    "year",
    "number",
    "old_style_number",
    "title",
    "language",
];

const CLASSIFICATION: &[&str] = &[
    "domain",
    "family",
    "sub_family",
    "si_code",
    "description",
    "subjects",
];

const DATES: &[&str] = &[
    "primary_date",
    "made_date",
    "enactment_date",
    "in_force_date",
    "valid_date",
    "modified_date",
    "restrict_start_date",
    "latest_amend_date",
    "latest_rescind_date",
];

const EXTENT: &[&str] = &[
    "extent_code",
    "extent_regions",
    "extent_national",
    "extent_detail",
    "restrict_extent",
];

const DOC_STATS: &[&str] = &[
    "total_paras",
    "body_paras",
    "schedule_paras",
    "attachment_paras",
    "images",
];

const STATUS: &[&str] = &[
    "status",
    "status_source",
    "status_conflict",
    "status_conflict_detail",
];

const FUNCTION: &[&str] = &[
    "function",
    "is_making",
    "is_commencing",
    "is_amending",
    "is_enacting",
    "is_rescinding",
];

const RELATIONSHIPS: &[&str] = &[
    "enacted_by",
    "enacting",
    "amending",
    "amended_by",
    "rescinding",
    "rescinded_by",
];

const AMEND_STATS: &[&str] = &[
    "self_affects_count",
    "affects_count",
    "affected_laws_count",
    "affected_by_count",
    "affected_by_laws_count",
    "rescinding_laws_count",
    "rescinded_by_laws_count",
];

const DRRP_LISTS: &[&str] = &[
    "duty_holder",
    "rights_holder",
    "responsibility_holder",
    "power_holder",
    "duty_type",
    "role",
    "role_gvt",
];

const DRRP_STRUCTS: &[&str] = &["duties", "rights", "responsibilities", "powers"];

const ANNOTATIONS: &[&str] = &[
    "total_text_amendments",
    "total_modifications",
    "total_commencements",
    "total_extents",
];

const TIMESTAMPS: &[&str] = &["created_at", "updated_at"];

// ── Public API ──

/// Print a single legislation record as a vertical card grouped by schema section.
pub fn print_law_card(batch: &RecordBatch) -> anyhow::Result<()> {
    let name = get_utf8(batch, "name").unwrap_or_default();
    let title = get_utf8(batch, "title").unwrap_or_default();

    println!("=== {} ===", name);
    if !title.is_empty() {
        println!("{}", title);
    }
    println!();

    print_section(batch, "Identity", IDENTITY);
    print_section(batch, "Classification", CLASSIFICATION);
    print_section(batch, "Dates", DATES);
    print_section(batch, "Territorial Extent", EXTENT);
    print_section(batch, "Document Statistics", DOC_STATS);
    print_section(batch, "Status", STATUS);
    print_section(batch, "Function", FUNCTION);
    print_section(batch, "Relationships", RELATIONSHIPS);
    print_section(batch, "Amendment Statistics", AMEND_STATS);
    print_section(batch, "DRRP Taxa", DRRP_LISTS);
    print_section(batch, "DRRP Detail", DRRP_STRUCTS);
    print_section(batch, "Annotation Totals", ANNOTATIONS);
    print_section(batch, "Timestamps", TIMESTAMPS);

    Ok(())
}

// ── Section rendering ──

fn print_section(batch: &RecordBatch, header: &str, cols: &[&str]) {
    // Check if any column in this section has a non-null value.
    let has_data = cols.iter().any(|&col| {
        batch
            .schema()
            .index_of(col)
            .ok()
            .is_some_and(|i| !batch.column(i).is_null(0))
    });
    if !has_data {
        return;
    }

    println!("{header}");
    for &col_name in cols {
        let idx = match batch.schema().index_of(col_name) {
            Ok(i) => i,
            Err(_) => continue,
        };
        let col = batch.column(idx);
        if col.is_null(0) {
            continue;
        }

        let schema = batch.schema();
        let field = schema.field(idx);
        match field.data_type() {
            DataType::Utf8 => {
                let arr = col.as_any().downcast_ref::<StringArray>().unwrap();
                println!("  {:<26} {}", col_name, arr.value(0));
            }
            DataType::LargeUtf8 => {
                let arr = col.as_any().downcast_ref::<LargeStringArray>().unwrap();
                println!("  {:<26} {}", col_name, arr.value(0));
            }
            DataType::Int32 => {
                let arr = col.as_any().downcast_ref::<Int32Array>().unwrap();
                println!("  {:<26} {}", col_name, arr.value(0));
            }
            DataType::Int64 => {
                let arr = col.as_any().downcast_ref::<Int64Array>().unwrap();
                println!("  {:<26} {}", col_name, arr.value(0));
            }
            DataType::Date32 => {
                let formatted = arrow::util::display::ArrayFormatter::try_new(
                    col.as_ref(),
                    &Default::default(),
                );
                match formatted {
                    Ok(fmt) => println!("  {:<26} {}", col_name, fmt.value(0)),
                    Err(_) => println!("  {:<26} (date)", col_name),
                }
            }
            DataType::Boolean => {
                let arr = col.as_any().downcast_ref::<BooleanArray>().unwrap();
                println!(
                    "  {:<26} {}",
                    col_name,
                    if arr.value(0) { "yes" } else { "no" }
                );
            }
            DataType::Timestamp(_, _) => {
                // Use Arrow's display formatting for timestamps.
                let formatted = arrow::util::display::ArrayFormatter::try_new(
                    col.as_ref(),
                    &Default::default(),
                );
                match formatted {
                    Ok(fmt) => println!("  {:<26} {}", col_name, fmt.value(0)),
                    Err(_) => println!("  {:<26} (timestamp)", col_name),
                }
            }
            DataType::List(inner) => match inner.data_type() {
                DataType::Utf8 => print_list_utf8(col, col_name),
                DataType::LargeUtf8 => print_list_large_utf8(col, col_name),
                DataType::Struct(fields) => {
                    if fields.len() == 5 && fields.iter().any(|f| f.name() == "latest_date") {
                        print_list_related_law(col, col_name);
                    } else if fields.len() == 4 && fields.iter().any(|f| f.name() == "holder") {
                        print_list_drrp_entry(col, col_name);
                    } else {
                        print_list_generic_struct(col, col_name, fields);
                    }
                }
                _ => println!("  {:<26} (list)", col_name),
            },
            _ => println!("  {:<26} {:?}", col_name, col),
        }
    }
    println!();
}

// ── List<Utf8> ──

fn print_list_utf8(col: &dyn Array, col_name: &str) {
    let list = col.as_any().downcast_ref::<ListArray>().unwrap();
    let values = list.value(0);
    let strings = values.as_any().downcast_ref::<StringArray>().unwrap();
    let items: Vec<&str> = (0..strings.len())
        .filter(|&i| !strings.is_null(i))
        .map(|i| strings.value(i))
        .collect();
    if items.is_empty() {
        return;
    }
    println!("  {:<26} {}", col_name, items.join(", "));
}

fn print_list_large_utf8(col: &dyn Array, col_name: &str) {
    let list = col.as_any().downcast_ref::<ListArray>().unwrap();
    let values = list.value(0);
    let strings = values.as_any().downcast_ref::<LargeStringArray>().unwrap();
    let items: Vec<&str> = (0..strings.len())
        .filter(|&i| !strings.is_null(i))
        .map(|i| strings.value(i))
        .collect();
    if items.is_empty() {
        return;
    }
    println!("  {:<26} {}", col_name, items.join(", "));
}

// ── List<Struct> — RelatedLaw ──

fn print_list_related_law(col: &dyn Array, col_name: &str) {
    let list = col.as_any().downcast_ref::<ListArray>().unwrap();
    let values = list.value(0);
    let structs = values.as_any().downcast_ref::<StructArray>().unwrap();
    let len = structs.len();
    if len == 0 {
        return;
    }

    println!("  {} ({}):", col_name, len);

    let names = struct_utf8_col(structs, "name");
    let titles = struct_utf8_col(structs, "title");
    let years = struct_int32_col(structs, "year");
    let counts = struct_int32_col(structs, "count");

    let show = len.min(MAX_LIST_ITEMS);
    for i in 0..show {
        let name = names
            .as_ref()
            .and_then(|a| col_str(a.as_ref(), i))
            .unwrap_or("-");
        let title = titles
            .as_ref()
            .and_then(|a| col_str(a.as_ref(), i))
            .unwrap_or("");
        let year = years.as_ref().and_then(|a| col_i32(a.as_ref(), i));
        let count = counts.as_ref().and_then(|a| col_i32(a.as_ref(), i));

        let title_short = if title.len() > 60 {
            format!("{}...", &title[..57])
        } else {
            title.to_string()
        };

        print!("    {:<30}", name);
        if let Some(y) = year {
            print!("  {}", y);
        }
        if let Some(c) = count
            && c > 0
        {
            print!("  ({} amendments)", c);
        }
        println!();
        if !title_short.is_empty() {
            println!("      {}", title_short);
        }
    }
    if len > MAX_LIST_ITEMS {
        println!("    ... and {} more", len - MAX_LIST_ITEMS);
    }
}

// ── List<Struct> — DRRPEntry ──

fn print_list_drrp_entry(col: &dyn Array, col_name: &str) {
    let list = col.as_any().downcast_ref::<ListArray>().unwrap();
    let values = list.value(0);
    let structs = values.as_any().downcast_ref::<StructArray>().unwrap();
    let len = structs.len();
    if len == 0 {
        return;
    }

    println!("  {} ({}):", col_name, len);

    let holders = struct_utf8_col(structs, "holder");
    let duty_types = struct_utf8_col(structs, "duty_type");
    let clauses = struct_utf8_col(structs, "clause");
    let articles = struct_utf8_col(structs, "article");

    let show = len.min(MAX_LIST_ITEMS);
    for i in 0..show {
        let holder = holders
            .as_ref()
            .and_then(|a| col_str(a.as_ref(), i))
            .unwrap_or("-");
        let dtype = duty_types
            .as_ref()
            .and_then(|a| col_str(a.as_ref(), i))
            .unwrap_or("");
        let clause = clauses
            .as_ref()
            .and_then(|a| col_str(a.as_ref(), i))
            .unwrap_or("");
        let article = articles
            .as_ref()
            .and_then(|a| col_str(a.as_ref(), i))
            .unwrap_or("");

        print!("    holder: {:<20}", holder);
        if !dtype.is_empty() {
            print!("  type: {}", dtype);
        }
        if !clause.is_empty() {
            print!("  clause: {}", clause);
        }
        if !article.is_empty() {
            print!("  article: {}", article);
        }
        println!();
    }
    if len > MAX_LIST_ITEMS {
        println!("    ... and {} more", len - MAX_LIST_ITEMS);
    }
}

// ── List<Struct> — generic fallback ──

fn print_list_generic_struct(col: &dyn Array, col_name: &str, fields: &arrow::datatypes::Fields) {
    let list = col.as_any().downcast_ref::<ListArray>().unwrap();
    let values = list.value(0);
    let structs = values.as_any().downcast_ref::<StructArray>().unwrap();
    let len = structs.len();
    if len == 0 {
        return;
    }
    let field_names: Vec<&str> = fields.iter().map(|f| f.name().as_str()).collect();
    println!(
        "  {} ({} items, fields: {})",
        col_name,
        len,
        field_names.join(", ")
    );
}

// ── Helpers ──

fn get_utf8(batch: &RecordBatch, col_name: &str) -> Option<String> {
    let idx = batch.schema().index_of(col_name).ok()?;
    let col = batch.column(idx);
    if col.is_null(0) {
        return None;
    }
    // Try Utf8 first, then LargeUtf8.
    if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
        return Some(arr.value(0).to_string());
    }
    if let Some(arr) = col.as_any().downcast_ref::<LargeStringArray>() {
        return Some(arr.value(0).to_string());
    }
    None
}

/// Extract a named string column from a StructArray.
fn struct_utf8_col(structs: &StructArray, name: &str) -> Option<ArrayRef> {
    let idx = structs.fields().iter().position(|f| f.name() == name)?;
    Some(structs.column(idx).clone())
}

/// Extract a named Int32 column from a StructArray.
fn struct_int32_col(structs: &StructArray, name: &str) -> Option<ArrayRef> {
    let idx = structs.fields().iter().position(|f| f.name() == name)?;
    Some(structs.column(idx).clone())
}

/// Get a string value from a column that might be Utf8 or LargeUtf8.
fn col_str(col: &dyn Array, i: usize) -> Option<&str> {
    if col.is_null(i) {
        return None;
    }
    if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
        return Some(arr.value(i));
    }
    if let Some(arr) = col.as_any().downcast_ref::<LargeStringArray>() {
        return Some(arr.value(i));
    }
    None
}

/// Get an i32 value from a column.
fn col_i32(col: &dyn Array, i: usize) -> Option<i32> {
    if col.is_null(i) {
        return None;
    }
    col.as_any()
        .downcast_ref::<Int32Array>()
        .map(|a| a.value(i))
}
