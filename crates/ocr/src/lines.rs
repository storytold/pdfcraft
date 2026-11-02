//! Line geometry before recognition: horizontal CJK line fragments, joined so crop margins
//! cannot recognize the same overlapping fragment twice, and skewed lines measured along
//! their own direction so they can be read straightened.

/// A text line along its own reading direction. `across` is the unit reading direction in
/// image pixels (x right, y down); `u` and `v` are the line's extents projected onto
/// `across` and onto its downward perpendicular.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LineBox {
    pub across: [f32; 2],
    pub u: [f32; 2],
    pub v: [f32; 2],
}

impl LineBox {
    /// Fit a line to its words: `directions` are each word's reading direction with a weight
    /// (its length along it), `points` all of the words' corners. None for empty or
    /// degenerate geometry.
    pub fn fit(directions: &[([f32; 2], f32)], points: &[[f32; 2]]) -> Option<LineBox> {
        let mut sum = [0.0f32; 2];
        for &([x, y], weight) in directions {
            // A word box has no reading direction of its own: take the one that reads rightwards.
            let sign = if x < 0.0 { -1.0 } else { 1.0 };
            sum = [sum[0] + sign * x * weight, sum[1] + sign * y * weight];
        }
        let length = (sum[0] * sum[0] + sum[1] * sum[1]).sqrt();
        if !length.is_finite() || length <= f32::EPSILON || points.is_empty() {
            return None;
        }
        let across = [sum[0] / length, sum[1] / length];
        let mut line = LineBox { across, u: [f32::INFINITY, f32::NEG_INFINITY], v: [f32::INFINITY, f32::NEG_INFINITY] };
        let down = line.down();
        for p in points {
            let (u, v) = (p[0] * across[0] + p[1] * across[1], p[0] * down[0] + p[1] * down[1]);
            line.u = [line.u[0].min(u), line.u[1].max(u)];
            line.v = [line.v[0].min(v), line.v[1].max(v)];
        }
        (line.u.iter().chain(&line.v).all(|v| v.is_finite()) && line.u[1] > line.u[0] && line.v[1] > line.v[0]).then_some(line)
    }

    fn down(&self) -> [f32; 2] {
        [-self.across[1], self.across[0]]
    }

    /// Clockwise skew of the reading direction from horizontal, in degrees.
    pub fn angle(&self) -> f32 {
        self.across[1].atan2(self.across[0]).to_degrees()
    }

    pub fn height(&self) -> f32 {
        self.v[1] - self.v[0]
    }

    pub fn length(&self) -> f32 {
        self.u[1] - self.u[0]
    }

    /// The image pixel at line coordinates `u`, `v`.
    pub fn point(&self, u: f32, v: f32) -> [f32; 2] {
        let down = self.down();
        [u * self.across[0] + v * down[0], u * self.across[1] + v * down[1]]
    }

    /// The axis-aligned bounds of the stretch `u0..u1` of the line, at the line's own height.
    pub fn bounds(&self, u0: f32, u1: f32) -> [f32; 4] {
        let corners = [self.point(u0, self.v[0]), self.point(u1, self.v[0]), self.point(u1, self.v[1]), self.point(u0, self.v[1])];
        corners.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |a, p| {
            [a[0].min(p[0]), a[1].min(p[1]), a[2].max(p[0]), a[3].max(p[1])]
        })
    }
}

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
    fn skewed_line_is_measured_along_its_own_direction() {
        // A 200 × 20 line turned 5° clockwise about (100, 100), its two words reported with
        // opposite directions as a min-area rectangle may.
        let (s, c) = 5f32.to_radians().sin_cos();
        let at = |u: f32, v: f32| [100.0 + u * c - v * s, 100.0 + u * s + v * c];
        let corners = [at(-100.0, -10.0), at(100.0, -10.0), at(100.0, 10.0), at(-100.0, 10.0)];
        let line = LineBox::fit(&[([c, s], 120.0), ([-c, -s], 80.0)], &corners).unwrap();
        assert!((line.angle() - 5.0).abs() < 1e-3, "{line:?}");
        assert!((line.length() - 200.0).abs() < 1e-3 && (line.height() - 20.0).abs() < 1e-3, "{line:?}");
        let [x, y] = line.point(line.u[0], line.v[0]);
        assert!((x - corners[0][0]).abs() < 1e-3 && (y - corners[0][1]).abs() < 1e-3);
        // A word in the left half of the line is bounded where it lies, not by the whole line.
        let half = line.bounds(line.u[0], line.u[0] + 100.0);
        assert!(half[0] < corners[0][0] && half[2] < 110.0 && half[3] - half[1] < 30.0, "{half:?}");
    }

    #[test]
    fn line_fit_rejects_degenerate_geometry() {
        assert_eq!(LineBox::fit(&[], &[[0.0, 0.0], [10.0, 10.0]]), None);
        assert_eq!(LineBox::fit(&[([1.0, 0.0], 0.0)], &[[0.0, 0.0], [10.0, 10.0]]), None);
        assert_eq!(LineBox::fit(&[([1.0, 0.0], 10.0)], &[]), None);
        assert_eq!(LineBox::fit(&[([1.0, 0.0], 10.0)], &[[5.0, 5.0]]), None);
        assert_eq!(LineBox::fit(&[([f32::NAN, 0.0], 10.0)], &[[0.0, 0.0], [10.0, 10.0]]), None);
    }

    #[test]
    fn drops_invalid_geometry_and_handles_empty_input() {
        assert!(join_fragments(vec![]).is_empty());
        assert!(join_fragments(vec![[f32::NAN, 0.0, 10.0, 10.0], [0.0, 0.0, f32::INFINITY, 10.0], [0.0; 4], [20.0, 0.0, 10.0, 20.0]]).is_empty());
    }
}
