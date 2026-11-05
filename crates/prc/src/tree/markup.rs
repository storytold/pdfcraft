//! What a tree carries beside its geometry, read past and not modelled: markup data (PMI)
//! and views [WD 7.3.10.4, 7.7], and a scene's lights and clipping planes [WD 7.5.15-7.5.18,
//! 7.11.13]. The tree section has no length to skip by, so each is read field by field.

use super::{
    AMBIENT_LIGHT, ANNOTATION_ITEM, ANNOTATION_REFERENCE, ANNOTATION_SET, DIRECTIONAL_LIGHT, LIGHT_INTENSITY_FROM, MARKUP_LINKED_ITEM, MAX_DEPTH,
    MKP_LEADER, MKP_MARKUP, MKP_VIEW, POINT_LIGHT, SPOT_LIGHT, SURF_PLANE, VIEW_FLAGS_FROM,
};
use crate::PrcError;
use crate::entity::{Ctx, malformed};

impl Ctx<'_, '_> {
    /// `MarkupData` [WD 7.3.10.4]: linked items, leaders, markups, then annotation entities.
    pub(super) fn markup_data(&mut self) -> Result<(), PrcError> {
        let n = self.count(1, "markup linked items")?;
        for _ in 0..n {
            self.markup_linked_item()?;
        }
        let n = self.count(1, "leaders")?;
        for _ in 0..n {
            self.leader()?;
        }
        let n = self.count(1, "markups")?;
        for _ in 0..n {
            self.mkp_markup()?;
        }
        let n = self.count(1, "annotation entities")?;
        for _ in 0..n {
            self.annotation_entity(0)?;
        }
        Ok(())
    }

    /// A counted list of `MKP_View`s.
    pub(super) fn views(&mut self) -> Result<(), PrcError> {
        let n = self.count(1, "views")?;
        for _ in 0..n {
            self.view()?;
        }
        Ok(())
    }

    /// A counted list of `ReferenceOnPRCBase`s.
    fn references(&mut self, what: &'static str) -> Result<(), PrcError> {
        let n = self.count(1, what)?;
        for _ in 0..n {
            self.reference_on_prc_base()?;
        }
        Ok(())
    }

    /// `MISC_MarkupLinkedItem` (204) [WD 7.4.5].
    // prc-rs: a `has_reference_data` flag and its reference data follow the content entity
    // reference (pdf-issues #722).
    fn markup_linked_item(&mut self) -> Result<(), PrcError> {
        self.expect_type(MARKUP_LINKED_ITEM)?;
        self.base_with_graphics()?;
        self.r.unsigned_integer()?; // local coordinate system + 1
        if self.r.bit()? {
            self.reference_data()?;
        }
        if self.r.bit()? {
            self.reference_data()?;
        }
        for _ in 0..4 {
            self.r.bit()?; // show and delete, markup and leader
        }
        self.schema.skip_added_fields(MARKUP_LINKED_ITEM, &mut self.r)?;
        self.user_data()
    }

    /// `MKP_Leader` (503) [WD 7.7.5; pdf-issues #698].
    // prc-rs: the leader's field order.
    fn leader(&mut self) -> Result<(), PrcError> {
        self.expect_type(MKP_LEADER)?;
        self.base_with_graphics()?;
        for _ in 0..2 {
            if self.r.bit()? {
                self.reference_on_prc_base()?; // first, second linked item
            }
        }
        self.r.unsigned_integer()?; // tessellation + 1
        self.schema.skip_added_fields(MKP_LEADER, &mut self.r)?;
        self.user_data()
    }

    /// `MKP_Markup` (502) [WD 7.7.4].
    // prc-rs: the markup's field order.
    fn mkp_markup(&mut self) -> Result<(), PrcError> {
        self.expect_type(MKP_MARKUP)?;
        self.base_with_graphics()?;
        self.r.unsigned_integer()?; // type
        self.r.unsigned_integer()?; // subtype
        self.references("markup linked items")?;
        self.references("markup leaders")?;
        self.r.unsigned_integer()?; // tessellation + 1
        self.schema.skip_added_fields(MKP_MARKUP, &mut self.r)?;
        self.user_data()
    }

    /// An annotation item (504), set (505) or reference (506) [WD 7.7.6-7.7.8]; sets nest.
    // prc-rs: the annotation entities' field orders.
    fn annotation_entity(&mut self, depth: usize) -> Result<(), PrcError> {
        if depth > MAX_DEPTH {
            return Err(malformed("annotation sets nest too deep".into()));
        }
        let t = self.r.unsigned_integer()?;
        if !matches!(t, ANNOTATION_ITEM | ANNOTATION_SET | ANNOTATION_REFERENCE) {
            return Err(malformed(format!("entity type {t} as an annotation")));
        }
        self.base_with_graphics()?;
        match t {
            ANNOTATION_ITEM => self.reference_on_prc_base()?,
            ANNOTATION_SET => {
                let n = self.count(1, "annotation set")?;
                for _ in 0..n {
                    self.annotation_entity(depth + 1)?;
                }
            }
            _ => self.references("annotation reference")?,
        }
        self.schema.skip_added_fields(t, &mut self.r)?;
        self.user_data()
    }

    /// `MKP_View` (501) [WD 7.7.3].
    // prc-rs: the view's field order, and its flags, linked items and filters from version
    // 8137.
    fn view(&mut self) -> Result<(), PrcError> {
        self.expect_type(MKP_VIEW)?;
        self.base_with_graphics()?;
        self.references("view annotations")?;
        self.plane()?;
        if self.r.bit()? {
            self.scene_display_parameters()?;
        }
        if self.version >= VIEW_FLAGS_FROM {
            for _ in 0..3 {
                self.r.bit()?; // annotation view, default view, direction
            }
            self.references("view linked items")?;
            let n = self.count(1, "view filters")?;
            for _ in 0..n {
                self.filter()?;
            }
        }
        self.schema.skip_added_fields(MKP_VIEW, &mut self.r)?;
        self.user_data()
    }

    /// `SURF_Plane` (86) [WD 7.11.13; pdf-issues #658]: `ContentSurface` [WD 7.11.3.2], a
    /// `Transformation3D` with no presence flag, the `Domain` [WD 8.2.4] and the
    /// parameterisation's four coefficients.
    // prc-rs: the plane's field order.
    pub(super) fn plane(&mut self) -> Result<(), PrcError> {
        self.expect_type(SURF_PLANE)?;
        if self.r.bit()? {
            self.content_prc_base()?; // base geometry: attributes and name
            self.r.unsigned_integer()?; // identifier
        }
        self.r.unsigned_integer()?; // extend type
        self.transformation_3d()?;
        for _ in 0..8 {
            self.r.double()?; // domain (4), coefficients (4)
        }
        self.schema.skip_added_fields(SURF_PLANE, &mut self.r)
    }

    /// A light (731-734) [WD 7.5.15-7.5.18].
    // prc-rs: the lights' field orders, and a directional light's intensity from version 8030
    // (pdf-issues #745).
    pub(super) fn light(&mut self) -> Result<(), PrcError> {
        let t = self.r.unsigned_integer()?;
        if !(AMBIENT_LIGHT..=SPOT_LIGHT).contains(&t) {
            return Err(malformed(format!("entity type {t} as a light")));
        }
        self.content_prc_ref_base()?;
        for _ in 0..3 {
            self.r.unsigned_integer()?; // ambient, diffuse, specular colour + 1
        }
        if matches!(t, POINT_LIGHT | SPOT_LIGHT) {
            self.vector3()?; // location
            for _ in 0..3 {
                self.r.double()?; // attenuation
            }
        }
        if matches!(t, DIRECTIONAL_LIGHT | SPOT_LIGHT) {
            self.vector3()?; // direction
        }
        if t == DIRECTIONAL_LIGHT && self.version >= LIGHT_INTENSITY_FROM {
            self.r.double()?;
        }
        if t == SPOT_LIGHT {
            self.r.double()?; // fall-off angle
            self.r.double()?; // fall-off exponent
        }
        self.schema.skip_added_fields(t, &mut self.r)
    }
}
