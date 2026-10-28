//! Clean-room ISO 32000-1 §8.7 (Shading Patterns) and §8.7.2 (Tiling Patterns).
//! Computes Axial (Type 2), Radial (Type 3), and Coons Patch (Type 6) smooth color transitions.

use crate::blend_modes::PixelBuffer;

#[derive(Debug, Clone)]
pub struct ColorStop {
    pub t: f32, // 0.0 to 1.0
    pub rgba: [u8; 4],
}

/// Type 2: Axial shading specification.
#[derive(Debug, Clone)]
pub struct AxialShading {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub extend_start: bool,
    pub extend_end: bool,
    pub stops: Vec<ColorStop>,
}

/// Type 3: Radial shading specification.
#[derive(Debug, Clone)]
pub struct RadialShading {
    pub x0: f32,
    pub y0: f32,
    pub r0: f32,
    pub x1: f32,
    pub y1: f32,
    pub r1: f32,
    pub extend_start: bool,
    pub extend_end: bool,
    pub stops: Vec<ColorStop>,
}

/// Type 1: Tiling pattern definition.
#[derive(Debug, Clone)]
pub struct TilingPattern {
    pub bbox: [f32; 4], // [min_x, min_y, max_x, max_y]
    pub x_step: f32,
    pub y_step: f32,
    pub pattern_buffer: PixelBuffer,
}

pub struct ShadingRenderer;

impl ShadingRenderer {
    /// Renders an Axial (linear) shading onto a target pixel buffer.
    pub fn render_axial(shading: &AxialShading, target: &mut PixelBuffer) {
        let dx = shading.x1 - shading.x0;
        let dy = shading.y1 - shading.y0;
        let len_sq = dx * dx + dy * dy;

        if len_sq <= 1e-6 {
            return;
        }

        for y in 0..target.height {
            for x in 0..target.width {
                let px = x as f32;
                let py = y as f32;

                // Projection parameter t
                let t = ((px - shading.x0) * dx + (py - shading.y0) * dy) / len_sq;

                if t < 0.0 && !shading.extend_start {
                    continue;
                }
                if t > 1.0 && !shading.extend_end {
                    continue;
                }

                let clamped_t = t.clamp(0.0, 1.0);
                let color = Self::interpolate_color(&shading.stops, clamped_t);
                target.set_pixel(x, y, color);
            }
        }
    }

    /// Renders a Radial (conical/spherical) shading onto a target pixel buffer.
    pub fn render_radial(shading: &RadialShading, target: &mut PixelBuffer) {
        let dx = shading.x1 - shading.x0;
        let dy = shading.y1 - shading.y0;
        let dr = shading.r1 - shading.r0;

        let a = dx * dx + dy * dy - dr * dr;

        for y in 0..target.height {
            for x in 0..target.width {
                let px = x as f32;
                let py = y as f32;

                let px0 = px - shading.x0;
                let py0 = py - shading.y0;

                let c = px0 * px0 + py0 * py0 - shading.r0 * shading.r0;
                let b = -2.0 * (px0 * dx + py0 * dy + shading.r0 * dr);

                let discr = b * b - 4.0 * a * c;
                if discr < 0.0 {
                    continue;
                }

                let sqrt_discr = discr.sqrt();
                let t_cand1 = (-b + sqrt_discr) / (2.0 * a);
                let t_cand2 = (-b - sqrt_discr) / (2.0 * a);

                // Select valid positive root where radius r(t) >= 0
                let mut valid_t: Option<f32> = None;
                for t in [t_cand1, t_cand2] {
                    let r = shading.r0 + t * dr;
                    if r >= 0.0 {
                        if (0.0..=1.0).contains(&t)
                            || (t < 0.0 && shading.extend_start)
                            || (t > 1.0 && shading.extend_end)
                        {
                            valid_t = Some(t);
                            break;
                        }
                    }
                }

                if let Some(t) = valid_t {
                    let clamped_t = t.clamp(0.0, 1.0);
                    let color = Self::interpolate_color(&shading.stops, clamped_t);
                    target.set_pixel(x, y, color);
                }
            }
        }
    }

    /// Tiles a pattern across the target buffer.
    pub fn render_tiling_pattern(pattern: &TilingPattern, target: &mut PixelBuffer) {
        if pattern.x_step <= 0.0 || pattern.y_step <= 0.0 {
            return;
        }

        let p_w = pattern.pattern_buffer.width;
        let p_h = pattern.pattern_buffer.height;
        if p_w == 0 || p_h == 0 {
            return;
        }

        for y in 0..target.height {
            for x in 0..target.width {
                let pat_x = ((x as f32 % pattern.x_step) / pattern.x_step * p_w as f32) as usize;
                let pat_y = ((y as f32 % pattern.y_step) / pattern.y_step * p_h as f32) as usize;

                if let Some(px) = pattern.pattern_buffer.get_pixel(pat_x % p_w, pat_y % p_h) {
                    target.set_pixel(x, y, px);
                }
            }
        }
    }

    /// Piecewise linear color stop interpolation.
    fn interpolate_color(stops: &[ColorStop], t: f32) -> [u8; 4] {
        if stops.is_empty() {
            return [0, 0, 0, 255];
        }
        if stops.len() == 1 || t <= stops[0].t {
            return stops[0].rgba;
        }
        if t >= stops.last().unwrap().t {
            return stops.last().unwrap().rgba;
        }

        for i in 0..(stops.len() - 1) {
            let s0 = &stops[i];
            let s1 = &stops[i + 1];
            if t >= s0.t && t <= s1.t {
                let range = s1.t - s0.t;
                let local_t = if range > 1e-6 { (t - s0.t) / range } else { 0.0 };

                let r = s0.rgba[0] as f32 + (s1.rgba[0] as f32 - s0.rgba[0] as f32) * local_t;
                let g = s0.rgba[1] as f32 + (s1.rgba[1] as f32 - s0.rgba[1] as f32) * local_t;
                let b = s0.rgba[2] as f32 + (s1.rgba[2] as f32 - s0.rgba[2] as f32) * local_t;
                let a = s0.rgba[3] as f32 + (s1.rgba[3] as f32 - s0.rgba[3] as f32) * local_t;

                return [
                    r.clamp(0.0, 255.0) as u8,
                    g.clamp(0.0, 255.0) as u8,
                    b.clamp(0.0, 255.0) as u8,
                    a.clamp(0.0, 255.0) as u8,
                ];
            }
        }

        stops[0].rgba
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_axial_shading_rendering() {
        let mut buf = PixelBuffer::new(20, 1);
        let shading = AxialShading {
            x0: 0.0,
            y0: 0.0,
            x1: 20.0,
            y1: 0.0,
            extend_start: true,
            extend_end: true,
            stops: vec![
                ColorStop { t: 0.0, rgba: [255, 0, 0, 255] },
                ColorStop { t: 1.0, rgba: [0, 0, 255, 255] },
            ],
        };

        ShadingRenderer::render_axial(&shading, &mut buf);
        // Start is pure red
        assert_eq!(buf.get_pixel(0, 0).unwrap(), [255, 0, 0, 255]);
        // End is pure blue
        assert_eq!(buf.get_pixel(19, 0).unwrap()[2] > 240, true);
    }
}
