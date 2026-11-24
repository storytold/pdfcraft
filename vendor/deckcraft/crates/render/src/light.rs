//! Light rigs (`a:lightRig rig= dir=`) and materials (`a:sp3d prstMaterial=`) for 3-D shapes:
//! how bright a surface is for its normal.
//!
//! The three-point rig with the matte material is fitted to PowerPoint's exports (see
//! [`crate::bevel`]): `c · (0.453 + 0.731 · max(0, n · L)) + 14.7` with the key light from the upper
//! left. The other rigs and materials are modelled on it, not measured: each rig is a set of
//! directional lights with an ambient level and a colour tint, each material scales the diffuse
//! and ambient terms and adds a specular highlight.

use deckcraft_color::Rgba;

/// A light rig and material: what lights a 3-D surface.
#[derive(Clone, Debug, PartialEq)]
pub struct Lighting {
    /// Unit directions from the surface towards each light (x right, y down, z to the viewer)
    /// and their strengths.
    pub lights: Vec<([f64; 3], f64)>,
    pub ambient: f64,
    pub diffuse: f64,
    /// Added to every channel (the measured lift of the three-point rig).
    pub lift: f64,
    /// Colour of the light, per channel.
    pub tint: [f64; 3],
    pub specular: f64,
    pub shininess: f64,
    /// `flat`: no shading at all.
    pub flat: bool,
}

fn dir(az: f64, el: f64) -> [f64; 3] {
    let (az, el) = (az.to_radians(), el.to_radians());
    [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()]
}

fn value_after<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    let s = raw.find(key)? + key.len();
    let rest = raw.get(s..)?;
    rest.get(..rest.find('"')?)
}

/// The rig's lights as (azimuth, elevation, strength), its ambient level and tint.
fn rig(name: &str) -> (Vec<(f64, f64, f64)>, f64, [f64; 3]) {
    const WHITE: [f64; 3] = [1.0, 1.0, 1.0];
    match name {
        "balanced" => (vec![(240.0, 40.0, 0.75), (300.0, 40.0, 0.35), (90.0, 30.0, 0.25)], 0.47, WHITE),
        "soft" => (vec![(270.0, 60.0, 0.6)], 0.58, WHITE),
        "harsh" => (vec![(225.0, 30.0, 1.15)], 0.3, WHITE),
        "flood" => (vec![(270.0, 70.0, 0.8), (240.0, 40.0, 0.35)], 0.5, WHITE),
        "contrasting" => (vec![(210.0, 30.0, 1.05), (30.0, 20.0, 0.3)], 0.33, WHITE),
        "morning" => (vec![(240.0, 35.0, 0.9)], 0.45, [1.0, 0.95, 0.85]),
        "sunrise" => (vec![(200.0, 20.0, 0.95)], 0.42, [1.0, 0.88, 0.75]),
        "sunset" => (vec![(340.0, 20.0, 0.95)], 0.42, [1.0, 0.82, 0.7]),
        "chilly" => (vec![(240.0, 45.0, 0.85)], 0.45, [0.88, 0.94, 1.0]),
        "freezing" => (vec![(240.0, 45.0, 0.85)], 0.45, [0.8, 0.9, 1.0]),
        "twoPt" => (vec![(225.0, 45.0, 0.8), (315.0, 45.0, 0.45)], 0.42, WHITE),
        "glow" => (vec![(270.0, 85.0, 0.55)], 0.68, WHITE),
        "brightRoom" => (vec![(270.0, 60.0, 0.45)], 0.75, WHITE),
        // threePt and the legacy rigs.
        _ => (vec![(240.0, 45.0, 1.0)], 0.453, WHITE),
    }
}

/// The rig direction (`dir`) turns the lights about the view axis.
fn dir_turn(d: &str) -> f64 {
    match d {
        "tr" => 45.0,
        "r" => 90.0,
        "br" => 135.0,
        "b" => 180.0,
        "bl" => 225.0,
        "l" => 270.0,
        "tl" => 315.0,
        _ => 0.0,
    }
}

impl Lighting {
    /// The lighting of 3-D XML kept from the file; the three-point rig and warm matte material
    /// when unset.
    pub fn from_raw(raw: &str) -> Lighting {
        let rig_name = value_after(raw, "rig=\"").unwrap_or("threePt");
        let turn = dir_turn(value_after(raw, " dir=\"").unwrap_or("t"));
        let material = value_after(raw, "prstMaterial=\"").unwrap_or("warmMatte");
        let (lights, ambient, tint) = rig(rig_name);
        let (dk, ak, specular, shininess) = match material {
            "plastic" => (1.0, 1.0, 0.45, 24.0),
            "metal" => (0.8, 0.9, 0.7, 40.0),
            "softmetal" => (0.9, 0.95, 0.35, 12.0),
            "powder" | "translucentPowder" => (0.85, 1.12, 0.0, 1.0),
            "dkEdge" => (1.12, 0.8, 0.1, 8.0),
            "softEdge" => (0.9, 1.08, 0.05, 6.0),
            "clear" => (0.9, 1.0, 0.6, 50.0),
            _ => (1.0, 1.0, 0.0, 1.0),
        };
        Lighting {
            lights: lights.into_iter().map(|(az, el, s)| (dir(az + turn, el), s)).collect(),
            ambient: ambient * ak,
            diffuse: 0.731 * dk,
            lift: 14.7,
            tint,
            specular,
            shininess,
            flat: rig_name == "flat" || material == "flat",
        }
    }

    /// The diffuse brightness factor for unit normal `n` (the three-point matte rig: 0.453 +
    /// 0.731 · max(0, n · L)).
    pub fn factor(&self, n: [f64; 3]) -> f64 {
        if self.flat {
            return 1.0;
        }
        let dot = |l: &[f64; 3]| (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
        self.ambient + self.diffuse * self.lights.iter().map(|(l, s)| s * dot(l)).sum::<f64>()
    }

    /// The specular highlight (0–1) for unit normal `n`, seen from the front.
    pub fn highlight(&self, n: [f64; 3]) -> f64 {
        if self.flat || self.specular <= 0.0 {
            return 0.0;
        }
        self.lights
            .iter()
            .map(|(l, s)| {
                let h = [l[0], l[1], l[2] + 1.0];
                let len = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt().max(1e-9);
                let d = ((n[0] * h[0] + n[1] * h[1] + n[2] * h[2]) / len).max(0.0);
                s * d.powf(self.shininess)
            })
            .sum::<f64>()
            * self.specular
    }

    /// Colour `c` lit on a surface with unit normal `n`.
    pub fn shade(&self, c: Rgba, n: [f64; 3]) -> Rgba {
        if self.flat {
            return c;
        }
        let f = self.factor(n);
        let hi = self.highlight(n) * 255.0;
        let ch = |v: u8, t: f64| (f64::from(v) * f * t + self.lift + hi).round().clamp(0.0, 255.0) as u8;
        Rgba::rgba(ch(c.r, self.tint[0]), ch(c.g, self.tint[1]), ch(c.b, self.tint[2]), c.a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_point_matte_is_the_measured_formula() {
        let l = Lighting::from_raw(r#"<a:lightRig rig="threePt" dir="t"/>"#);
        let n = [0.0, 0.0, 1.0];
        let expect = 0.453 + 0.731 * 45f64.to_radians().sin();
        assert!((l.factor(n) - expect).abs() < 1e-9);
        assert_eq!(l.highlight(n), 0.0);
        let c = l.shade(Rgba::rgb(100, 100, 100), n);
        assert_eq!(c.r, (100.0 * expect + 14.7_f64).round() as u8);
    }

    #[test]
    fn rigs_and_materials_differ_as_named() {
        let face = [0.0, 0.0, 1.0];
        let f = |raw: &str| Lighting::from_raw(raw).factor(face);
        assert!(f(r#"rig="brightRoom""#) > f(r#"rig="threePt""#), "a bright room is brighter");
        assert!(f(r#"rig="harsh""#) < f(r#"rig="soft""#) + 0.2);
        assert_eq!(Lighting::from_raw(r#"rig="flat""#).shade(Rgba::rgb(10, 20, 30), face), Rgba::rgb(10, 20, 30));
        // Metal shines where plastic shines less and matte not at all.
        let up_left = {
            let d = dir(240.0, 45.0);
            let h = [d[0], d[1], d[2] + 1.0];
            let l = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt();
            [h[0] / l, h[1] / l, h[2] / l]
        };
        let hi = |m: &str| Lighting::from_raw(&format!(r#"prstMaterial="{m}""#)).highlight(up_left);
        assert!(hi("metal") > hi("plastic") && hi("plastic") > 0.0 && hi("matte") == 0.0);
        // Turning the rig to the right moves the key light: a right-facing surface gets brighter.
        let right = [0.7, 0.0, 0.7];
        assert!(Lighting::from_raw(r#"rig="threePt" dir="r""#).factor(right) > Lighting::from_raw(r#"rig="threePt" dir="t""#).factor(right));
        // Sunset is warm.
        let warm = Lighting::from_raw(r#"rig="sunset""#).shade(Rgba::rgb(200, 200, 200), face);
        assert!(warm.r > warm.b);
    }
}
