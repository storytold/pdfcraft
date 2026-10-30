//! The prefix-code table behind the PRC `Double` [WD 11.17.2].
//!
//! The table has 2077 rows: 29 frequent doubles and one row per IEEE binary64 exponent
//! (0..=2047). Each row is a code of 1 to 22 bits and the value it decodes to.
//! The rows come from prc-rs under its MIT licence (see [`table`]).

use std::collections::HashMap;
use std::sync::OnceLock;

mod table;

/// Rows a complete table has.
const ROWS: usize = 2077;

/// The longest code, in bits.
pub(crate) const MAX_CODE_BITS: u32 = 22;

/// What a code decodes to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Entry {
    /// A frequent value, returned with the sign bit applied.
    Double(f64),
    /// The upper 32 bits of a binary64 (exponent set, mantissa zero); the mantissa follows
    /// in the stream.
    Exponent(u32),
}

/// One row: code length, code, and what it decodes to.
type Row = (u32, u32, Entry);

fn key(nbits: u32, code: u32) -> u32 {
    (nbits << 24) | code
}

/// The rows keyed by `(nbits, code)`, or `None` when the table is incomplete or not prefix
/// free (then no `Double` decodes, rather than some decoding wrongly).
fn build(rows: &[Row]) -> Option<HashMap<u32, Entry>> {
    let mut map = HashMap::with_capacity(rows.len());
    for &(nbits, code, entry) in rows {
        if nbits == 0 || nbits > MAX_CODE_BITS || code >> nbits != 0 || map.insert(key(nbits, code), entry).is_some() {
            return None;
        }
    }
    let prefix_free = map.keys().all(|&k| {
        let (n, c) = (k >> 24, k & 0xff_ffff);
        (1..n).all(|m| !map.contains_key(&key(m, c >> (n - m))))
    });
    (map.len() == ROWS && prefix_free).then_some(map)
}

fn table() -> Option<&'static HashMap<u32, Entry>> {
    static MAP: OnceLock<Option<HashMap<u32, Entry>>> = OnceLock::new();
    MAP.get_or_init(|| build(table::TABLE)).as_ref()
}

/// The entry for an `nbits`-long code, if one exists.
pub(crate) fn lookup(nbits: u32, code: u32) -> Option<Entry> {
    table()?.get(&key(nbits, code)).copied()
}

/// The `(nbits, code)` of IEEE exponent `e`'s row, the test writer's half of [`lookup`].
#[cfg(test)]
pub(crate) fn exponent_code(e: u32) -> Option<(u32, u32)> {
    table()?.iter().find(|(_, v)| matches!(v, Entry::Exponent(hi) if (hi >> 20) & 0x7ff == e)).map(|(&k, _)| (k >> 24, k & 0xff_ffff))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn an_incomplete_or_ambiguous_table_decodes_nothing() {
        assert!(build(&[(2, 0b01, Entry::Double(0.0))]).is_none(), "one row is not the table");
        let rows: Vec<Row> = (0..ROWS as u32).map(|i| (22, i, Entry::Exponent(i))).collect();
        assert!(build(&rows).is_some(), "2077 distinct 22-bit codes are prefix free");
        let mut ambiguous = rows.clone();
        ambiguous[0] = (1, 0, Entry::Double(1.0)); // a prefix of every code starting 0
        assert!(build(&ambiguous).is_none());
        let mut too_long = rows;
        too_long[0] = (2, 0b111, Entry::Double(1.0)); // a code wider than its length
        assert!(build(&too_long).is_none());
    }

    #[test]
    fn the_table_is_complete() {
        let t = table().unwrap();
        assert_eq!(t.len(), ROWS);
        assert_eq!(t.values().filter(|e| matches!(e, Entry::Double(_))).count(), 29);
        assert!((0..2048).all(|e| exponent_code(e).is_some()));
        assert_eq!(lookup(2, 0b01), Some(Entry::Double(0.0)));
    }
}
