//! Graphics, styles, materials and the globals that hold them [WD 7.2.4, 7.3.5, 7.5].

use super::{
    FATHER_HERIT_COLOR, FILE_STRUCTURE_GLOBALS, LINE_PATTERN, MATERIAL, Matrix, PICTURE, RI_COORDINATE_SYSTEM, SON_HERIT_COLOR, STYLE,
    TEXTURE_ALPHA_TEST, TEXTURE_APPLICATION, TEXTURE_DEFINITION, TEXTURE_FUNCTION_BLEND, TEXTURE_MAPPING_OPERATOR, TEXTURE_TRANSFORMATION, Vec3,
};
use crate::PrcError;
use crate::container::offset;
use crate::entity::{Ctx, malformed};

/// One entity's `GraphicsContent` [WD 7.2.4]: `style` is `line_style_index + 1` (0 none),
/// `bits` the behaviour bits. `fs` is the file structure whose globals `style` indexes: the
/// one whose tree holds the entity. The walk sets it; a reader leaves it 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Graphics {
    pub(crate) style: u32,
    pub(crate) bits: u16,
    pub(crate) fs: usize,
}

/// A straight (not premultiplied) colour, each channel 0-1; `a` is the opacity.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct Rgba {
    /// Red.
    pub r: f64,
    /// Green.
    pub g: f64,
    /// Blue.
    pub b: f64,
    /// Opacity: 0 transparent, 1 opaque.
    pub a: f64,
}

/// How a style's `transparency` combines with its material's diffuse alpha. ISO 14739-1
/// defines both and not their combination, and gives a diffuse alpha of 0.0 no "unset"
/// meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum StyleAlpha {
    /// As [`Self::StyleWins`], except that a material diffuse alpha of exactly 0.0 under a
    /// style with no transparency reads as unset, so opaque. Producers write 0.0 on the
    /// materials of models meant to be seen; read literally, every part is invisible.
    #[default]
    ZeroUnset,
    /// The style's transparency, when it has one, is the opacity; the material alpha, read
    /// literally (0.0 invisible), applies only to a style without one.
    StyleWins,
    /// The opacity is material alpha × style transparency.
    Multiply,
}

/// A style's resolved colour, and whether its opacity comes from reading a zero material
/// alpha as unset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Paint {
    pub(crate) rgba: Rgba,
    pub(crate) alpha_unset: bool,
}

/// A `Style` (701) as far as colour: `index` is `colour_or_material + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Style {
    pub(crate) is_material: bool,
    pub(crate) index: u32,
    pub(crate) transparency: Option<u8>,
}

/// A `materials` entry as far as colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Material {
    /// `Material` (702): `diffuse + 1` (a colour index) and the diffuse alpha.
    Plain { diffuse: u32, alpha: f64 },
    /// `TextureApplication` (711) [WD 7.5.6]: `material_generic_index + 1`, the material
    /// under the texture.
    Textured { base: u32 },
}

/// What `FileStructureGlobals` contributes to placing and colouring.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Globals {
    /// Entry `i` places an item with biased local coordinate system `i + 1`.
    pub(crate) systems: Vec<Matrix>,
    /// RGB, `a` 1.
    pub(crate) colours: Vec<Rgba>,
    pub(crate) materials: Vec<Material>,
    pub(crate) styles: Vec<Style>,
}

/// The entry at biased index `biased` (`index + 1`); `None` for 0 or past the end.
pub(crate) fn biased<T>(list: &[T], biased: u32) -> Option<&T> {
    list.get(offset(biased.checked_sub(1)?))
}

impl Globals {
    /// The colour a style or material names by `biased` (`index + 1`). Stored indices count
    /// the colour array's doubles, so `colours[i]` is stored as `3i + 1`; an index that is
    /// not of that form names nothing.
    fn colour(&self, biased: u32) -> Option<Rgba> {
        let i = biased.checked_sub(1)?;
        // pdf-issues #816: a colour index counts doubles, three per colour.
        if i % 3 != 0 {
            return None;
        }
        self.colours.get(offset(i / 3)).copied()
    }

    /// The colour of style `biased` (`line_style_index + 1`) under `rule`.
    pub(crate) fn style_paint(&self, biased_style: u32, rule: StyleAlpha) -> Option<Paint> {
        let style = biased(&self.styles, biased_style)?;
        let (rgb, mut alpha) = if style.is_material {
            let mut m = biased(&self.materials, style.index)?;
            if let Material::Textured { base } = *m {
                m = biased(&self.materials, base)?;
            }
            match *m {
                Material::Plain { diffuse, alpha } => (self.colour(diffuse)?, alpha),
                Material::Textured { .. } => return None,
            }
        } else {
            (self.colour(style.index)?, 1.0)
        };
        let mut alpha_unset = false;
        match style.transparency {
            Some(t) => {
                let t = f64::from(t) / 255.0;
                alpha = match rule {
                    StyleAlpha::ZeroUnset | StyleAlpha::StyleWins => t,
                    StyleAlpha::Multiply => alpha * t,
                };
            }
            None if rule == StyleAlpha::ZeroUnset && style.is_material && alpha == 0.0 => {
                alpha = 1.0;
                alpha_unset = true;
            }
            None => {}
        }
        // A NaN alpha (a stored double may be one) reads as opaque.
        let a = if alpha.is_nan() { 1.0 } else { alpha.clamp(0.0, 1.0) };
        Some(Paint { rgba: Rgba { a, ..rgb }, alpha_unset })
    }
}

/// The style a chain of graphics resolves to, outermost first [WD 7.2.4.2]: a son's own
/// style is used unless an ancestor set `FatherHeritColor` (the oldest such wins), and a son
/// setting `SonHeritColor` overrides that. Entities with no style inherit. Returns the
/// winning entity's graphics, style 0 when none.
pub(crate) fn resolve_style(chain: &[Graphics]) -> Graphics {
    let (mut won, mut forced) = (Graphics::default(), false);
    for g in chain {
        if g.style == 0 {
            continue;
        }
        if !forced || g.bits & SON_HERIT_COLOR != 0 {
            won = *g;
            forced = g.bits & FATHER_HERIT_COLOR != 0;
        }
    }
    won
}

impl Ctx<'_, '_> {
    /// `FileStructureGlobals` (303) [WD 7.3.5]: colours, materials, styles and the reference
    /// coordinate systems. Pictures, textures and line patterns are read past; fonts and fill
    /// patterns are [`PrcError::Unsupported`].
    // prc-rs: the globals' field order.
    pub(crate) fn globals(&mut self) -> Result<Globals, PrcError> {
        let mut g = Globals::default();
        self.expect_type(FILE_STRUCTURE_GLOBALS)?;
        self.content_prc_base()?;
        let n = self.count(4, "referenced file structures")?;
        for _ in 0..n {
            self.unique_id()?;
        }
        self.r.double()?; // tessellation chord-height ratio
        self.r.double()?; // tessellation angle
        self.r.string()?; // default font family
        if self.r.unsigned_integer()? != 0 {
            return Err(PrcError::Unsupported("PRC global fonts"));
        }
        let n = self.count(6, "colours")?;
        g.colours = (0..n).map(|_| self.vector3().map(|Vec3([r, g, b])| Rgba { r, g, b, a: 1.0 })).collect::<Result<_, _>>()?;
        let n = self.count(1, "pictures")?;
        for _ in 0..n {
            self.picture()?;
        }
        let n = self.count(1, "texture definitions")?;
        for _ in 0..n {
            self.texture_definition()?;
        }
        let n = self.count(1, "materials")?;
        g.materials = (0..n).map(|_| self.material()).collect::<Result<_, _>>()?;
        let n = self.count(1, "line patterns")?;
        for _ in 0..n {
            self.line_pattern()?;
        }
        let n = self.count(1, "styles")?;
        g.styles = (0..n).map(|_| self.style()).collect::<Result<_, _>>()?;
        if self.r.unsigned_integer()? != 0 {
            return Err(PrcError::Unsupported("PRC fill patterns"));
        }
        let n = self.count(1, "reference coordinate systems")?;
        for _ in 0..n {
            self.expect_type(RI_COORDINATE_SYSTEM)?;
            self.base_with_graphics()?;
            self.r.unsigned_integer()?; // local coordinate system + 1
            self.r.unsigned_integer()?; // tessellation + 1
            g.systems.push(self.transformation()?);
            self.schema.skip_added_fields(RI_COORDINATE_SYSTEM, &mut self.r)?;
            self.user_data()?;
        }
        self.schema.skip_added_fields(FILE_STRUCTURE_GLOBALS, &mut self.r)?;
        self.user_data()?;
        Ok(g)
    }

    /// `LinePattern` (721) [WD 7.5.2]: read past.
    fn line_pattern(&mut self) -> Result<(), PrcError> {
        self.expect_type(LINE_PATTERN)?;
        self.content_prc_ref_base()?;
        self.doubles("line pattern lengths")?;
        self.r.double()?; // phase
        self.r.bit()?; // is_real_length
        self.schema.skip_added_fields(LINE_PATTERN, &mut self.r)
    }

    /// One `materials` entry, type-tagged: `Material` (702) or `TextureApplication` (711)
    /// [WD 7.5.4, 7.5.6].
    // prc-rs: a material entry is type-tagged, then a ref base, then its fields.
    fn material(&mut self) -> Result<Material, PrcError> {
        let t = self.r.unsigned_integer()?;
        self.content_prc_ref_base()?;
        let m = match t {
            MATERIAL => {
                self.r.unsigned_integer()?; // ambient + 1
                let diffuse = self.r.unsigned_integer()?;
                self.r.unsigned_integer()?; // emissive + 1
                self.r.unsigned_integer()?; // specular + 1
                self.r.double()?; // shininess
                self.r.double()?; // ambient alpha
                let alpha = self.r.double()?;
                self.r.double()?; // emissive alpha
                self.r.double()?; // specular alpha
                Material::Plain { diffuse, alpha }
            }
            TEXTURE_APPLICATION => {
                let base = self.r.unsigned_integer()?;
                self.r.unsigned_integer()?; // texture definition + 1
                self.r.unsigned_integer()?; // next texture + 1
                self.r.unsigned_integer()?; // UV coordinates set + 1
                Material::Textured { base }
            }
            t => return Err(malformed(format!("entity type {t} as a material"))),
        };
        self.schema.skip_added_fields(t, &mut self.r)?;
        Ok(m)
    }

    /// `Style` (701) [WD 7.5.3].
    // prc-rs: the transparency and the three rendering parameters each follow a presence bit.
    fn style(&mut self) -> Result<Style, PrcError> {
        self.expect_type(STYLE)?;
        self.content_prc_ref_base()?;
        self.r.double()?; // line width
        self.r.bit()?; // is_vpicture
        self.r.unsigned_integer()?; // line pattern + 1
        let is_material = self.r.bit()?;
        let index = self.r.unsigned_integer()?;
        let transparency = if self.r.bit()? { Some(self.r.character()?) } else { None };
        for _ in 0..3 {
            if self.r.bit()? {
                self.r.character()?;
            }
        }
        self.schema.skip_added_fields(STYLE, &mut self.r)?;
        Ok(Style { is_material, index, transparency })
    }

    /// `Picture` (703) [WD 7.5.5]: read past.
    fn picture(&mut self) -> Result<(), PrcError> {
        self.expect_type(PICTURE)?;
        self.content_prc_base()?;
        for _ in 0..4 {
            self.r.unsigned_integer()?; // format, file + 1, width, height
        }
        self.schema.skip_added_fields(PICTURE, &mut self.r)
    }

    /// `TextureDefinition` (712) [WD 7.5.7; pdf-issues #485]: read past.
    fn texture_definition(&mut self) -> Result<(), PrcError> {
        self.expect_type(TEXTURE_DEFINITION)?;
        self.content_prc_ref_base()?;
        self.r.unsigned_integer()?; // picture + 1
        let dimension = self.r.character()?;
        if self.r.integer()? == TEXTURE_MAPPING_OPERATOR {
            self.r.integer()?; // mapping operator
            if self.r.bit()? {
                self.transformation()?;
            }
        }
        self.r.unsigned_integer()?; // channels
        self.doubles("texture intensities")?;
        let n = self.count(8, "texture components")?;
        for _ in 0..n {
            self.r.character()?;
        }
        if self.r.integer()? == TEXTURE_FUNCTION_BLEND {
            for _ in 0..4 {
                self.r.double()?; // blend colour
            }
        }
        for _ in 0..2 {
            // RGB, then alpha: a source blend, and a destination one when set.
            if self.r.integer()? != 0 {
                self.r.integer()?;
            }
        }
        if self.r.character()? & TEXTURE_ALPHA_TEST != 0 {
            self.r.integer()?; // alpha test function
            self.r.double()?; // reference
        }
        for _ in 0..dimension.clamp(1, 3) {
            self.r.integer()?; // wrapping mode
        }
        if self.r.bit()? {
            self.texture_transformation()?;
        }
        self.schema.skip_added_fields(TEXTURE_DEFINITION, &mut self.r)
    }

    /// `TextureTransformation` (713) [WD 7.5.8, 7.4.11.1; pdf-issues #819]: read past.
    fn texture_transformation(&mut self) -> Result<(), PrcError> {
        self.expect_type(TEXTURE_TRANSFORMATION)?;
        self.r.bit()?; // invert u
        self.r.bit()?; // invert v
        self.r.bit()?; // is 2D
        let b = self.r.character()?;
        // Translation; non-orthogonal axes, else one rotation vector; non-uniform, else
        // uniform scale; homogeneous row.
        let doubles = [(0x01, 2), (0x20, 4), (0x10, 2), (0x40, 3)].iter().filter(|(bit, _)| b & bit != 0).map(|(_, n)| n).sum::<usize>()
            + if b & 0x20 == 0 && b & 0x02 != 0 { 2 } else { 0 }
            + usize::from(b & 0x10 == 0 && b & 0x08 != 0);
        for _ in 0..doubles {
            self.r.double()?;
        }
        self.schema.skip_added_fields(TEXTURE_TRANSFORMATION, &mut self.r)
    }
}
