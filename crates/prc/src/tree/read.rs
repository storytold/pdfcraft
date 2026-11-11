//! The tree section's and the model file's entities: part definitions, representation items,
//! product occurrences and their transformations and entity references [WD 7.3, 7.4, 7.6].

use super::{
    ABSOLUTE_SCENE_FROM, BASE_WITH_GRAPHICS, CAMERA, CARTESIAN_TRANSFORMATION, ENTITY_REFERENCE, FILE_STRUCTURE, FILE_STRUCTURE_TREE, FILTER,
    GENERAL_TRANSFORMATION, Graphics, Item, MAX_DEPTH, MODEL_FILE, Matrix, ModelFile, Override, PART_DEFINITION, PRODUCT_OCCURRENCE, Product,
    REFERENCE_ON_PRC_BASE, REFERENCE_ON_TOPOLOGY, REMOVED, RI_BREP_MODEL, RI_COORDINATE_SYSTEM, RI_CURVE, RI_DIRECTION, RI_PLANE, RI_POINT_SET,
    RI_POLY_BREP_MODEL, RI_POLY_WIRE, RI_SET, SCENE_DISPLAY_PARAMETERS, SHOW, SUPPRESSED, Target, Tree, UniqueId, Vec3,
};
use crate::PrcError;
use crate::entity::{Ctx, malformed};

impl Ctx<'_, '_> {
    pub(super) fn vector3(&mut self) -> Result<Vec3, PrcError> {
        Ok(Vec3([self.r.double()?, self.r.double()?, self.r.double()?]))
    }

    pub(super) fn unique_id(&mut self) -> Result<UniqueId, PrcError> {
        let mut id = [0; 4];
        for v in &mut id {
            *v = self.r.unsigned_integer()?;
        }
        Ok(UniqueId(id))
    }

    /// `ContentPRCRefBase` [WD 7.2.3.2; pdf-issues #697]: the base, then the CAD ids and the
    /// PRC unique id, which becomes [`Ctx::uid`].
    pub(super) fn content_prc_ref_base(&mut self) -> Result<(), PrcError> {
        self.content_prc_base()?;
        self.r.unsigned_integer()?; // CAD identifier
        self.r.unsigned_integer()?; // CAD persistent identifier
        self.uid = self.r.unsigned_integer()?;
        Ok(())
    }

    /// `PRCBaseWithGraphics` [WD 7.2.4; pdf-issues #405]: whether the entity's own graphics
    /// hide it (Show clear or Removed set), and its graphics. Reused graphics
    /// (`same_graphics`) are the current graphics and never hide.
    pub(super) fn base_with_graphics(&mut self) -> Result<(bool, Graphics), PrcError> {
        self.content_prc_ref_base()?;
        let hidden = if self.r.bit()? {
            false
        } else {
            self.r.unsigned_integer()?; // layer + 1
            let style = self.r.unsigned_integer()?;
            let lo = u16::from(self.r.character()?);
            let hi = u16::from(self.r.character()?);
            let bits = lo | hi << 8;
            self.graphics = Graphics { style, bits, fs: 0 };
            bits & SHOW == 0 || bits & REMOVED != 0
        };
        // prc-rs: schema additions to the base itself follow the graphics.
        self.schema.skip_added_fields(BASE_WITH_GRAPHICS, &mut self.r)?;
        Ok((hidden, self.graphics))
    }

    /// `CartesianTransformation` (202) or `GeneralTransformation` (207), type code included
    /// [WD 7.4.8, 7.4.9, 7.4.11; pdf-issues #430].
    pub(super) fn transformation(&mut self) -> Result<Matrix, PrcError> {
        let t = self.r.unsigned_integer()?;
        let m = match t {
            GENERAL_TRANSFORMATION => {
                // Column-major [WD 7.4.9].
                let mut m = Matrix::IDENTITY;
                for col in 0..4 {
                    for row in &mut m.0 {
                        if let Some(c) = row.get_mut(col) {
                            *c = self.r.double()?;
                        }
                    }
                }
                m
            }
            CARTESIAN_TRANSFORMATION => self.transformation_3d()?,
            t => return Err(malformed(format!("entity type {t} as a transformation"))),
        };
        self.schema.skip_added_fields(t, &mut self.r)?;
        Ok(m)
    }

    /// The embedded `Transformation3D` [WD 7.4.11.1]: a behaviour byte, then each field its
    /// bits name.
    pub(super) fn transformation_3d(&mut self) -> Result<Matrix, PrcError> {
        let b = self.r.character()?;
        let mut m = Matrix::IDENTITY;
        let set_col = |m: &mut Matrix, col: usize, v: Vec3| {
            for (row, x) in m.0.iter_mut().zip(v.0) {
                if let Some(c) = row.get_mut(col) {
                    *c = x;
                }
            }
        };
        if b & 0x01 != 0 {
            let t = self.vector3()?;
            set_col(&mut m, 3, t);
        }
        if b & 0x20 != 0 {
            for col in 0..3 {
                let v = self.vector3()?;
                set_col(&mut m, col, v);
            }
        } else if b & 0x02 != 0 {
            let x = self.vector3()?;
            let y = self.vector3()?;
            // Mirrored (0x04): the third axis is y × x.
            let z = if b & 0x04 != 0 { y.cross(x) } else { x.cross(y) };
            set_col(&mut m, 0, x);
            set_col(&mut m, 1, y);
            set_col(&mut m, 2, z);
        }
        let mut scale = [1.0; 3];
        if b & 0x10 != 0 {
            scale = self.vector3()?.0;
        }
        // prc-rs: with both scale bits set, both are read and multiply.
        if b & 0x08 != 0 {
            let s = self.r.double()?;
            scale = scale.map(|v| v * s);
        }
        for row in m.0.iter_mut().take(3) {
            for (c, s) in row.iter_mut().zip(scale) {
                *c *= s;
            }
        }
        if b & 0x40 != 0 {
            let h = [self.r.double()?, self.r.double()?, self.r.double()?, self.r.double()?];
            if let Some(r) = m.0.get_mut(3) {
                *r = h;
            }
        }
        Ok(m)
    }

    /// A polymorphic representation item [WD 7.6]; drawable ones are pushed onto `out` with
    /// `local` (the enclosing sets' coordinate systems) prefixed.
    // prc-rs: the item's own fields follow its local coordinate system and tessellation.
    fn representation_item(&mut self, at: &Item, out: &mut Vec<Item>, depth: usize) -> Result<(), PrcError> {
        if depth > MAX_DEPTH {
            return Err(malformed("representation sets nest too deep".into()));
        }
        let t = self.r.unsigned_integer()?;
        let (hidden, own) = self.base_with_graphics()?;
        let mut item = at.clone();
        item.uids.push(self.uid);
        item.graphics.push(own);
        let cs = self.r.unsigned_integer()?;
        let tess = self.r.unsigned_integer()?;
        if cs != 0 {
            item.local.push(cs);
        }
        let (drawable, brep) = if t == RI_SET {
            let n = self.count(1, "representation set")?;
            let mut members = Vec::new();
            for _ in 0..n {
                self.representation_item(&item, &mut members, depth + 1)?;
            }
            if !hidden {
                out.extend(members);
            }
            (false, None)
        } else {
            self.leaf_item_fields(t)?
        };
        self.schema.skip_added_fields(t, &mut self.r)?;
        self.user_data()?;
        if drawable && !hidden && tess != 0 {
            out.push(Item { tessellation: tess, brep, ..item });
        }
        Ok(())
    }

    /// The type-specific fields of a representation item other than `RI_Set` [WD 7.6];
    /// whether the item can be drawn, and a B-rep model's `(context, body)`.
    fn leaf_item_fields(&mut self, t: u32) -> Result<(bool, Option<(u32, u32)>), PrcError> {
        match t {
            RI_BREP_MODEL => {
                let brep = self.exact_tolerance()?;
                self.r.bit()?; // is_closed
                return Ok((true, brep));
            }
            RI_CURVE | RI_PLANE => {
                self.exact_tolerance()?;
            }
            RI_DIRECTION => {
                if self.r.bit()? {
                    self.vector3()?; // origin
                }
                self.vector3()?;
                return Ok((false, None));
            }
            RI_POINT_SET => {
                let n = self.count(6, "point set")?;
                for _ in 0..n {
                    self.vector3()?;
                }
            }
            RI_POLY_BREP_MODEL => {
                self.r.bit()?; // is_closed
            }
            RI_POLY_WIRE => {}
            RI_COORDINATE_SYSTEM => {
                self.transformation()?;
                return Ok((false, None));
            }
            t => return Err(malformed(format!("entity type {t} as a representation item"))),
        }
        Ok((true, None))
    }

    /// A B-rep, curve or plane item's optional exact-geometry `(context, body)` ids
    /// [WD 7.6.2].
    fn exact_tolerance(&mut self) -> Result<Option<(u32, u32)>, PrcError> {
        if self.r.bit()? {
            return Ok(Some((self.r.unsigned_integer()?, self.r.unsigned_integer()?)));
        }
        Ok(None)
    }

    /// `SceneDisplayParameters` (741) [WD 7.5.19]: read past, lights and clipping planes
    /// included.
    // prc-rs: the scene's field order, and `is_absolute` from version 8137.
    pub(super) fn scene_display_parameters(&mut self) -> Result<(), PrcError> {
        self.expect_type(SCENE_DISPLAY_PARAMETERS)?;
        self.content_prc_ref_base()?;
        self.r.bit()?; // is_active
        let n = self.count(1, "scene lights")?;
        for _ in 0..n {
            self.light()?;
        }
        if self.r.bit()? {
            self.camera()?;
        }
        if self.r.bit()? {
            self.vector3()?; // rotation centre
        }
        let n = self.count(1, "scene clipping planes")?;
        for _ in 0..n {
            self.plane()?;
        }
        self.r.unsigned_integer()?; // background style + 1
        self.r.unsigned_integer()?; // default style + 1
        let n = self.count(2, "scene default styles")?;
        for _ in 0..n {
            self.r.unsigned_integer()?; // entity type
            self.r.unsigned_integer()?; // style + 1
        }
        if self.version >= ABSOLUTE_SCENE_FROM {
            self.r.bit()?; // is_absolute
        }
        self.schema.skip_added_fields(SCENE_DISPLAY_PARAMETERS, &mut self.r)
    }

    /// `Camera` (742) [WD 7.5.20]: read past.
    // prc-rs: the camera's field order.
    fn camera(&mut self) -> Result<(), PrcError> {
        self.expect_type(CAMERA)?;
        self.content_prc_ref_base()?;
        self.r.bit()?; // is_orthographic
        for _ in 0..3 {
            self.vector3()?; // position, look, up
        }
        for _ in 0..6 {
            self.r.double()?; // x, y, ratio, near, far, zoom
        }
        self.schema.skip_added_fields(CAMERA, &mut self.r)
    }

    /// `MISC_EntityReference` (203) [WD 7.4.4, 7.4.10]; `None` when it names no target.
    // prc-rs: the local coordinate system precedes the optional reference data.
    fn entity_reference(&mut self) -> Result<Option<Override>, PrcError> {
        self.expect_type(ENTITY_REFERENCE)?;
        let (hidden, graphics) = self.base_with_graphics()?;
        let local = self.r.unsigned_integer()?;
        let target = if self.r.bit()? { self.reference_data()? } else { None };
        self.schema.skip_added_fields(ENTITY_REFERENCE, &mut self.r)?;
        self.user_data()?;
        Ok(target.map(|target| Override { graphics, hidden, local, target }))
    }

    /// `ReferenceData` [WD 7.4.10.2]: a `ReferenceOnPRCBase` (205) or a
    /// `ReferenceOnTopology` (206), type code included; `None` when it names no target.
    pub(super) fn reference_data(&mut self) -> Result<Option<Target>, PrcError> {
        let t = self.r.unsigned_integer()?;
        let target = match t {
            REFERENCE_ON_PRC_BASE => Some(self.reference_on_prc_base_body()?),
            REFERENCE_ON_TOPOLOGY => self.reference_on_topology()?,
            t => return Err(malformed(format!("entity type {t} as reference data"))),
        };
        self.schema.skip_added_fields(t, &mut self.r)?;
        Ok(target)
    }

    /// `ReferenceOnPRCBase` (205) [WD 7.4.6] after its type code.
    fn reference_on_prc_base_body(&mut self) -> Result<Target, PrcError> {
        let kind = self.r.unsigned_integer()?;
        let fs = self.file_identifier()?;
        let uid = self.r.unsigned_integer()?;
        Ok(Target::Entity { kind, fs, uid })
    }

    /// `ReferenceOnPRCBase` (205) [WD 7.4.6], type code included: read past.
    pub(super) fn reference_on_prc_base(&mut self) -> Result<(), PrcError> {
        self.expect_type(REFERENCE_ON_PRC_BASE)?;
        self.reference_on_prc_base_body()?;
        self.schema.skip_added_fields(REFERENCE_ON_PRC_BASE, &mut self.r)
    }

    /// `ReferenceOnTopology` (206) after its type code; `None` without a body.
    fn reference_on_topology(&mut self) -> Result<Option<Target>, PrcError> {
        let kind = self.r.unsigned_integer()?;
        if !self.r.bit()? {
            return Ok(None);
        }
        let fs = self.file_identifier()?;
        let brep = (self.r.unsigned_integer()?, self.r.unsigned_integer()?);
        let indices = self.uints("topology reference indices")?;
        Ok(Some(Target::Topology { kind, fs, brep, indices }))
    }

    /// `ASM_Filter` (320) [WD 7.3.12]: read past.
    // prc-rs: the filter's field order.
    pub(super) fn filter(&mut self) -> Result<(), PrcError> {
        self.expect_type(FILTER)?;
        self.content_prc_ref_base()?;
        self.r.bit()?; // is_active
        self.r.bit()?; // layers inclusive
        self.uints("filter layers")?;
        self.r.bit()?; // entities inclusive
        let n = self.count(1, "filter entities")?;
        for _ in 0..n {
            self.entity_reference()?;
        }
        self.schema.skip_added_fields(FILTER, &mut self.r)?;
        self.user_data()
    }

    /// `PartDefinition` (311): its drawable items and its name.
    fn part_definition(&mut self) -> Result<(Vec<Item>, Option<String>), PrcError> {
        self.expect_type(PART_DEFINITION)?;
        let (hidden, own) = self.base_with_graphics()?;
        let name = self.name.clone();
        self.vector3()?; // bounding box
        self.vector3()?;
        let n = self.count(1, "representation items")?;
        let mut items = Vec::new();
        let at = Item { local: Vec::new(), tessellation: 0, graphics: vec![own], uids: Vec::new(), brep: None };
        for _ in 0..n {
            self.representation_item(&at, &mut items, 0)?;
        }
        self.markup_data()?;
        self.views()?;
        self.schema.skip_added_fields(PART_DEFINITION, &mut self.r)?;
        self.user_data()?;
        if hidden {
            items.clear();
        }
        Ok((items, name))
    }

    /// `FileIdentifier` [WD 7.3.10.2.2]: `None` for the same structure.
    fn file_identifier(&mut self) -> Result<Option<UniqueId>, PrcError> {
        if self.r.bit()? { Ok(None) } else { self.unique_id().map(Some) }
    }

    /// `ProductOccurrence` (310) [WD 7.3.10].
    fn product_occurrence(&mut self) -> Result<Product, PrcError> {
        self.expect_type(PRODUCT_OCCURRENCE)?;
        let (hidden, graphics) = self.base_with_graphics()?;
        let mut p = Product { hidden, graphics, name: self.name.clone(), part: self.r.unsigned_integer()?, ..Product::default() };
        p.prototype = self.r.unsigned_integer()?;
        if p.prototype != 0 {
            p.prototype_fs = self.file_identifier()?;
        }
        p.external = self.r.unsigned_integer()?;
        if p.external != 0 {
            p.external_fs = self.file_identifier()?;
        }
        p.sons = self.uints("son occurrences")?;
        p.suppressed = self.r.character()? & SUPPRESSED != 0;
        // ProductInformation [WD 7.3.10.3]: unit flag, unit, load status, information.
        self.r.bit()?;
        self.r.double()?;
        self.r.character()?;
        self.r.integer()?;
        if self.r.bit()? {
            p.location = Some(self.transformation()?);
        }
        let n = self.count(1, "entity references")?;
        for _ in 0..n {
            p.overrides.extend(self.entity_reference()?);
        }
        self.markup_data()?;
        self.views()?;
        if self.r.bit()? {
            self.filter()?;
        }
        let n = self.count(1, "display filters")?;
        for _ in 0..n {
            self.filter()?;
        }
        let n = self.count(1, "scene display parameters")?;
        for _ in 0..n {
            self.scene_display_parameters()?;
        }
        self.schema.skip_added_fields(PRODUCT_OCCURRENCE, &mut self.r)?;
        self.user_data()?;
        Ok(p)
    }

    /// `FileStructureTree` (304) [WD 7.3.6], with its `FileStructure` (302) inside.
    pub(super) fn file_structure_tree(&mut self) -> Result<Tree, PrcError> {
        self.expect_type(FILE_STRUCTURE_TREE)?;
        self.content_prc_base()?;
        let n = self.count(1, "part definitions")?;
        let mut tree = Tree::default();
        for _ in 0..n {
            let (items, name) = self.part_definition()?;
            tree.parts.push(items);
            tree.part_names.push(name);
        }
        let n = self.count(1, "product occurrences")?;
        for _ in 0..n {
            tree.products.push(self.product_occurrence()?);
        }
        self.expect_type(FILE_STRUCTURE)?;
        self.content_prc_base()?;
        self.r.unsigned_integer()?; // next_available_index
        self.r.unsigned_integer()?; // product_occurrence_index + 1 (the structure's root)
        self.schema.skip_added_fields(FILE_STRUCTURE, &mut self.r)?;
        self.schema.skip_added_fields(FILE_STRUCTURE_TREE, &mut self.r)?;
        self.user_data()?;
        Ok(tree)
    }

    /// `ModelFile` (301) after its schema [WD 7.3.3; pdf-issues #724, #740]: `units_in_mm` is
    /// always present.
    pub(super) fn model_file(&mut self) -> Result<ModelFile, PrcError> {
        self.expect_type(MODEL_FILE)?;
        self.content_prc_base()?;
        let units_from_cad_file = self.r.bit()?;
        let units_in_mm = self.r.double()?;
        let n = self.count(6, "root occurrences")?;
        let mut roots = Vec::with_capacity(n);
        for _ in 0..n {
            let id = self.unique_id()?;
            let index = self.r.unsigned_integer()?;
            self.r.bit()?; // is_active
            roots.push((id, index));
        }
        Ok(ModelFile { roots, units_in_mm, units_from_cad_file })
    }
}
