//! The quadrilaterals of a `/QuadPoints` array (ISO 32000-2 §12.5.6.10), read by one rule.
//!
//! Redaction apply, the comment summaries and outlines, the inspector and generated appearances
//! all read `/QuadPoints`; they go through [`usable_quads`] so the area a user sees marked is the
//! area apply removes.

/// The usable quadrilaterals of a `/QuadPoints` array, given its entries in order (`None` for an
/// entry that is not a number).
///
/// A quadrilateral is usable only when all eight of its entries are present, numbers, and within
/// the range of a PDF real (finite as an `f32`, Annex C), so the readers that keep `f32` see the
/// same set as those that keep `f64`. An unusable one is skipped whole: the ones after it keep
/// their coordinates, and it never becomes a folded box or zeros. A trailing partial group is
/// dropped.
pub fn usable_quads<I: IntoIterator<Item = Option<f64>>>(entries: I) -> Vec<[f64; 8]> {
    let entries: Vec<Option<f64>> = entries.into_iter().collect();
    entries
        .as_chunks::<8>()
        .0
        .iter()
        .filter_map(|q| {
            let mut quad = [0.0; 8];
            for (slot, v) in quad.iter_mut().zip(q) {
                // NaN and infinities stay non-finite as `f32`; so does a value past its range.
                *slot = v.filter(|v| (*v as f32).is_finite())?;
            }
            Some(quad)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::usable_quads;

    const A: [f64; 8] = [10.0, 20.0, 50.0, 20.0, 10.0, 40.0, 50.0, 40.0];
    const B: [f64; 8] = [60.0, 70.0, 90.0, 70.0, 60.0, 80.0, 90.0, 80.0];

    fn some(v: &[f64]) -> Vec<Option<f64>> {
        v.iter().copied().map(Some).collect()
    }

    #[test]
    fn finite_quads_are_all_usable() {
        assert_eq!(usable_quads(some(&[A, B].concat())), vec![A, B]);
        assert!(usable_quads(Vec::new()).is_empty());
    }

    #[test]
    fn an_all_nan_quad_is_unusable() {
        assert!(usable_quads(some(&[f64::NAN; 8])).is_empty());
    }

    #[test]
    fn one_non_finite_entry_makes_its_quad_unusable() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300] {
            let mut q = A;
            q[3] = bad;
            assert_eq!(usable_quads(some(&[q, B].concat())), vec![B], "{bad}");
        }
    }

    #[test]
    fn a_non_number_drops_its_quad_without_shifting_the_next() {
        let mut entries = some(&[A, B].concat());
        entries[2] = None;
        assert_eq!(usable_quads(entries), vec![B]);
    }

    #[test]
    fn a_trailing_partial_group_is_dropped() {
        assert_eq!(usable_quads(some(&[&A[..], &B[..4]].concat())), vec![A]);
    }
}
