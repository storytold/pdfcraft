//! Arbitrary bytes through every reader: an error or a value, never a panic.

use pdfcraft_prc::{BitReader, PrcFile, Schema};
use proptest::prelude::*;

proptest! {
    #[test]
    fn any_bytes_parse_or_fail_cleanly(data in proptest::collection::vec(any::<u8>(), 0..512), magic in any::<bool>()) {
        let mut data = data;
        if magic && data.len() >= 3 {
            data[..3].copy_from_slice(b"PRC");
        }
        let _ = PrcFile::parse_with_limit(&data, 1 << 20);
    }

    #[test]
    fn any_bits_read_or_fail_cleanly(data in proptest::collection::vec(any::<u8>(), 0..64), n in 0u32..40) {
        let mut r = BitReader::new(&data);
        let _ = r.unsigned_integer();
        let _ = r.integer();
        let _ = r.integer_vbn(n);
        let _ = r.double_vbn(0.5, n);
        let _ = r.nbits_then_unsigned();
        let _ = r.string();
        let _ = r.float_as_bytes();
        let _ = r.double();
        let _ = r.skip_bits(usize::try_from(n).unwrap_or(0));
        prop_assert!(r.position() <= data.len() * 8);
    }

    #[test]
    fn any_schema_runs_or_fails_cleanly(data in proptest::collection::vec(any::<u8>(), 0..256), entity in 0u32..4) {
        let mut r = BitReader::new(&data);
        if let Ok(schema) = Schema::read(&mut r) {
            let types: Vec<u32> = schema.entity_types().chain([entity]).collect();
            for t in types {
                let _ = schema.skip_added_fields(t, &mut r.clone());
            }
        }
    }
}
