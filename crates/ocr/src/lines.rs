//! Horizontal CJK line fragments, joined before recognition so crop margins cannot
//! recognize the same overlapping fragment twice.

fn order(a: &[f32; 4], b: &[f32; 4]) -> std::cmp::Ordering {
    a[1].total_cmp(&b[1]).then_with(|| a[0].total_cmp(&b[0]))
}

fn neighbours(a: [f32; 4], b: [f32; 4]) -> bool {
    let ah = a[3] - a[1];
    let bh = b[3] - b[1];
    let overlap = a[3].min(b[3]) - a[1].max(b[1]);
    let gap = a[0].max(b[0]) - a[2].min(b[2]);
    // Small centred punctuation may be shorter than the surrounding Han glyphs.
    // Require a shared vertical band and about one em of horizontal whitespace (ink boxes can be shorter than the em).
    overlap >= ah.min(bh) * 0.6 && gap <= ah.max(bh) * 1.25
}

pub(crate) fn join_fragments(rects: Vec<[f32; 4]>) -> Vec<[f32; 4]> {
    let mut rects: Vec<_> = rects
        .into_iter()
        .enumerate()
        .filter_map(|(index, r)| {
            (r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1] && (r[2] - r[0]).is_finite() && (r[3] - r[1]).is_finite())
                .then_some((r, index))
        })
        .collect();
    rects.sort_by(|a, b| order(&a.0, &b.0));
    let mut joined: Vec<([f32; 4], usize)> = Vec::with_capacity(rects.len());
    for (mut rect, mut first) in rects {
        // Bound the local search even for adversarial pages with thousands of fragments.
        // Removal shifts at most 63 entries, not the entire page. Never compare text strings:
        // repeated words at separate positions must survive.
        for _ in 0..64 {
            let start = joined.len().saturating_sub(64);
            let Some(tail) = joined.get(start..) else { break };
            let Some(offset) = tail.iter().rposition(|(other, _)| neighbours(rect, *other)) else { break };
            let Some(index) = start.checked_add(offset) else { break };
            let Some((other, original)) = joined.get(index).copied() else { break };
            joined.remove(index);
            first = first.min(original);
            rect = [rect[0].min(other[0]), rect[1].min(other[1]), rect[2].max(other[2]), rect[3].max(other[3])];
        }
        joined.push((rect, first));
    }
    // Preserve the detector's order between distinct lines/columns. Sorting the whole page
    // into rows would incorrectly interleave two columns; only joined fragments change.
    joined.sort_by_key(|(_, first)| *first);
    joined.into_iter().map(|(rect, _)| rect).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_centered_punctuation_gaps_and_overlapping_fragments_in_reading_order() {
        let input = vec![
            [10.0, 10.0, 75.0, 30.0],
            [110.0, 40.0, 190.0, 60.0],
            [90.0, 11.0, 160.0, 31.0],
            [150.0, 10.0, 220.0, 30.0],
            [10.0, 40.0, 95.0, 60.0],
            [82.0, 18.0, 87.0, 23.0],
        ];
        assert_eq!(join_fragments(input), vec![[10.0, 10.0, 220.0, 31.0], [10.0, 40.0, 190.0, 60.0]]);
    }

    #[test]
    fn centred_comma_gap_in_a_rendered_han_line_is_joined() {
        // Detector output from our own BIZ UDPGothic fixture: the comma's em is
        // omitted from both ink boxes, and rounding adds another three pixels.
        let left = [47.0, 39.0, 284.0, 79.0];
        let right = [326.96, 37.4, 589.16, 79.05];
        assert_eq!(join_fragments(vec![right, left]), vec![[47.0, 37.4, 589.16, 79.05]]);
    }

    #[test]
    fn retains_separate_rows_columns_and_repeated_text_at_distinct_positions() {
        let boxes = vec![[10.0, 10.0, 100.0, 30.0], [150.0, 10.0, 240.0, 30.0], [10.0, 35.0, 100.0, 55.0]];
        assert_eq!(join_fragments(boxes.clone()), boxes);
    }

    #[test]
    fn preserves_column_major_order_between_unjoined_lines() {
        let boxes = vec![[10.0, 10.0, 100.0, 30.0], [10.0, 40.0, 100.0, 60.0], [200.0, 10.0, 290.0, 30.0], [200.0, 40.0, 290.0, 60.0]];
        assert_eq!(join_fragments(boxes.clone()), boxes);
    }

    #[test]
    fn drops_invalid_geometry_and_handles_empty_input() {
        assert!(join_fragments(vec![]).is_empty());
        assert!(join_fragments(vec![[f32::NAN, 0.0, 10.0, 10.0], [0.0, 0.0, f32::INFINITY, 10.0], [0.0; 4], [20.0, 0.0, 10.0, 20.0]]).is_empty());
    }
}
