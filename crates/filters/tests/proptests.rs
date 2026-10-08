//! Round-trip property tests (encode → decode) for every encodable filter and predictor,
//! and fuzz-style tests feeding arbitrary bytes and parameters to every decoder.

use pdfcraft_filters::{Filter, FilterError, Params, decode, decode_tolerant, encode};
use proptest::prelude::*;

const MAX: usize = 1 << 24;

fn encodable() -> impl Strategy<Value = Filter> {
    prop_oneof![Just(Filter::Flate), Just(Filter::Lzw), Just(Filter::AsciiHex), Just(Filter::Ascii85), Just(Filter::RunLength),]
}

fn any_filter() -> impl Strategy<Value = Filter> {
    prop_oneof![
        encodable(),
        Just(Filter::Dct),
        Just(Filter::Jpx),
        Just(Filter::Jbig2),
        Just(Filter::CcittFax),
        Just(Filter::Crypt),
        Just(Filter::Unknown("Foo".into())),
    ]
}

fn valid_params() -> impl Strategy<Value = Params> {
    (prop::sample::select(vec![1i64, 2, 10, 11, 12, 13, 14, 15]), 1i64..=5, prop::sample::select(vec![1i64, 2, 4, 8, 16]), 1i64..=40, 0i64..=1)
        .prop_map(|(predictor, colors, bits_per_component, columns, early_change)| Params {
            predictor,
            colors,
            bits_per_component,
            columns,
            early_change,
        })
}

/// Anything, including nonsense values.
fn wild_params() -> impl Strategy<Value = Params> {
    let wild = prop_oneof![Just(i64::MIN), Just(i64::MAX), Just(-1i64), Just(0i64), 0i64..=40, any::<i64>()];
    (
        prop_oneof![valid_params().prop_map(|p| p.predictor), wild.clone()],
        prop_oneof![1i64..=5, wild.clone()],
        prop_oneof![Just(8i64), Just(16), Just(1), wild.clone()],
        prop_oneof![1i64..=40, wild.clone()],
        wild,
    )
        .prop_map(|(predictor, colors, bits_per_component, columns, early_change)| Params {
            predictor,
            colors,
            bits_per_component,
            columns,
            early_change,
        })
}

/// Low-entropy data (so LZW/Flate/RunLength exercise their repeat paths) mixed with noise.
fn data() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![prop::collection::vec(any::<u8>(), 0..2000), prop::collection::vec(prop::sample::select(vec![0u8, 1, 7, 255]), 0..6000),]
}

proptest! {
    #[test]
    fn round_trip(filter in encodable(), params in valid_params(), data in data()) {
        let enc = encode(&filter, &params, &data).unwrap();
        let chain = [(filter, params)];
        prop_assert_eq!(&decode(&chain, &enc, MAX).unwrap(), &data);
        let (out, partial) = decode_tolerant(&chain, &enc, MAX).unwrap();
        prop_assert!(!partial);
        prop_assert_eq!(&out, &data);
    }

    #[test]
    fn round_trip_chain(a in encodable(), b in encodable(), params in valid_params(), data in data()) {
        let inner = encode(&b, &params, &data).unwrap();
        let outer = encode(&a, &Params::default(), &inner).unwrap();
        let chain = [(a, Params::default()), (b, params)];
        prop_assert_eq!(decode(&chain, &outer, MAX).unwrap(), data);
    }

    #[test]
    fn lzw_long_round_trip(early in 0i64..=1, data in prop::collection::vec(0u8..4, 20_000..60_000)) {
        let p = Params { early_change: early, ..Params::default() };
        let enc = encode(&Filter::Lzw, &p, &data).unwrap();
        prop_assert_eq!(decode(&[(Filter::Lzw, p)], &enc, MAX).unwrap(), data);
    }

    #[test]
    fn output_limit_is_respected(filter in encodable(), params in valid_params(), data in data(), max in 0usize..3000) {
        let enc = encode(&filter, &params, &data).unwrap();
        // PNG predictors add a tag byte per row to the intermediate (pre-predictor) buffer.
        let png = matches!(filter, Filter::Flate | Filter::Lzw) && params.predictor >= 10;
        let row = (params.colors * params.bits_per_component * params.columns + 7) as usize / 8;
        let intermediate = if png { data.len() + data.len().div_ceil(row) } else { data.len() };
        match decode(&[(filter, params)], &enc, max) {
            Ok(out) => prop_assert!(out.len() <= max && out == data),
            Err(FilterError::LimitExceeded(m)) => prop_assert!(m == max && intermediate > max),
            Err(e) => prop_assert!(false, "unexpected {e}"),
        }
    }

    #[test]
    fn decoders_never_panic(
        chain in prop::collection::vec((any_filter(), wild_params()), 0..4),
        data in prop::collection::vec(any::<u8>(), 0..3000),
        max in prop_oneof![Just(0usize), 1usize..100_000],
    ) {
        if let Ok(out) = decode(&chain, &data, max) {
            prop_assert!(out.len() <= max || chain.is_empty());
        }
        if let Ok((out, _)) = decode_tolerant(&chain, &data, max) {
            prop_assert!(out.len() <= max || chain.is_empty());
        }
    }

    #[test]
    fn single_decoders_never_panic(filter in encodable(), params in wild_params(), data in prop::collection::vec(any::<u8>(), 0..3000)) {
        let chain = [(filter, params)];
        let _ = decode(&chain, &data, 1 << 20);
        let _ = decode_tolerant(&chain, &data, 1 << 20);
    }

    #[test]
    fn encoders_never_panic(filter in any_filter(), params in wild_params(), data in prop::collection::vec(any::<u8>(), 0..2000)) {
        if let Ok(enc) = encode(&filter, &params, &data) {
            prop_assert_eq!(decode(&[(filter, params)], &enc, MAX).unwrap(), data);
        }
    }

    #[test]
    fn mutated_streams_never_panic(filter in encodable(), params in valid_params(), data in data(), flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8)) {
        let mut enc = encode(&filter, &params, &data).unwrap();
        for (i, x) in flips {
            if !enc.is_empty() {
                let k = i.index(enc.len());
                enc[k] ^= x;
            }
        }
        let chain = [(filter, params)];
        let _ = decode(&chain, &enc, 1 << 20);
        let _ = decode_tolerant(&chain, &enc, 1 << 20);
    }
}

#[test]
fn flate_bomb_in_chain() {
    let bomb = pdfcraft_filters::encode_flate(&vec![0u8; 32 << 20]);
    let a85 = encode(&Filter::Ascii85, &Params::default(), &bomb).unwrap();
    let chain = [(Filter::Ascii85, Params::default()), (Filter::Flate, Params::default())];
    assert!(matches!(decode(&chain, &a85, 1 << 20), Err(FilterError::LimitExceeded(_))));
    assert!(matches!(decode_tolerant(&chain, &a85, 1 << 20), Err(FilterError::LimitExceeded(_))));
}
