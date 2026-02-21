//! Minimal Arrow IPC streaming format extractors.
//!
//! These parse just enough of the Arrow IPC stream to extract simple scalar values
//! from single-row, single-column query results. This avoids pulling the full `arrow`
//! crate into the WASM guest.
//!
//! The Arrow IPC streaming format is:
//!   1. Schema message (SCHEMA flatbuffer)
//!   2. RecordBatch messages (RECORD_BATCH flatbuffer header + body)
//!   3. End-of-stream marker (continuation: 0xFFFFFFFF, size: 0x00000000)
//!
//! Each message is prefixed with:
//!   - 4 bytes: continuation marker (0xFFFFFFFF)
//!   - 4 bytes: metadata size (little-endian i32)
//!   - N bytes: flatbuffer metadata
//!   - body bytes (for RecordBatch: validity + offsets + data)

/// Extract a BIGINT (i64) from a single-row, single-column Arrow IPC stream.
///
/// Use with queries like: `SELECT count(*)::BIGINT AS n FROM ...`
pub fn extract_i64(ipc: &[u8]) -> Option<i64> {
    // We need to find the RecordBatch body which contains the i64 value.
    // Strategy: the i64 is 8 bytes, aligned, in the body of the second message.
    // Walk through messages to find the RecordBatch body.
    let mut pos = 0;
    let mut message_idx = 0;

    while pos + 8 <= ipc.len() {
        // Read continuation marker
        let cont = read_i32(ipc, pos);
        if cont != -1 {
            // Pre-1.0 format without continuation — metadata size is at pos directly
            return None;
        }
        pos += 4;

        // Read metadata size
        let meta_size = read_i32(ipc, pos) as usize;
        pos += 4;

        if meta_size == 0 {
            // End-of-stream marker
            break;
        }

        // Skip metadata flatbuffer (padded to 8-byte boundary)
        let meta_padded = (meta_size + 7) & !7;
        let meta_start = pos;
        pos += meta_padded;

        if message_idx == 0 {
            // First message is Schema — skip it
            message_idx += 1;
            continue;
        }

        // Second message is RecordBatch — extract body length from flatbuffer
        // The Message flatbuffer has: bodyLength at offset in the table.
        // Rather than fully parse the flatbuffer, read the body that follows.
        // The body for a single non-null i64 column is:
        //   - 0 bytes validity (no nulls, so bitmap can be omitted or 8-byte padded)
        //   - 8 bytes: the i64 value
        // However, the body length is encoded in the flatbuffer.
        // Simpler: extract body_length from the Message flatbuffer.
        let body_length = extract_body_length(ipc, meta_start, meta_size)?;
        let body_start = pos;
        pos += body_length;

        // Body layout for a single non-null BIGINT column (1 row):
        //   - Buffer 0 (validity bitmap): starts at offset 0, length 8, padded to 64
        //   - Buffer 1 (data): starts at offset 64, length 8
        //   Arrow aligns buffers to 64-byte boundaries by default.
        if body_length >= 72 {
            return Some(read_i64(ipc, body_start + 64));
        }
        // Fallback for compact bodies (no 64-byte alignment).
        if body_length >= 16 {
            return Some(read_i64(ipc, body_start + 8));
        }
        if body_length >= 8 {
            return Some(read_i64(ipc, body_start));
        }

        message_idx += 1;
    }

    None
}

/// Extract a VARCHAR string from a single-row, single-column Arrow IPC stream.
///
/// Use with queries like: `SELECT to_json(...) AS j FROM ...`
pub fn extract_string(ipc: &[u8]) -> Option<String> {
    let mut pos = 0;
    let mut message_idx = 0;

    while pos + 8 <= ipc.len() {
        let cont = read_i32(ipc, pos);
        if cont != -1 {
            return None;
        }
        pos += 4;

        let meta_size = read_i32(ipc, pos) as usize;
        pos += 4;

        if meta_size == 0 {
            break;
        }

        let meta_start = pos;
        let meta_padded = (meta_size + 7) & !7;
        pos += meta_padded;

        if message_idx == 0 {
            message_idx += 1;
            continue;
        }

        let body_length = extract_body_length(ipc, meta_start, meta_size)?;
        let body_start = pos;

        // Body layout for Utf8 column (1 row, non-null):
        //   - Validity bitmap: 0 or 8 bytes (often omitted for all-valid)
        //   - Offsets: (n+1) * 4 bytes = 8 bytes for 1 row: [0_i32, len_i32]
        //   - Data: the string bytes, padded to 8-byte boundary
        //
        // We scan the body for the offsets array pattern: two i32 values where
        // offsets[0] == 0 and offsets[1] > 0, followed by that many bytes of data.
        return extract_string_from_body(ipc, body_start, body_length);
    }

    None
}

/// Extract a string from a RecordBatch body containing a single Utf8 column, 1 row.
fn extract_string_from_body(ipc: &[u8], body_start: usize, body_length: usize) -> Option<String> {
    let body_end = body_start + body_length;

    // Arrow body layout for a single Utf8 column (1 row, non-null):
    //   Buffer 0 (validity):  offset 0, padded to 64 bytes
    //   Buffer 1 (offsets):   offset 64, 8 bytes (2 x i32), padded to 64 bytes
    //   Buffer 2 (data):      offset 128, string_len bytes
    // Arrow aligns each buffer to 64-byte boundaries by default.

    // Try 64-byte aligned layout first (standard Arrow IPC writer).
    for &(offsets_off, data_off) in &[(64usize, 128usize), (8, 16), (0, 8)] {
        if body_start + offsets_off + 8 > body_end {
            continue;
        }

        let offset0 = read_i32(ipc, body_start + offsets_off) as usize;
        let offset1 = read_i32(ipc, body_start + offsets_off + 4) as usize;

        if offset0 == 0 && offset1 > 0 && offset1 < body_length {
            let str_start = body_start + data_off;
            if str_start + offset1 <= body_end {
                let s = &ipc[str_start..str_start + offset1];
                return String::from_utf8(s.to_vec()).ok();
            }
        }
    }

    None
}

/// Extract bodyLength from a Message flatbuffer.
///
/// The Arrow Message flatbuffer schema (simplified):
///   table Message { version, header_type, header, bodyLength }
///
/// In FlatBuffers, the root table starts with a vtable offset at the root.
/// bodyLength is typically at field index 3 (0-indexed) → vtable slot 10.
fn extract_body_length(ipc: &[u8], meta_start: usize, meta_size: usize) -> Option<usize> {
    if meta_size < 8 {
        return None;
    }

    // FlatBuffer root: first 4 bytes are offset to the root table
    let root_offset = read_i32(ipc, meta_start) as usize;
    let table_start = meta_start + root_offset;

    if table_start + 4 > meta_start + meta_size {
        return None;
    }

    // vtable is at table_start - vtable_offset (signed)
    let vtable_soffset = read_i32(ipc, table_start);
    let vtable_start = (table_start as i64 - vtable_soffset as i64) as usize;

    if vtable_start + 4 > meta_start + meta_size {
        return None;
    }

    // vtable layout: [vtable_size: u16, table_size: u16, field0_offset: u16, field1_offset: u16, ...]
    let vtable_size = read_u16(ipc, vtable_start) as usize;

    // bodyLength is field index 3 → vtable byte offset 4 + 3*2 = 10
    let body_length_voffset = 10usize;
    if body_length_voffset + 2 > vtable_size {
        // Field not present in vtable → bodyLength is 0
        return Some(0);
    }

    let field_offset = read_u16(ipc, vtable_start + body_length_voffset) as usize;
    if field_offset == 0 {
        return Some(0);
    }

    // bodyLength is an i64 at table_start + field_offset
    let body_length = read_i64(ipc, table_start + field_offset);
    Some(body_length as usize)
}

fn read_i32(buf: &[u8], pos: usize) -> i32 {
    i32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
}

fn read_i64(buf: &[u8], pos: usize) -> i64 {
    i64::from_le_bytes([
        buf[pos],
        buf[pos + 1],
        buf[pos + 2],
        buf[pos + 3],
        buf[pos + 4],
        buf[pos + 5],
        buf[pos + 6],
        buf[pos + 7],
    ])
}

fn read_u16(buf: &[u8], pos: usize) -> u16 {
    u16::from_le_bytes([buf[pos], buf[pos + 1]])
}
