//! A procedurally generated raster (so the showcase needs no third-party art).

use anyhow::Result;

/// Centre and width of the rendered region of the complex plane ("Seahorse Valley").
const CX: f64 = -0.743_643_887;
const CY: f64 = 0.131_825_904;
const SPAN: f64 = 0.018;
const MAX_ITER: u32 = 700;

/// Gradient stops for the smooth escape-time colouring, matching the showcase palette.
const STOPS: &[(f64, [f64; 3])] = &[
    (0.00, [15.0, 24.0, 48.0]),
    (0.18, [42.0, 111.0, 219.0]),
    (0.38, [26.0, 158.0, 143.0]),
    (0.58, [242.0, 193.0, 78.0]),
    (0.78, [228.0, 87.0, 46.0]),
    (1.00, [251.0, 248.0, 242.0]),
];

/// Render the Mandelbrot set (2×2 supersampled) and encode it as an RGB PNG.
pub fn mandelbrot_png(size: u32) -> Result<Vec<u8>> {
    let mut rgb = Vec::with_capacity((size * size * 3) as usize);
    let step = SPAN / f64::from(size);
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f64; 3];
            for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let re = CX + (f64::from(px) + sx - f64::from(size) / 2.0) * step;
                let im = CY - (f64::from(py) + sy - f64::from(size) / 2.0) * step;
                let c = colour(escape(re, im));
                for (a, v) in acc.iter_mut().zip(c) {
                    *a += v / 4.0;
                }
            }
            rgb.extend(acc.map(|v| v.round().clamp(0.0, 255.0) as u8));
        }
    }

    let img = craft_image_codecs::Image::from_u8(size, size, craft_image_codecs::ChannelLayout::Rgb, rgb)?;
    let png_bytes = craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default())?;
    Ok(png_bytes)
}

/// Smooth (continuous) iteration count, or `None` inside the set.
fn escape(re: f64, im: f64) -> Option<f64> {
    let (mut x, mut y) = (0.0f64, 0.0f64);
    for n in 0..MAX_ITER {
        let (x2, y2) = (x * x, y * y);
        if x2 + y2 > 256.0 {
            let log_zn = (x2 + y2).ln() / 2.0;
            let nu = (log_zn / std::f64::consts::LN_2).ln() / std::f64::consts::LN_2;
            return Some(f64::from(n) + 1.0 - nu);
        }
        y = 2.0 * x * y + im;
        x = x2 - y2 + re;
    }
    None
}

fn colour(mu: Option<f64>) -> [f64; 3] {
    let Some(mu) = mu else {
        return [11.0, 18.0, 38.0];
    };
    // Cycle the palette on a log scale so detail shows at every depth.
    let t = (mu.max(1.0).ln() * 0.9).fract();
    let t = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 };
    for pair in STOPS.windows(2) {
        let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
        if t <= t1 {
            let f = (t - t0) / (t1 - t0);
            return [0, 1, 2].map(|i| c0[i] + (c1[i] - c0[i]) * f);
        }
    }
    STOPS[STOPS.len() - 1].1
}

/// Standard base64 (RFC 4648) with padding, for `data:` URIs.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_rfc4648_vectors() {
        let cases = [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")];
        for (input, expected) in cases {
            assert_eq!(super::base64(input.as_bytes()), expected);
        }
    }
}
