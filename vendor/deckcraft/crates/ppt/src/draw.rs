//! OfficeArt drawings ([MS-ODRAW]) as `.ppt` stores them: a tree of shape group containers and
//! shape containers, each shape with its `FSP` (type, id, flags), property tables (`FOPT`),
//! anchor and client records (placeholder, text).

use std::collections::HashMap;

use crate::doc::rt;
use crate::record::{MAX_DEPTH, Rec};

/// Property ids ([MS-ODRAW] §2.3).
pub mod prop {
    pub const ROTATION: u16 = 0x0004;
    pub const TEXT_LEFT: u16 = 0x0081;
    pub const TEXT_TOP: u16 = 0x0082;
    pub const TEXT_RIGHT: u16 = 0x0083;
    pub const TEXT_BOTTOM: u16 = 0x0084;
    pub const WRAP_TEXT: u16 = 0x0085;
    pub const ANCHOR_TEXT: u16 = 0x0087;
    pub const CROP_TOP: u16 = 0x0100;
    pub const CROP_BOTTOM: u16 = 0x0101;
    pub const CROP_LEFT: u16 = 0x0102;
    pub const CROP_RIGHT: u16 = 0x0103;
    pub const PIB: u16 = 0x0104;
    pub const GEO_LEFT: u16 = 0x0140;
    pub const GEO_TOP: u16 = 0x0141;
    pub const GEO_RIGHT: u16 = 0x0142;
    pub const GEO_BOTTOM: u16 = 0x0143;
    pub const VERTICES: u16 = 0x0145;
    pub const SEGMENT_INFO: u16 = 0x0146;
    pub const ADJUST_VALUE: u16 = 0x0147;
    pub const FILL_TYPE: u16 = 0x0180;
    pub const FILL_COLOR: u16 = 0x0181;
    pub const FILL_OPACITY: u16 = 0x0182;
    pub const FILL_BACK_COLOR: u16 = 0x0183;
    pub const FILL_BACK_OPACITY: u16 = 0x0184;
    pub const FILL_BLIP: u16 = 0x0186;
    pub const FILL_ANGLE: u16 = 0x018B;
    pub const FILL_FOCUS: u16 = 0x018C;
    pub const FILL_BOOLS: u16 = 0x01BF;
    pub const LINE_COLOR: u16 = 0x01C0;
    pub const LINE_OPACITY: u16 = 0x01C1;
    pub const LINE_WIDTH: u16 = 0x01CB;
    pub const LINE_STYLE: u16 = 0x01CD;
    pub const LINE_DASHING: u16 = 0x01CE;
    pub const LINE_START_ARROW: u16 = 0x01D0;
    pub const LINE_END_ARROW: u16 = 0x01D1;
    pub const LINE_START_ARROW_WIDTH: u16 = 0x01D2;
    pub const LINE_START_ARROW_LENGTH: u16 = 0x01D3;
    pub const LINE_END_ARROW_WIDTH: u16 = 0x01D4;
    pub const LINE_END_ARROW_LENGTH: u16 = 0x01D5;
    pub const LINE_JOIN: u16 = 0x01D6;
    pub const LINE_CAP: u16 = 0x01D7;
    pub const LINE_BOOLS: u16 = 0x01FF;
    pub const SHADOW_COLOR: u16 = 0x0201;
    pub const SHADOW_OPACITY: u16 = 0x0204;
    pub const SHADOW_OFFSET_X: u16 = 0x0205;
    pub const SHADOW_OFFSET_Y: u16 = 0x0206;
    pub const SHADOW_BOOLS: u16 = 0x023F;
    pub const HSP_MASTER: u16 = 0x0301;
    pub const NAME: u16 = 0x0380;
    pub const DESCRIPTION: u16 = 0x0381;
    pub const GROUP_BOOLS: u16 = 0x03BF;
}

/// One property value: the 32-bit operand and, for complex properties, the extra bytes.
#[derive(Clone, Copy, Debug)]
pub struct PropVal<'a> {
    pub op: u32,
    pub complex: Option<&'a [u8]>,
}

/// A shape's merged property tables (primary first; secondary and tertiary fill gaps).
#[derive(Clone, Debug, Default)]
pub struct Props<'a> {
    map: HashMap<u16, PropVal<'a>>,
}

impl<'a> Props<'a> {
    /// Read one `FOPT` body: `count` 6-byte entries then the complex data in entry order. Entries
    /// already present are kept (the caller adds the most specific table first).
    pub fn add_table(&mut self, r: &Rec<'a>) {
        let count = usize::from(r.instance);
        let mut complex_at = count.saturating_mul(6);
        for i in 0..count {
            let (Some(id), Some(op)) = (r.u16(i * 6), r.u32(i * 6 + 2)) else { break };
            let is_complex = id & 0x8000 != 0;
            let pid = id & 0x3FFF;
            let complex = if is_complex {
                let len = op as usize;
                let data = r.data.get(complex_at..complex_at.saturating_add(len).min(r.data.len()));
                complex_at = complex_at.saturating_add(len);
                data
            } else {
                None
            };
            self.fill_gap(pid, PropVal { op, complex });
        }
    }

    /// Fill gaps from another set (a master shape's or the drawing defaults).
    pub fn inherit(&mut self, other: &Props<'a>) {
        for (k, v) in &other.map {
            self.fill_gap(*k, *v);
        }
    }

    /// Add `v` for `id` where this set doesn't say it already. A boolean property word ([MS-ODRAW]:
    /// the last id of each 64-id group) holds sixteen flags, each with its own `fUse` bit 16
    /// places up, so it is merged flag by flag: a shape that sets only `fNoFillHitTest` still
    /// inherits `fFilled` from its master or the defaults.
    fn fill_gap(&mut self, id: u16, v: PropVal<'a>) {
        match self.map.get_mut(&id) {
            None => {
                self.map.insert(id, v);
            }
            Some(have) if id & 0x3F == 0x3F => {
                let unset = !(have.op >> 16) & 0xFFFF;
                let take = (v.op >> 16) & unset;
                have.op |= take << 16;
                have.op = (have.op & !take) | (v.op & take);
            }
            Some(_) => {}
        }
    }

    /// Set a simple (non-complex) property.
    pub fn set(&mut self, id: u16, op: u32) {
        self.map.insert(id, PropVal { op, complex: None });
    }

    pub fn get(&self, id: u16) -> Option<u32> {
        self.map.get(&id).map(|v| v.op)
    }

    pub fn complex(&self, id: u16) -> Option<&'a [u8]> {
        self.map.get(&id).and_then(|v| v.complex)
    }

    /// A 16.16 fixed-point value.
    pub fn fixed(&self, id: u16) -> Option<f64> {
        self.get(id).map(|v| f64::from(v as i32) / 65536.0)
    }

    /// A boolean from a bool-property word: bit `bit` holds the value, bit `bit + 16` says it is set.
    pub fn flag(&self, id: u16, bit: u32) -> Option<bool> {
        let v = self.get(id)?;
        (v & (1 << (bit + 16)) != 0).then_some(v & (1 << bit) != 0)
    }

    /// A UTF-16 string property (names, alt text).
    pub fn string(&self, id: u16) -> Option<String> {
        let b = self.complex(id)?;
        let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
        Some(String::from_utf16_lossy(&units)).filter(|s| !s.is_empty())
    }

    /// An `IMsoArray` property: element size and the elements' bytes.
    pub fn array(&self, id: u16) -> Option<(usize, &'a [u8])> {
        let b = self.complex(id)?;
        let n = usize::from(crate::cfb::u16_at(b, 0)?);
        let size = crate::cfb::u16_at(b, 4)?;
        // 0xFFF0 marks 4-byte elements stored as two 2-byte halves.
        let size = if size == 0xFFF0 { 4 } else { usize::from(size) };
        if size == 0 {
            return None;
        }
        let body = b.get(6..)?;
        let len = n.saturating_mul(size).min(body.len() / size * size);
        Some((size, body.get(..len)?))
    }
}

/// An anchor rectangle in its own coordinate units: master units for a client anchor, the parent
/// group's units for a child anchor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Anchor {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

/// The placeholder a shape stands for (`OEPlaceholderAtom`).
#[derive(Clone, Copy, Debug)]
pub struct Placeholder {
    pub position: u32,
    /// `PlaceholderEnum`.
    pub kind: u8,
}

/// One shape of a drawing, before conversion.
#[derive(Clone, Debug)]
pub struct RawShape<'a> {
    /// `MSOSPT` shape type.
    pub shape_type: u16,
    pub spid: u32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub patriarch: bool,
    pub deleted: bool,
    pub props: Props<'a>,
    pub anchor: Option<Anchor>,
    /// For a group: the child coordinate rectangle (`FSPGR`).
    pub group_rect: Option<Anchor>,
    pub children: Vec<RawShape<'a>>,
    pub is_group: bool,
    pub placeholder: Option<Placeholder>,
    /// The records of the shape's text box (`OfficeArtClientTextbox`).
    pub text: Option<Rec<'a>>,
}

/// The top-level shapes and the background shape of a `DrawingContainer`.
pub struct Drawing<'a> {
    pub shapes: Vec<RawShape<'a>>,
    pub background: Option<RawShape<'a>>,
}

/// Read the drawing of a slide, master or notes container.
pub fn drawing<'a>(container: &Rec<'a>) -> Drawing<'a> {
    let mut out = Drawing { shapes: vec![], background: None };
    let Some(dg) = container.child(rt::DRAWING).and_then(|d| d.child(rt::DG_CONTAINER)) else { return out };
    // The background is the container's own shape (`OfficeArtDgContainer.shape`, [MS-ODRAW]
    // 2.2.13), not one found by its fBackground flag.
    if let Some(bg) = dg.child(rt::SP_CONTAINER) {
        out.background = Some(shape(&bg));
    }
    if let Some(top) = dg.child(rt::SPGR_CONTAINER) {
        // The first shape of the top group is the patriarch, standing for the drawing itself.
        out.shapes = group_children(&top, 0).into_iter().filter(|s| !s.patriarch).collect();
    }
    out
}

fn group_children<'a>(gr: &Rec<'a>, depth: usize) -> Vec<RawShape<'a>> {
    if depth > MAX_DEPTH {
        return vec![];
    }
    let mut out = vec![];
    for r in gr.children() {
        match r.kind {
            rt::SP_CONTAINER => out.push(shape(&r)),
            rt::SPGR_CONTAINER => {
                let mut kids = r.children();
                // A group container's first shape describes the group itself.
                let Some(head) = kids.next().filter(|h| h.kind == rt::SP_CONTAINER) else { continue };
                let mut g = shape(&head);
                g.is_group = true;
                g.children = group_children(&r, depth + 1).into_iter().skip(1).collect();
                out.push(g);
            }
            _ => {}
        }
    }
    out
}

fn rect_i32(r: &Rec) -> Option<Anchor> {
    Some(Anchor { left: f64::from(r.i32(0)?), top: f64::from(r.i32(4)?), right: f64::from(r.i32(8)?), bottom: f64::from(r.i32(12)?) })
}

fn shape<'a>(sp: &Rec<'a>) -> RawShape<'a> {
    let mut s = RawShape {
        shape_type: 0,
        spid: 0,
        flip_h: false,
        flip_v: false,
        patriarch: false,
        deleted: false,
        props: Props::default(),
        anchor: None,
        group_rect: None,
        children: vec![],
        is_group: false,
        placeholder: None,
        text: None,
    };
    // Tables in specificity order: primary, then secondary and tertiary.
    let mut tables: Vec<Rec> = vec![];
    for r in sp.children() {
        match r.kind {
            rt::FSP => {
                s.shape_type = r.instance;
                s.spid = r.u32(0).unwrap_or(0);
                let f = r.u32(4).unwrap_or(0);
                s.patriarch = f & 0x4 != 0;
                s.deleted = f & 0x8 != 0;
                s.flip_h = f & 0x40 != 0;
                s.flip_v = f & 0x80 != 0;
            }
            rt::FOPT => tables.insert(0, r),
            rt::SECONDARY_FOPT | rt::TERTIARY_FOPT => tables.push(r),
            rt::FSPGR => s.group_rect = rect_i32(&r),
            rt::CHILD_ANCHOR => s.anchor = rect_i32(&r),
            rt::CLIENT_ANCHOR => {
                s.anchor = if r.data.len() >= 16 {
                    // RectStruct: top, left, right, bottom.
                    (|| {
                        Some(Anchor {
                            top: f64::from(r.i32(0)?),
                            left: f64::from(r.i32(4)?),
                            right: f64::from(r.i32(8)?),
                            bottom: f64::from(r.i32(12)?),
                        })
                    })()
                } else {
                    // SmallRectStruct: top, left, right, bottom.
                    (|| {
                        Some(Anchor {
                            top: f64::from(r.i16(0)?),
                            left: f64::from(r.i16(2)?),
                            right: f64::from(r.i16(4)?),
                            bottom: f64::from(r.i16(6)?),
                        })
                    })()
                };
            }
            rt::CLIENT_DATA => {
                if let Some(p) = r.child(rt::PLACEHOLDER_ATOM) {
                    s.placeholder = Some(Placeholder { position: p.u32(0).unwrap_or(0), kind: p.u8(4).unwrap_or(0) });
                }
            }
            rt::CLIENT_TEXTBOX => s.text = Some(r),
            _ => {}
        }
    }
    for t in &tables {
        s.props.add_table(t);
    }
    s
}

/// The drawing-wide default properties (`OfficeArtDggContainer`'s `FOPT`).
pub fn default_props<'a>(dgg: Option<Rec<'a>>) -> Props<'a> {
    let mut p = Props::default();
    if let Some(dgg) = dgg {
        for t in dgg.children().filter(|r| matches!(r.kind, rt::FOPT | rt::SECONDARY_FOPT | rt::TERTIARY_FOPT)) {
            p.add_table(&t);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::record::build::*;

    pub fn fopt(entries: &[(u16, u32)], complex: &[u8]) -> Vec<u8> {
        let mut b = vec![];
        for (id, op) in entries {
            b.extend_from_slice(&id.to_le_bytes());
            b.extend_from_slice(&op.to_le_bytes());
        }
        b.extend_from_slice(complex);
        atom(rt::FOPT, entries.len() as u16, &b)
    }

    #[test]
    fn reads_properties_and_complex_strings() {
        let name: Vec<u8> = "Logo\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let t = fopt(&[(prop::FILL_COLOR, 0x0011_2233), (0x8000 | prop::NAME, name.len() as u32), (prop::FILL_BOOLS, 0x0010_0000)], &name);
        let mut p = Props::default();
        p.add_table(&Rec::at(&t, 0).unwrap());
        assert_eq!(p.get(prop::FILL_COLOR), Some(0x0011_2233));
        assert_eq!(p.string(prop::NAME).as_deref(), Some("Logo"));
        assert_eq!(p.flag(prop::FILL_BOOLS, 4), Some(false));
        assert_eq!(p.flag(prop::LINE_BOOLS, 3), None);
    }

    /// A boolean word inherits the flags it doesn't set (#1137 review): a text box that sets only
    /// `fNoFillHitTest` (bit 4 is `fFilled`, bit 0 `fNoFillHitTest`) still gets "not filled"
    /// from below, and its own flags win over the inherited ones.
    #[test]
    fn boolean_properties_merge_flag_by_flag() {
        let mut p = Props::default();
        p.set(prop::FILL_BOOLS, 0x0001_0001); // fNoFillHitTest = true; fFilled not said
        p.set(prop::LINE_BOOLS, 0x0008_0008); // fLine = true
        let mut below = Props::default();
        below.set(prop::FILL_BOOLS, 0x0011_0000); // fFilled = false, fNoFillHitTest = false
        below.set(prop::LINE_BOOLS, 0x0008_0000); // fLine = false
        p.inherit(&below);
        assert_eq!(p.flag(prop::FILL_BOOLS, 4), Some(false));
        assert_eq!(p.flag(prop::FILL_BOOLS, 0), Some(true));
        assert_eq!(p.flag(prop::LINE_BOOLS, 3), Some(true));
        // The same merge applies between a shape's own tables.
        let t = fopt(&[(prop::FILL_BOOLS, 0x0001_0001)], &[]);
        let t2 = fopt(&[(prop::FILL_BOOLS, 0x0010_0010)], &[]);
        let mut q = Props::default();
        q.add_table(&Rec::at(&t, 0).unwrap());
        q.add_table(&Rec::at(&t2, 0).unwrap());
        assert_eq!(q.flag(prop::FILL_BOOLS, 4), Some(true));
        assert_eq!(q.flag(prop::FILL_BOOLS, 0), Some(true));
        // Non-boolean properties still keep the first value.
        let mut r = Props::default();
        r.set(prop::FILL_COLOR, 1);
        let mut s = Props::default();
        s.set(prop::FILL_COLOR, 2);
        r.inherit(&s);
        assert_eq!(r.get(prop::FILL_COLOR), Some(1));
    }

    #[test]
    fn a_complex_length_past_the_record_is_clipped() {
        let t = fopt(&[(0x8000 | prop::NAME, 0xFFFF_FFFF), (0x8000 | prop::DESCRIPTION, 10)], b"A\0");
        let mut p = Props::default();
        p.add_table(&Rec::at(&t, 0).unwrap());
        assert_eq!(p.string(prop::NAME).as_deref(), Some("A"));
        assert_eq!(p.complex(prop::DESCRIPTION), None);
    }

    #[test]
    fn reads_groups_anchors_and_placeholders() {
        let fsp = |t: u16, id: u32, flags: u32| atom(rt::FSP, t, &[id.to_le_bytes(), flags.to_le_bytes()].concat());
        let small =
            |t: i16, l: i16, r: i16, b: i16| atom(rt::CLIENT_ANCHOR, 0, &[t, l, r, b].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
        let child =
            |l: i32, t: i32, r: i32, b: i32| atom(rt::CHILD_ANCHOR, 0, &[l, t, r, b].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
        let ph = container(rt::CLIENT_DATA, 0, &[atom(rt::PLACEHOLDER_ATOM, 0, &[0, 0, 0, 0, 0x0D, 0, 0, 0])]);
        let patriarch = container(rt::SP_CONTAINER, 0, &[atom(rt::FSPGR, 1, &[0; 16]), fsp(0, 1024, 0x5)]);
        let title = container(rt::SP_CONTAINER, 0, &[fsp(1, 1025, 0xA00), small(10, 20, 300, 80), ph]);
        let group = container(
            rt::SPGR_CONTAINER,
            0,
            &[
                container(
                    rt::SP_CONTAINER,
                    0,
                    &[
                        atom(rt::FSPGR, 1, &[0i32, 0, 100, 100].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()),
                        fsp(0, 1026, 0x201),
                        small(100, 100, 200, 200),
                    ],
                ),
                container(rt::SP_CONTAINER, 0, &[fsp(3, 1027, 0x40), child(0, 0, 50, 50)]),
            ],
        );
        let drawing = container(rt::DRAWING, 0, &[container(rt::DG_CONTAINER, 0, &[container(rt::SPGR_CONTAINER, 0, &[patriarch, title, group])])]);
        let slide = container(rt::SLIDE, 0, &[drawing]);
        let d = super::drawing(&Rec::at(&slide, 0).unwrap());
        assert_eq!(d.shapes.len(), 2);
        let t = &d.shapes[0];
        assert_eq!(t.placeholder.unwrap().kind, 0x0D);
        assert_eq!(t.anchor, Some(Anchor { left: 20.0, top: 10.0, right: 300.0, bottom: 80.0 }));
        let g = &d.shapes[1];
        assert!(g.is_group);
        assert_eq!(g.children.len(), 1);
        assert!(g.children[0].flip_h);
        assert_eq!(g.group_rect.unwrap().right, 100.0);
    }
}
