//! Sort key normalisation for UK provision numbers.
//!
//! Converts bare provision numbers (e.g., "3", "3A", "41ZA", "19DZA") into
//! lexicographically-sortable strings so that `ORDER BY sort_key` recovers
//! correct document order.
//!
//! # UK legal numbering conventions
//!
//! - Plain numeric: s.1, s.2, ..., s.10
//! - Letter suffix (amendment insertion): s.3A between s.3 and s.4
//! - Z-prefix (pre-insertion): s.3ZA between s.3 and s.3A
//! - Double letter: s.3AA, s.3AB after s.3A
//! - Combined: s.19DZA = section 19, suffix D, then Z-prefix A

/// Normalise a provision number into a lexicographically-sortable string.
///
/// Input: bare number like "3", "3A", "41ZA", "19DZA"
/// Output: "003.000.000", "003.010.000", "041.001.000", "019.040.001"
///
/// # Algorithm
///
/// 1. Extract leading ASCII digits → base number (zero-padded to 3 digits)
/// 2. Parse remaining uppercase letters into up to 2 suffix groups:
///    - Z-prefix group: ZA=001, ZB=002, ..., ZZ=026 (sorts before plain letters)
///    - Plain letter: A=010, B=020, ..., Z=260 (gaps of 10 for future insertions)
/// 3. Pad to exactly 3 segments with "000"
/// 4. Join with "."
pub fn normalize_provision(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "000.000.000".to_string();
    }

    let upper = s.to_ascii_uppercase();
    let bytes = upper.as_bytes();

    // Extract leading digits.
    let digit_end = bytes
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(bytes.len());
    let base: u32 = if digit_end > 0 {
        upper[..digit_end].parse().unwrap_or(0)
    } else {
        0
    };

    // Parse suffix groups from remaining characters.
    let suffix = &bytes[digit_end..];
    let mut segments: Vec<u32> = vec![base];
    let mut i = 0;

    while i < suffix.len() && segments.len() < 3 {
        if suffix[i] == b'Z' && i + 1 < suffix.len() && suffix[i + 1].is_ascii_uppercase() {
            // Z-prefix group: ZA=001, ZB=002, ..., ZZ=026
            let letter_val = (suffix[i + 1] - b'A') as u32 + 1;
            segments.push(letter_val);
            i += 2;
        } else if suffix[i].is_ascii_uppercase() {
            // Plain letter: A=010, B=020, ..., Z=260
            let letter_val = (suffix[i] - b'A') as u32 + 1;
            segments.push(letter_val * 10);
            i += 1;
        } else {
            // Stop on unexpected character.
            break;
        }
    }

    // Pad to exactly 3 segments.
    while segments.len() < 3 {
        segments.push(0);
    }

    format!("{:03}.{:03}.{:03}", segments[0], segments[1], segments[2])
}

/// Append an extent qualifier to a sort key for parallel territorial provisions.
///
/// "003.010.000" + "E+W" → "003.010.000~E+W"
pub fn with_extent(sort_key: &str, extent: &str) -> String {
    format!("{}~{}", sort_key, extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: assert a list of inputs produces sort keys in strictly ascending order.
    fn assert_sorted_order(inputs: &[&str]) {
        let keys: Vec<String> = inputs.iter().map(|s| normalize_provision(s)).collect();
        for i in 1..keys.len() {
            assert!(
                keys[i - 1] < keys[i],
                "Expected {:?} ({}) < {:?} ({}), got {:?} >= {:?}",
                inputs[i - 1],
                keys[i - 1],
                inputs[i],
                keys[i],
                keys[i - 1],
                keys[i],
            );
        }
    }

    #[test]
    fn plain_numeric_sequence() {
        assert_sorted_order(&["1", "2", "3", "4", "5", "10", "11", "100"]);
    }

    #[test]
    fn letter_suffix_insertion() {
        assert_sorted_order(&["3", "3A", "3B", "4"]);
    }

    #[test]
    fn z_prefix_insertion() {
        assert_sorted_order(&["3", "3ZA", "3ZB", "3A", "3B", "4"]);
    }

    #[test]
    fn double_letter() {
        assert_sorted_order(&["3A", "3AA", "3AB", "3B"]);
    }

    #[test]
    fn letter_then_z_prefix() {
        assert_sorted_order(&["19D", "19DZA", "19DZB", "19DA", "19DB", "19E"]);
    }

    #[test]
    fn environment_act_real_world() {
        // Environment Act 1995: confirmed document order from position column.
        assert_sorted_order(&["40", "41", "41A", "41B", "41C", "42"]);
    }

    #[test]
    fn exact_values() {
        assert_eq!(normalize_provision("3"), "003.000.000");
        assert_eq!(normalize_provision("3ZA"), "003.001.000");
        assert_eq!(normalize_provision("3ZB"), "003.002.000");
        assert_eq!(normalize_provision("3A"), "003.010.000");
        assert_eq!(normalize_provision("3AA"), "003.010.010");
        assert_eq!(normalize_provision("3AB"), "003.010.020");
        assert_eq!(normalize_provision("3B"), "003.020.000");
        assert_eq!(normalize_provision("4"), "004.000.000");
        assert_eq!(normalize_provision("19DZA"), "019.040.001");
        assert_eq!(normalize_provision("19AZA"), "019.010.001");
    }

    #[test]
    fn empty_string() {
        assert_eq!(normalize_provision(""), "000.000.000");
    }

    #[test]
    fn just_a_number() {
        assert_eq!(normalize_provision("42"), "042.000.000");
        assert_eq!(normalize_provision("999"), "999.000.000");
    }

    #[test]
    fn lowercase_normalised() {
        assert_eq!(normalize_provision("3a"), normalize_provision("3A"));
        assert_eq!(normalize_provision("41za"), normalize_provision("41ZA"));
    }

    #[test]
    fn whitespace_trimmed() {
        assert_eq!(normalize_provision("  3A  "), normalize_provision("3A"));
    }

    #[test]
    fn with_extent_basic() {
        assert_eq!(with_extent("023.000.000", "E+W"), "023.000.000~E+W");
    }

    #[test]
    fn extent_variants_sort_together() {
        let ew = with_extent("023.000.000", "E+W");
        let ni = with_extent("023.000.000", "NI");
        let s = with_extent("023.000.000", "S");
        assert!(ew < ni);
        assert!(ni < s);
    }
}
