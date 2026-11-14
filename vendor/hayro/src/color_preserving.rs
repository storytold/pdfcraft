//! Hue-preserving night-reading curve, independently implemented from Sebyoshi's public
//! mathematical reference: https://github.com/storytold/pdfcraft/pull/3281#issuecomment-6725501625.

const CHROMA_NEUTRAL: f64 = 0.12;
const CHROMA_COLORED: f64 = 0.25;
const BASE_EXPONENT: f64 = 0.96;
const DARK_HUE_WEIGHT: f64 = 0.25;
const HUE_BRIGHTNESS_TARGET: f64 = 0.6;
const RED_WEIGHT: f64 = 0.2126;
const GREEN_WEIGHT: f64 = 0.7152;
const BLUE_WEIGHT: f64 = 0.0722;
const CHANNEL_MAX: f64 = u8::MAX as f64;
const HUE_SECTORS: f64 = 6.0;

/// Maps straight, gamma-encoded sRGB paint; alpha and the requested paper/ink endpoints remain.
pub(crate) fn map([r, g, b, alpha]: [u8; 4], text: [u8; 3], paper: [u8; 3]) -> [u8; 4] {
    let [r, g, b] = [r, g, b].map(|channel| f64::from(channel) / CHANNEL_MAX);
    let maximum = r.max(g).max(b);
    let minimum = r.min(g).min(b);
    let chroma = maximum - minimum;
    let lightness = (maximum + minimum) / 2.0;
    let (hue, saturation) = if chroma == 0.0 {
        (0.0, 0.0)
    } else {
        let sector = if maximum == r {
            ((g - b) / chroma).rem_euclid(HUE_SECTORS)
        } else if maximum == g {
            (b - r) / chroma + 2.0
        } else {
            (r - g) / chroma + 4.0
        };
        // Nonzero chroma excludes black/white, so the saturation denominator is positive.
        (sector / HUE_SECTORS, chroma / (1.0 - (2.0 * lightness - 1.0).abs()))
    };

    // Near-white, low-chroma paper should become neutral rather than acquire a colour cast.
    let t = ((chroma - CHROMA_NEUTRAL) / (CHROMA_COLORED - CHROMA_NEUTRAL)).clamp(0.0, 1.0);
    let saturation = saturation * t * t * (3.0 - 2.0 * t);
    let [rh, gh, bh] = rgb(hue, 1.0, 0.5);
    // These weights apply to gamma RGB here; this is not WCAG's linear-light luminance.
    let brightness = RED_WEIGHT * rh + GREEN_WEIGHT * gh + BLUE_WEIGHT * bh;
    let hue_weight = DARK_HUE_WEIGHT + (1.0 - DARK_HUE_WEIGHT) * (brightness / HUE_BRIGHTNESS_TARGET).min(1.0);
    let exponent = BASE_EXPONENT * (1.0 - saturation * (1.0 - hue_weight));
    let [r, g, b] = rgb(hue, saturation, (1.0 - lightness).powf(exponent));

    let channel =
        |value: f64, paper: u8, ink: u8| (f64::from(paper) + (f64::from(ink) - f64::from(paper)) * value).round().clamp(0.0, CHANNEL_MAX) as u8;
    let [pr, pg, pb] = paper;
    let [tr, tg, tb] = text;
    [channel(r, pr, tr), channel(g, pg, tg), channel(b, pb, tb), alpha]
}

fn rgb(hue: f64, saturation: f64, lightness: f64) -> [f64; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue * HUE_SECTORS;
    let secondary = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let offset = lightness - chroma / 2.0;
    // Hue comes only from the bounded RGB conversion above, so the sector is finite and in 0..6.
    let components = match sector as u8 {
        0 => [chroma, secondary, 0.0],
        1 => [secondary, chroma, 0.0],
        2 => [0.0, chroma, secondary],
        3 => [0.0, secondary, chroma],
        4 => [secondary, 0.0, chroma],
        _ => [chroma, 0.0, secondary],
    };
    components.map(|component| component + offset)
}
