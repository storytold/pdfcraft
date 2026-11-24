//! Building the DeckCraft model from the parsed `.ppt` structure.
//!
//! Each main master becomes a DeckCraft master (with the standard layouts, its own colours,
//! fonts, text styles, background and decorative shapes); a title master becomes a layout of its
//! main master. Slide shapes keep their own boxes, and their text carries its formatting resolved
//! against the master's text styles, so it draws the same whatever DeckCraft's layouts hold.

use std::collections::HashMap;
use std::sync::Arc;

use deckcraft_color::{ColorTransform, Rgba, SchemeSlot};
use deckcraft_model::style::{Dash, LineCap, LineEnd, LineJoin, PictureFill, PictureMode, Shadow};
use deckcraft_model::text::{Align, Anchor as TextAnchor, AutoFit, BodyProps, Bullet, LevelStyle, ListStyle, ParaProps, RunKind, Spacing, TabStop};
use deckcraft_model::{
    Background, ColorRef, Effects, Fill, Layout, LayoutId, LayoutType, Line, MediaId, Paragraph, PhType, Placeholder, Presentation, Run, RunProps,
    Shape, ShapeId, ShapeKind, Size, Slide, SlideId, TextBody, Theme, Xfrm, defaults,
};

use crate::doc::{self, Doc, Listed, rt};
use crate::draw::{self, Props, RawShape, prop};
use crate::geom;
use crate::record::{MAX_DEPTH, Rec};
use crate::text::{self, Cf, FieldKind, Pf, StyleSheet, TextBlock, TextColor, ty};

/// Master units (576 per inch) per point.
const MU: f64 = 8.0;
/// EMUs per point.
const EMU: f64 = 12700.0;

/// The eight colours of a `.ppt` colour scheme: background, text, shadow, title text, fill,
/// accent, accent and hyperlink, accent and followed hyperlink.
type Scheme = [Rgba; 8];

const DEFAULT_SCHEME: Scheme = [
    Rgba::rgb(0xFF, 0xFF, 0xFF),
    Rgba::rgb(0, 0, 0),
    Rgba::rgb(0x80, 0x80, 0x80),
    Rgba::rgb(0, 0, 0),
    Rgba::rgb(0xBB, 0xE0, 0xE3),
    Rgba::rgb(0x33, 0x33, 0x99),
    Rgba::rgb(0, 0x99, 0x99),
    Rgba::rgb(0x99, 0xCC, 0x00),
];

fn scheme_of(container: &Rec) -> Option<Scheme> {
    let r = container.children_of(rt::COLOR_SCHEME).find(|r| r.instance == 1)?;
    let mut s = DEFAULT_SCHEME;
    for (i, c) in s.iter_mut().enumerate() {
        *c = Rgba::rgb(r.u8(i * 4)?, r.u8(i * 4 + 1)?, r.u8(i * 4 + 2)?);
    }
    Some(s)
}

/// What converting one drawing needs besides the shapes.
struct Scope<'s, 'a> {
    scheme: Scheme,
    styles: &'s StyleSheet,
    /// The slide list's outline text, for placeholders that point at it.
    outline: &'s [crate::record::Records<'a>],
    /// Each outline block's numbering (`StyleTextProp9Atom` entries), by outline index.
    numbering: &'s [Option<Vec<text::Pf9>>],
    master: bool,
    /// Where `hspMaster` shape ids are looked up: the slide's master, then a title master's main
    /// master.
    masters: Chain,
}

/// Master slide ids, nearest first.
type Chain = [Option<u32>; 2];

struct Ctx<'a> {
    p: Presentation,
    fonts: Vec<String>,
    blips: crate::blip::Store<'a>,
    media: HashMap<usize, Option<MediaId>>,
    /// Pictures held in a shape's own property, by where their bytes start, so a master shape's
    /// picture inherited by many slides is added once.
    inline_media: HashMap<usize, Option<MediaId>>,
    /// The slides' outline numbering, by slide id and text type ([`doc::outline_numbering`]).
    numbering: HashMap<(u32, u32), Vec<Vec<text::Pf9>>>,
    defaults: Props<'a>,
    /// Master shapes by master slide id and shape id, for slide shapes that inherit from one
    /// (`hspMaster`). Keyed by master too, so a shape id repeated in another master can't replace
    /// this master's shape.
    master_props: HashMap<(u32, u32), Props<'a>>,
}

pub fn convert<'a>(d: &Doc<'a>, pictures: &'a [u8]) -> Presentation {
    let atom = d.document_atom();
    let side =
        |v: Option<i32>, fallback: f64| v.map(|v| f64::from(v) / MU).filter(|v| v.is_finite() && *v >= 1.0 && *v <= 56.0 * 72.0).unwrap_or(fallback);
    let size = Size::new(side(atom.as_ref().map(|a| a.slide_w), 720.0), side(atom.as_ref().map(|a| a.slide_h), 540.0));
    let mut p = defaults::blank_presentation(size, Theme::default(), false);
    p.masters.clear();
    p.notes_size = Size::new(side(atom.as_ref().map(|a| a.notes_w), 540.0), side(atom.as_ref().map(|a| a.notes_h), 720.0));
    p.first_slide_number = atom.as_ref().map_or(1, |a| u32::from(a.first_slide_number));
    let dgg = d.document.child(rt::DRAWING_GROUP).and_then(|g| g.child(rt::DGG_CONTAINER));
    let mut cx = Ctx {
        p,
        fonts: doc::fonts(d),
        blips: crate::blip::Store::new(dgg, pictures),
        media: HashMap::new(),
        inline_media: HashMap::new(),
        numbering: doc::outline_numbering(&d.document),
        defaults: draw::default_props(dgg),
        master_props: HashMap::new(),
    };
    let environment = d.document.child(rt::ENVIRONMENT);
    if let Some(hf) = doc::headers_footers(&d.document) {
        cx.p.header_footer = deckcraft_model::HeaderFooter {
            date: hf.date,
            slide_number: hf.slide_number,
            footer: hf.footer,
            footer_text: hf.footer_text,
            date_text: if hf.user_date { hf.date_text } else { String::new() },
            date_format: format!("datetime{}", hf.format.min(12) + 1),
            ..Default::default()
        };
    }

    // Main masters first, then title masters as layouts of theirs.
    let mut masters: HashMap<u32, MasterInfo> = HashMap::new();
    for m in d.masters.iter().filter(|m| m.rec.kind == rt::MAIN_MASTER) {
        if let Some(info) = cx.main_master(m, environment, size) {
            masters.insert(m.slide_id, info);
        }
    }
    if cx.p.masters.is_empty() {
        let (m, next) = defaults::build_master(size, Theme::default(), cx.p.next_id.max(1));
        cx.p.next_id = next;
        cx.p.masters.push(Arc::new(m));
    }
    for m in d.masters.iter().filter(|m| m.rec.kind == rt::SLIDE) {
        cx.title_master(m, &mut masters);
    }

    // Each slide's notes page, by the slide id its NotesAtom names (the first one wins), built once:
    // a scan per slide would be slides × notes pages.
    let mut notes_of: HashMap<u32, usize> = HashMap::new();
    for (i, n) in d.notes.iter().enumerate() {
        if n.rec.kind == rt::NOTES
            && let Some(id) = n.rec.child(rt::NOTES_ATOM).and_then(|a| a.u32(0))
        {
            notes_of.entry(id).or_insert(i);
        }
    }
    for s in &d.slides {
        if s.rec.kind != rt::SLIDE {
            continue;
        }
        if let Some(slide) = cx.slide(d, s, &masters, &notes_of) {
            cx.p.slides.push(Arc::new(slide));
        }
    }
    cx.p
}

/// A converted master: where it went and what its slides inherit.
#[derive(Clone)]
struct MasterInfo {
    index: usize,
    scheme: Scheme,
    styles: Arc<StyleSheet>,
    /// A layout made for a title master; slides of that title master use it.
    layout: Option<LayoutId>,
    /// This master's slide id, then its main master's for a title master.
    ids: Chain,
}

impl<'a> Ctx<'a> {
    fn alloc(&mut self) -> Option<u32> {
        self.p.alloc_id()
    }

    fn main_master(&mut self, m: &Listed<'a>, environment: Option<Rec<'a>>, size: Size) -> Option<MasterInfo> {
        let scheme = scheme_of(&m.rec).unwrap_or(DEFAULT_SCHEME);
        let styles = Arc::new(StyleSheet::new(environment, Some(&m.rec)));
        let theme = self.theme(&scheme, &styles);
        let (mut master, next) = defaults::build_master(size, theme, self.p.next_id.max(1));
        self.p.next_id = next;
        let drawing = draw::drawing(&m.rec);
        self.remember_master_props(m.slide_id, &drawing.shapes, 0);
        let ids = [Some(m.slide_id), None];
        let scope = Scope { scheme, styles: &styles, outline: &m.outline, numbering: &[], master: true, masters: ids };
        master.background = drawing.background.as_ref().and_then(|b| self.background(b, &scope));
        for raw in &drawing.shapes {
            if let Some(ph) = raw.placeholder.filter(|p| p.kind != 0) {
                // The master's placeholders set where new slides' placeholders go.
                if let (Some(kind), Some(x)) = (ph_type(ph.kind), raw.anchor.map(|a| xfrm(&a, TOP, 0.0, false, false)))
                    && let Some(s) = master.shapes.iter_mut().find(|s| s.ph_type().is_some_and(|t| same_family(t, kind)))
                {
                    s.xfrm = Some(x);
                    // The footer row sits in the same place on every layout, with the master's look:
                    // its first paragraph's alignment and its first run's size and colour.
                    if matches!(kind, PhType::Date | PhType::Footer | PhType::SlideNum) {
                        let look = self.shape(raw, &scope, 0, TOP).and_then(|c| c.text).and_then(|t| t.paragraphs.into_iter().next());
                        let style = |t: &mut TextBody| {
                            if let Some(p) = &look {
                                let mut level = t.list_style.level(0).cloned().unwrap_or_default();
                                level.para.align = p.props.align.or(level.para.align);
                                if let Some(run) = p.runs.first() {
                                    level.run.size = run.props.size.or(level.run.size);
                                    level.run.fill = run.props.fill.clone().or(level.run.fill.take());
                                }
                                t.list_style.set(0, level);
                            }
                        };
                        if let Some(t) = s.text.as_mut() {
                            style(t);
                        }
                        for l in &mut master.layouts {
                            for ls in l.shapes.iter_mut().filter(|ls| ls.ph_type() == Some(kind)) {
                                ls.xfrm = Some(x);
                                if let Some(t) = ls.text.as_mut() {
                                    style(t);
                                }
                            }
                        }
                    }
                }
                continue;
            }
            if let Some(s) = self.shape(raw, &scope, 0, TOP) {
                master.shapes.push(s);
            }
        }
        master.title_style = self.list_style(&styles, ty::TITLE, &scheme);
        master.body_style = self.list_style(&styles, ty::BODY, &scheme);
        master.other_style = self.list_style(&styles, ty::OTHER, &scheme);
        let index = self.p.masters.len();
        self.p.masters.push(Arc::new(master));
        Some(MasterInfo { index, scheme, styles, layout: None, ids })
    }

    /// A title master: a layout of its main master holding its background and shapes.
    fn title_master(&mut self, m: &Listed<'a>, masters: &mut HashMap<u32, MasterInfo>) {
        let atom = doc::slide_atom(&m.rec);
        let Some(parent) = masters.get(&atom.master_id).cloned() else { return };
        let scheme = if atom.follow_master_scheme { parent.scheme } else { scheme_of(&m.rec).unwrap_or(parent.scheme) };
        let Some(id) = self.alloc() else { return };
        let drawing = draw::drawing(&m.rec);
        self.remember_master_props(m.slide_id, &drawing.shapes, 0);
        let ids = [Some(m.slide_id), parent.ids[0]];
        let scope = Scope { scheme, styles: &parent.styles, outline: &m.outline, numbering: &[], master: true, masters: ids };
        let background = if atom.follow_master_background { None } else { drawing.background.as_ref().and_then(|b| self.background(b, &scope)) };
        let shapes: Vec<Shape> =
            drawing.shapes.iter().filter(|r| r.placeholder.is_none_or(|p| p.kind == 0)).filter_map(|r| self.shape(r, &scope, 0, TOP)).collect();
        // Its placeholders follow the main master's title layout, with ids of their own.
        let base = self
            .p
            .masters
            .get(parent.index)
            .and_then(|m| m.layouts.iter().find(|l| l.kind == LayoutType::Title))
            .map(|l| l.shapes.clone())
            .unwrap_or_default();
        let mut placeholders = Vec::with_capacity(base.len());
        for mut s in base {
            let Some(id) = self.alloc() else { break };
            s.id = ShapeId(id);
            placeholders.push(s);
        }
        let Some(master) = self.p.masters.get_mut(parent.index) else { return };
        Arc::make_mut(master).layouts.push(Layout {
            id: LayoutId(id),
            name: "Title Master".into(),
            kind: LayoutType::Title,
            shapes: shapes.into_iter().chain(placeholders).collect(),
            background,
            show_master_shapes: atom.follow_master_objects,
            ..Default::default()
        });
        masters.insert(m.slide_id, MasterInfo { layout: Some(LayoutId(id)), scheme, ids, ..parent });
    }

    fn remember_master_props(&mut self, master: u32, shapes: &[RawShape<'a>], depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for s in shapes {
            self.master_props.insert((master, s.spid), s.props.clone());
            self.remember_master_props(master, &s.children, depth + 1);
        }
    }

    /// A theme carrying the master's colours and fonts, so shapes added later match the deck.
    fn theme(&self, scheme: &Scheme, styles: &StyleSheet) -> Theme {
        let mut t = Theme { name: "Imported".into(), ..Default::default() };
        for (slot, i) in [
            (SchemeSlot::Lt1, 0),
            (SchemeSlot::Dk1, 1),
            (SchemeSlot::Lt2, 2),
            (SchemeSlot::Dk2, 3),
            (SchemeSlot::Accent1, 4),
            (SchemeSlot::Accent2, 5),
            (SchemeSlot::Hlink, 6),
            (SchemeSlot::FolHlink, 7),
        ] {
            if let Some(c) = scheme.get(i) {
                t.colors.set(slot, *c);
            }
        }
        if let Some(f) = styles.resolve(ty::TITLE, 0).1.font.and_then(|i| self.fonts.get(usize::from(i))) {
            t.fonts.major.latin = f.clone();
        }
        if let Some(f) = styles.resolve(ty::BODY, 0).1.font.and_then(|i| self.fonts.get(usize::from(i))) {
            t.fonts.minor.latin = f.clone();
        }
        t
    }

    fn list_style(&self, styles: &StyleSheet, text_type: u32, scheme: &Scheme) -> ListStyle {
        let mut ls = ListStyle::default();
        for level in 0..5u8 {
            let (pf, cf) = styles.resolve(text_type, level);
            let run = self.run_props(&cf, scheme);
            let para = self.para_props(&pf, scheme, run.size.unwrap_or(18.0));
            ls.set(level, LevelStyle { para, run });
        }
        ls
    }

    fn slide(&mut self, d: &Doc<'a>, s: &Listed<'a>, masters: &HashMap<u32, MasterInfo>, notes_of: &HashMap<u32, usize>) -> Option<Slide> {
        let atom = doc::slide_atom(&s.rec);
        // An unknown master falls back to the first main master. Title masters share their main
        // master's index, so including them would tie, and HashMap order would pick the winner.
        let first_main = || masters.values().filter(|m| m.layout.is_none()).min_by_key(|m| m.index);
        let info = masters.get(&atom.master_id).or_else(first_main).cloned();
        let (index, master_scheme, styles, title_layout, ids) = match &info {
            Some(m) => (m.index, m.scheme, m.styles.clone(), m.layout, m.ids),
            None => (0, DEFAULT_SCHEME, Arc::new(StyleSheet::new(d.document.child(rt::ENVIRONMENT), None)), None, [None; 2]),
        };
        let scheme = if atom.follow_master_scheme { master_scheme } else { scheme_of(&s.rec).unwrap_or(master_scheme) };
        let layout = title_layout.or_else(|| {
            let kind = layout_kind(atom.geom);
            let m = self.p.masters.get(index)?;
            m.layouts.iter().find(|l| l.kind == kind).or_else(|| m.layouts.first()).map(|l| l.id)
        })?;
        let id = SlideId(self.alloc()?);
        let drawing = draw::drawing(&s.rec);
        // The n-th outline block of a text type takes the n-th numbering entry for that type.
        let mut seen: HashMap<u32, usize> = HashMap::new();
        let numbering: Vec<Option<Vec<text::Pf9>>> = s
            .outline
            .iter()
            .map(|block| {
                let tt = block.clone().next().filter(|h| h.kind == rt::TEXT_HEADER).and_then(|h| h.u32(0))?;
                let k = seen.entry(tt).or_insert(0);
                let n = *k;
                *k += 1;
                self.numbering.get(&(s.slide_id, tt)).and_then(|v| v.get(n)).cloned()
            })
            .collect();
        let scope = Scope { scheme, styles: &styles, outline: &s.outline, numbering: &numbering, master: false, masters: ids };
        let background = if atom.follow_master_background { None } else { drawing.background.as_ref().and_then(|b| self.background(b, &scope)) };
        let mut shapes: Vec<Shape> = drawing.shapes.iter().filter_map(|r| self.shape(r, &scope, 0, TOP)).collect();
        // The slide's own headers and footers, else the document's for every slide.
        if let Some(hf) = doc::headers_footers(&s.rec).or_else(|| doc::headers_footers(&d.document)) {
            self.footers(&mut shapes, &hf);
        }
        let hidden = s.rec.child(rt::SLIDE_SHOW_SLIDE_INFO).and_then(|i| i.u16(10)).is_some_and(|f| f & 0x4 != 0);
        let notes = notes_of.get(&s.slide_id).and_then(|&i| d.notes.get(i)).map(|n| self.notes(n, &styles, scheme)).unwrap_or_default();
        Some(Slide { id, layout, shapes, background, hidden, show_master_shapes: atom.follow_master_objects, notes, ..Default::default() })
    }

    /// The date, footer and slide number placeholders `hf` turns on, for those the slide doesn't
    /// hold itself. They take their place and look from the layout and master.
    fn footers(&mut self, shapes: &mut Vec<Shape>, hf: &doc::HeadersFooters) {
        let field = |text: &str, field: String| TextBody {
            paragraphs: vec![Paragraph {
                runs: vec![Run { text: text.into(), props: Default::default(), kind: RunKind::Field { field } }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let wanted = [
            (PhType::Date, hf.date, "Date Placeholder"),
            (PhType::Footer, hf.footer && !hf.footer_text.is_empty(), "Footer Placeholder"),
            (PhType::SlideNum, hf.slide_number, "Slide Number Placeholder"),
        ];
        for (kind, on, name) in wanted {
            if !on || shapes.iter().any(|s| s.ph_type() == Some(kind)) {
                continue;
            }
            let text = match kind {
                PhType::SlideNum => field("‹#›", "slidenum".into()),
                PhType::Date if hf.user_date => TextBody::from_text(&hf.date_text),
                // formatId 0–12 are the thirteen date formats, in DrawingML's order.
                PhType::Date => field("", format!("datetime{}", hf.format.min(12) + 1)),
                _ => TextBody::from_text(&hf.footer_text),
            };
            let Some(id) = self.alloc() else { return };
            shapes.push(Shape {
                id: ShapeId(id),
                name: name.into(),
                ph: Some(Placeholder { kind, ..Default::default() }),
                text: Some(text),
                ..Default::default()
            });
        }
    }

    /// The text of a notes page's notes placeholder.
    fn notes(&mut self, n: &Listed<'a>, styles: &StyleSheet, scheme: Scheme) -> TextBody {
        let drawing = draw::drawing(&n.rec);
        // Notes masters aren't imported, so there are no master shapes to inherit from.
        let scope = Scope { scheme, styles, outline: &n.outline, numbering: &[], master: false, masters: [None; 2] };
        for raw in &drawing.shapes {
            let Some(block) = self.text_block(raw, &scope) else { continue };
            // The notes body placeholder; a slide number or header placeholder on the page isn't notes.
            let is_notes = match raw.placeholder.filter(|p| p.kind != 0) {
                Some(p) => matches!(p.kind, 0x06 | 0x0C),
                None => block.text_type == ty::NOTES,
            };
            if is_notes {
                let mut body = self.text_body(&block, &scope, &raw.props);
                body.body = BodyProps::default();
                return body;
            }
        }
        TextBody::default()
    }

    fn background(&mut self, raw: &RawShape<'a>, scope: &Scope) -> Option<Background> {
        let props = self.props_of(raw, scope, false);
        let fill = self.fill(&props, scope, true)?;
        Some(Background::Fill { fill })
    }

    /// The shape's properties with what it inherits: its master shape's, then (for placeholders,
    /// text boxes and picture frames) no fill and no line, then the drawing defaults.
    fn props_of(&self, raw: &RawShape<'a>, scope: &Scope, unfilled: bool) -> Props<'a> {
        let mut props = raw.props.clone();
        let inherited =
            props.get(prop::HSP_MASTER).and_then(|spid| scope.masters.iter().flatten().find_map(|&master| self.master_props.get(&(master, spid))));
        if let Some(m) = inherited {
            props.inherit(m);
        }
        if unfilled {
            props.inherit(&no_fill_no_line());
        }
        props.inherit(&self.defaults);
        props
    }

    fn shape(&mut self, raw: &RawShape<'a>, scope: &Scope, depth: usize, map: Map) -> Option<Shape> {
        if raw.deleted || depth > MAX_DEPTH {
            return None;
        }
        let ph = raw.placeholder.filter(|p| p.kind != 0);
        let text_box = raw.shape_type == 202;
        // A picture frame (also an OLE object's frame) shows its picture with no fill or line of
        // its own unless it says so; the drawing defaults' fill would cover a transparent picture.
        let picture_frame = raw.shape_type == PICTURE_FRAME;
        let props = self.props_of(raw, scope, ph.is_some() || text_box || picture_frame);
        let rot = props.fixed(prop::ROTATION).unwrap_or(0.0);
        let anchor = raw.anchor?;
        let mut s = Shape {
            id: ShapeId(self.alloc()?),
            name: props.string(prop::NAME).unwrap_or_else(|| default_name(raw)),
            xfrm: Some(xfrm(&anchor, map, rot, raw.flip_h, raw.flip_v)),
            descr: props.string(prop::DESCRIPTION).unwrap_or_default(),
            hidden: props.flag(prop::GROUP_BOOLS, 1).unwrap_or(false),
            ..Default::default()
        };
        if raw.is_group {
            // Children are placed in slide points; the group's child space is its own box.
            let at = s.xfrm.unwrap_or_default();
            let inner = Map::into_group(&at, &raw.group_rect.unwrap_or(anchor));
            let children = raw.children.iter().filter_map(|c| self.shape(c, scope, depth + 1, inner)).collect();
            let child = Xfrm::new(at.x, at.y, at.w, at.h);
            s.kind = ShapeKind::Group { children, child };
            return Some(s);
        }
        if !scope.master
            && let Some(ph) = ph
            && let Some(kind) = ph_type(ph.kind)
        {
            s.ph = Some(Placeholder { kind, idx: ph.position, ..Default::default() });
        }
        s.geom = geom::geometry(raw.shape_type, &props);
        if geom::is_connector(raw.shape_type) {
            s.kind = ShapeKind::Connector { start: None, end: None };
        }
        // A picture: a picture frame, or any shape showing a picture (an OLE object's preview).
        if let Some(media) = self.picture(&props, prop::PIB) {
            let crop = |id| props.fixed(id).unwrap_or(0.0).clamp(-1.0, 1.0);
            s.kind = ShapeKind::Picture {
                fill: PictureFill {
                    media,
                    crop: [crop(prop::CROP_LEFT), crop(prop::CROP_TOP), crop(prop::CROP_RIGHT), crop(prop::CROP_BOTTOM)],
                    ..Default::default()
                },
            };
        }
        s.fill = Some(self.fill(&props, scope, false).unwrap_or(Fill::None));
        s.line = Some(self.line(&props, scope));
        if let Some(shadow) = self.shadow(&props, scope) {
            s.effects = Some(Effects { outer_shadow: Some(shadow), ..Default::default() });
        }
        if let Some(block) = self.text_block(raw, scope) {
            s.text = Some(self.text_body(&block, scope, &props));
        } else if ph.is_some() {
            // An empty placeholder draws nothing (its prompt shows only while editing).
            s.text = Some(TextBody::default());
        }
        Some(s)
    }

    /// The picture a `pib` or `fillBlip` property names: a blip store entry (1-based), or a blip
    /// held in the property's own complex bytes.
    fn picture(&mut self, props: &Props<'a>, id: u16) -> Option<MediaId> {
        let Some(bytes) = props.complex(id) else {
            return props.get(id).filter(|&i| i > 0).and_then(|i| self.media(i as usize));
        };
        let key = bytes.as_ptr() as usize;
        if let Some(m) = self.inline_media.get(&key) {
            return *m;
        }
        let n = self.inline_media.len() + 1;
        let id = self.blips.decode_inline(bytes).and_then(|pic| self.p.add_media(&format!("inline{n}.{}", pic.ext), pic.content_type, pic.data));
        self.inline_media.insert(key, id);
        id
    }

    fn media(&mut self, pib: usize) -> Option<MediaId> {
        if let Some(m) = self.media.get(&pib) {
            return *m;
        }
        let id =
            self.blips.take(pib.wrapping_sub(1)).and_then(|pic| self.p.add_media(&format!("image{pib}.{}", pic.ext), pic.content_type, pic.data));
        self.media.insert(pib, id);
        id
    }

    fn color(&self, v: u32, props: &Props, scope: &Scope, depth: u8) -> Rgba {
        let [r, g, b, flags] = v.to_le_bytes();
        if flags & 0x08 != 0 {
            return scope.scheme.get(usize::from(r)).copied().unwrap_or(Rgba::BLACK);
        }
        if flags & 0x10 != 0 {
            // A system colour index; the high ones stand for the shape's own fill and line colours.
            let other = match r {
                0xF0 => Some(prop::FILL_COLOR),
                0xF1 => Some(prop::LINE_COLOR),
                0xF3 => Some(prop::FILL_BACK_COLOR),
                _ => None,
            };
            return match other.and_then(|id| props.get(id)) {
                Some(o) if depth < 2 => self.color(o, props, scope, depth + 1),
                _ => Rgba::BLACK,
            };
        }
        if flags & 0x01 != 0 {
            return Rgba::BLACK;
        }
        Rgba::rgb(r, g, b)
    }

    fn color_ref(&self, id: u16, fallback: u32, opacity: Option<u16>, props: &Props, scope: &Scope) -> ColorRef {
        let c = ColorRef::rgb(self.color(props.get(id).unwrap_or(fallback), props, scope, 0));
        match opacity.and_then(|o| props.fixed(o)).filter(|a| *a < 1.0) {
            Some(a) => c.with(ColorTransform::Alpha((a.clamp(0.0, 1.0) * 100_000.0) as i32)),
            None => c,
        }
    }

    fn fill(&mut self, props: &Props<'a>, scope: &Scope, background: bool) -> Option<Fill> {
        if !background && !props.flag(prop::FILL_BOOLS, 4).unwrap_or(true) {
            return None;
        }
        let fg = self.color_ref(prop::FILL_COLOR, 0x00FF_FFFF, Some(prop::FILL_OPACITY), props, scope);
        let bg = self.color_ref(prop::FILL_BACK_COLOR, 0x00FF_FFFF, Some(prop::FILL_BACK_OPACITY), props, scope);
        Some(match props.get(prop::FILL_TYPE).unwrap_or(0) {
            // Texture (tiled) and picture (stretched).
            t @ (2 | 3) => match self.picture(props, prop::FILL_BLIP) {
                Some(media) => {
                    let mode = if t == 2 {
                        PictureMode::Tile { tx: 0.0, ty: 0.0, sx: 1.0, sy: 1.0, flip: String::new(), align: "tl".into() }
                    } else {
                        PictureMode::default()
                    };
                    Fill::Picture(PictureFill { media, mode, ..Default::default() })
                }
                None => Fill::solid(fg),
            },
            // Gradients.
            t @ 4..=8 => {
                use deckcraft_model::style::{Gradient, GradientShape};
                let focus = props.get(prop::FILL_FOCUS).map_or(0, |v| v as i32).clamp(-100, 100);
                let stops = gradient_stops(focus, &fg, &bg);
                let shape = if matches!(t, 5 | 6) {
                    // Position 0 is the centre (the focus rectangle), 1 the shape's edge.
                    GradientShape::Path { path: if t == 5 { "rect".into() } else { "shape".into() }, focus: [0.5, 0.5, 0.5, 0.5] }
                } else {
                    GradientShape::Linear { angle: linear_angle(props.fixed(prop::FILL_ANGLE).unwrap_or(0.0)), scaled: false }
                };
                Fill::Gradient(Gradient { stops, shape, flip: None, rotate_with_shape: true, tile: [0.0; 4] })
            }
            9 => Fill::Background,
            _ => Fill::solid(fg),
        })
    }

    fn line(&self, props: &Props, scope: &Scope) -> Line {
        if !props.flag(prop::LINE_BOOLS, 3).unwrap_or(true) {
            return Line::none();
        }
        let width = props.get(prop::LINE_WIDTH).map_or(9525.0, f64::from) / EMU;
        let mut l = Line::solid(self.color_ref(prop::LINE_COLOR, 0, Some(prop::LINE_OPACITY), props, scope), width);
        l.dash = Some(Dash::Preset {
            name: match props.get(prop::LINE_DASHING).unwrap_or(0) {
                1 => "sysDash",
                2 => "sysDot",
                3 => "sysDashDot",
                4 => "sysDashDotDot",
                5 => "dot",
                6 => "dash",
                7 => "lgDash",
                8 => "dashDot",
                9 => "lgDashDot",
                10 => "lgDashDotDot",
                _ => "solid",
            }
            .into(),
        });
        l.compound = props.get(prop::LINE_STYLE).map(|v| match v {
            1 => deckcraft_model::style::Compound::Double,
            2 => deckcraft_model::style::Compound::ThickThin,
            3 => deckcraft_model::style::Compound::ThinThick,
            4 => deckcraft_model::style::Compound::Triple,
            _ => deckcraft_model::style::Compound::Single,
        });
        l.join = props.get(prop::LINE_JOIN).map(|v| match v {
            0 => LineJoin::Bevel,
            1 => LineJoin::Miter,
            _ => LineJoin::Round,
        });
        l.cap = props.get(prop::LINE_CAP).map(|v| match v {
            0 => LineCap::Round,
            1 => LineCap::Square,
            _ => LineCap::Flat,
        });
        let end = |kind: u16, w: u16, len: u16| {
            let k = match props.get(kind).unwrap_or(0) {
                1 => "triangle",
                2 => "stealth",
                3 => "diamond",
                4 => "oval",
                5 => "arrow",
                _ => return None,
            };
            let size = |v: Option<u32>| match v {
                Some(0) => "sm",
                Some(2) => "lg",
                _ => "med",
            };
            Some(LineEnd { kind: k.into(), w: size(props.get(w)).into(), len: size(props.get(len)).into() })
        };
        l.head = end(prop::LINE_START_ARROW, prop::LINE_START_ARROW_WIDTH, prop::LINE_START_ARROW_LENGTH);
        l.tail = end(prop::LINE_END_ARROW, prop::LINE_END_ARROW_WIDTH, prop::LINE_END_ARROW_LENGTH);
        l
    }

    fn shadow(&self, props: &Props, scope: &Scope) -> Option<Shadow> {
        if !props.flag(prop::SHADOW_BOOLS, 1).unwrap_or(false) {
            return None;
        }
        let dx = props.get(prop::SHADOW_OFFSET_X).map_or(25400.0, |v| f64::from(v as i32)) / EMU;
        let dy = props.get(prop::SHADOW_OFFSET_Y).map_or(25400.0, |v| f64::from(v as i32)) / EMU;
        Some(Shadow {
            color: self.color_ref(prop::SHADOW_COLOR, 0x0080_8080, Some(prop::SHADOW_OPACITY), props, scope),
            blur: 0.0,
            dist: dx.hypot(dy),
            dir: dy.atan2(dx).to_degrees().rem_euclid(360.0),
            inner: false,
            sx: 1.0,
            sy: 1.0,
            kx: 0.0,
            ky: 0.0,
            align: String::new(),
            rotate_with_shape: false,
        })
    }

    /// The text block of a shape: its own, or the slide list's outline block it points at.
    fn text_block(&self, raw: &RawShape<'a>, scope: &Scope) -> Option<TextBlock> {
        let tb = raw.text?;
        let own = text::read_block(tb.children())?;
        match own.outline_ref {
            Some(i) if own.chars.is_empty() => scope
                .outline
                .get(i as usize)
                .and_then(|r| text::read_block(r.clone()))
                .map(|mut b| {
                    b.outline_index = Some(i as usize);
                    b
                })
                .or(Some(own)),
            _ => Some(own),
        }
    }

    fn text_body(&self, block: &TextBlock, scope: &Scope, props: &Props) -> TextBody {
        let inset = |id, d: f64| props.get(id).map_or(d, |v| f64::from(v as i32) / EMU).max(0.0);
        let body = BodyProps {
            inset_l: Some(inset(prop::TEXT_LEFT, 7.2)),
            inset_t: Some(inset(prop::TEXT_TOP, 3.6)),
            inset_r: Some(inset(prop::TEXT_RIGHT, 7.2)),
            inset_b: Some(inset(prop::TEXT_BOTTOM, 3.6)),
            anchor: Some(match props.get(prop::ANCHOR_TEXT).unwrap_or(0) {
                // middle, middle centred
                1 | 4 => TextAnchor::Middle,
                // bottom, bottom centred, and their baseline forms
                2 | 5 | 7 | 9 => TextAnchor::Bottom,
                _ => TextAnchor::Top,
            }),
            anchor_ctr: Some(matches!(props.get(prop::ANCHOR_TEXT), Some(3 | 4 | 5 | 8 | 9))),
            wrap: Some(props.get(prop::WRAP_TEXT) != Some(2)),
            autofit: Some(AutoFit::None),
            ..Default::default()
        };
        TextBody { body, paragraphs: self.paragraphs(block, scope), ..Default::default() }
    }

    fn paragraphs(&self, block: &TextBlock, scope: &Scope) -> Vec<Paragraph> {
        let chars = &block.chars;
        let total = chars.len();
        let fields: HashMap<usize, FieldKind> = block.fields.iter().map(|(p, k)| (*p as usize, *k)).collect();
        // Where each formatting run starts.
        let starts = |counts: &mut dyn Iterator<Item = u32>| -> Vec<usize> {
            let mut at = 0usize;
            counts
                .map(|n| {
                    let s = at;
                    at = at.saturating_add(n as usize);
                    s
                })
                .collect()
        };
        let para_starts = starts(&mut block.para_runs.iter().map(|r| r.0));
        let char_starts = starts(&mut block.char_runs.iter().map(|r| r.0));
        // The starts never decrease (a running sum), so both lookups are binary searches. A scan per
        // run, break and field made text with many runs quadratic: 1M one-character runs, about 10 MB
        // of file, took minutes.
        let run_at = |starts: &[usize], i: usize| starts.partition_point(|&s| s <= i).checked_sub(1);
        let mut out = vec![];
        let mut start = 0usize;
        loop {
            let end = chars.get(start..).and_then(|rest| rest.iter().position(|&c| c == 0x0D)).map_or(total, |e| start + e);
            let (level, pf_exc) = run_at(&para_starts, start).and_then(|i| block.para_runs.get(i)).map_or((0, None), |r| (r.1, Some(&r.2)));
            let (mut pf, base_cf) = scope.styles.resolve(block.text_type, level);
            if let Some((lm, ind)) = block.ruler.get(usize::from(level)) {
                if lm.is_some() {
                    pf.left_margin = *lm;
                }
                if ind.is_some() {
                    pf.indent = *ind;
                }
            }
            if let Some(e) = pf_exc {
                pf.apply(e);
            }
            let cf_at = |i: usize| -> Cf {
                let mut cf = base_cf.clone();
                if let Some(e) = run_at(&char_starts, i).and_then(|k| block.char_runs.get(k)) {
                    cf.apply(&e.1);
                }
                cf
            };
            let first_cf = cf_at(start);
            let first = self.run_props(&first_cf, &scope.scheme);
            let mut para = Paragraph { level, props: self.para_props(&pf, &scope.scheme, first.size.unwrap_or(18.0)), ..Default::default() };
            // A bulleted paragraph whose PowerPoint 2000 formatting numbers it.
            if matches!(para.props.bullet, Some(Bullet::Char { .. }))
                && let Some((scheme, start_at)) = block
                    .outline_index
                    .and_then(|i| scope.numbering.get(i))
                    .and_then(Option::as_ref)
                    .zip(first_cf.pp9rt)
                    .and_then(|(entries, i)| entries.get(usize::from(i)))
                    .and_then(|e| e.auto_number)
            {
                para.props.bullet =
                    Some(Bullet::AutoNum { scheme: text::auto_number_scheme(scheme).into(), start_at: u32::try_from(start_at).unwrap_or(1).max(1) });
            }
            let mut i = start;
            while i < end {
                let props = self.run_props(&cf_at(i), &scope.scheme);
                let c = chars.get(i).copied().unwrap_or(0);
                if let Some(k) = fields.get(&i) {
                    let field = match k {
                        FieldKind::SlideNumber => "slidenum",
                        FieldKind::DateTime => "datetime1",
                    };
                    para.runs.push(Run {
                        text: if *k == FieldKind::SlideNumber { "‹#›".into() } else { String::new() },
                        props,
                        kind: RunKind::Field { field: field.into() },
                    });
                    i += 1;
                    continue;
                }
                if c == 0x0B {
                    para.runs.push(Run { text: String::new(), props, kind: RunKind::Break });
                    i += 1;
                    continue;
                }
                // A text run: up to the next format change, field, break or the paragraph's end.
                let next_change = char_starts.get(char_starts.partition_point(|&s| s <= i)).copied().unwrap_or(end).min(end);
                let mut j = i;
                while j < next_change && !fields.contains_key(&j) && chars.get(j) != Some(&0x0B) {
                    j += 1;
                }
                let j = j.max(i + 1);
                let units: Vec<u16> = chars.get(i..j).unwrap_or(&[]).iter().copied().filter(|&u| u >= 0x20 || u == 0x09).collect();
                para.runs.push(Run::with(String::from_utf16_lossy(&units), props));
                i = j;
            }
            para.end_props = first;
            para.normalize();
            out.push(para);
            if end >= total {
                break;
            }
            start = end + 1;
        }
        out
    }

    fn run_props(&self, cf: &Cf, scheme: &Scheme) -> RunProps {
        let font = |i: Option<u16>| i.and_then(|i| self.fonts.get(usize::from(i))).filter(|f| !f.is_empty()).cloned();
        RunProps {
            font: font(cf.font).or_else(|| self.fonts.first().cloned()),
            font_ea: font(cf.ea_font),
            font_sym: font(cf.symbol_font),
            size: cf.size.map(f64::from).filter(|s| (1.0..=4000.0).contains(s)),
            bold: Some(cf.bold.unwrap_or(false)),
            italic: Some(cf.italic.unwrap_or(false)),
            underline: Some(if cf.underline.unwrap_or(false) { "sng" } else { "none" }.into()),
            baseline: cf.position.filter(|p| *p != 0).map(|p| f64::from(p) / 100.0),
            fill: cf.color.map(|c| Fill::solid(ColorRef::rgb(text_color(c, scheme)))),
            shadow: cf.shadow.filter(|&on| on).map(|_| text_shadow(scheme)),
            ..Default::default()
        }
    }

    fn para_props(&self, pf: &Pf, scheme: &Scheme, text_size: f64) -> ParaProps {
        let spacing = |v: i16| if v >= 0 { Spacing::Pct(f64::from(v) / 100.0) } else { Spacing::Pts(-f64::from(v) / MU) };
        let margin = f64::from(pf.left_margin.unwrap_or(0)) / MU;
        let indent = f64::from(pf.indent.unwrap_or(0)) / MU;
        let bullet = if pf.has_bullet == Some(true) {
            let c = pf.bullet_char.filter(|&c| c >= 0x20).unwrap_or(0x2022);
            Bullet::Char { char: char::from_u32(u32::from(c)).unwrap_or('•').to_string() }
        } else {
            Bullet::None
        };
        let has_bullet = matches!(bullet, Bullet::Char { .. });
        ParaProps {
            align: Some(match pf.align.unwrap_or(0) {
                1 => Align::Center,
                2 => Align::Right,
                3 | 5 => Align::Justify,
                4 => Align::Distributed,
                6 => Align::JustLow,
                _ => Align::Left,
            }),
            margin_left: Some(margin),
            indent: Some(indent - margin),
            line_spacing: Some(spacing(pf.line_spacing.unwrap_or(100))),
            space_before: Some(spacing(pf.space_before.unwrap_or(0))),
            space_after: Some(spacing(pf.space_after.unwrap_or(0))),
            bullet: Some(bullet),
            bullet_font: if has_bullet && pf.bullet_has_font == Some(true) {
                pf.bullet_font.and_then(|i| self.fonts.get(usize::from(i))).cloned()
            } else {
                None
            },
            bullet_color: if has_bullet && pf.bullet_has_color == Some(true) {
                pf.bullet_color.map(|c| ColorRef::rgb(text_color(c, scheme)))
            } else {
                None
            },
            bullet_size: if has_bullet && pf.bullet_has_size == Some(true) {
                pf.bullet_size.map(|v| if v > 0 { f64::from(v) / 100.0 } else { -f64::from(v) / text_size.max(1.0) }).map(|v| v.clamp(0.25, 4.0))
            } else {
                None
            },
            default_tab: pf.default_tab.map(|t| f64::from(t) / MU),
            tabs: pf.tabs.as_ref().map(|t| {
                t.iter()
                    .map(|(pos, kind)| TabStop {
                        pos: f64::from(*pos) / MU,
                        align: match kind {
                            1 => "ctr",
                            2 => "r",
                            3 => "dec",
                            _ => "l",
                        }
                        .into(),
                    })
                    .collect()
            }),
            ..Default::default()
        }
    }
}

fn text_color(c: TextColor, scheme: &Scheme) -> Rgba {
    match c {
        TextColor::Rgb(r, g, b) => Rgba::rgb(r, g, b),
        TextColor::Scheme(i) => scheme.get(usize::from(i)).copied().unwrap_or(Rgba::BLACK),
    }
}

/// A text shadow in the colour scheme's shadow colour. The record holds only the on/off bit, so
/// the distance and direction are the app's own Text Shadow command's (`format.textShadow`); with
/// no blur in the record, none is added.
fn text_shadow(scheme: &Scheme) -> Shadow {
    Shadow {
        color: ColorRef::rgb(scheme.get(2).copied().unwrap_or(Rgba::rgb(0x80, 0x80, 0x80))),
        blur: 0.0,
        dist: 2.0,
        dir: 45.0,
        inner: false,
        sx: 1.0,
        sy: 1.0,
        kx: 0.0,
        ky: 0.0,
        align: String::new(),
        rotate_with_shape: false,
    }
}

/// A shaded fill's stops ([MS-ODRAW] `fillFocus`: the position of the last colour, the back
/// colour). Focus 0 starts at the back colour and ends at the fill colour, ±100 the other way; in
/// between, the back colour sits at that point with the fill colour at both ends.
fn gradient_stops(focus: i32, fg: &ColorRef, bg: &ColorRef) -> Vec<deckcraft_model::style::GradientStop> {
    use deckcraft_model::style::GradientStop;
    let stop = |pos: f64, c: &ColorRef| GradientStop { pos, color: c.clone() };
    match focus {
        0 => vec![stop(0.0, bg), stop(1.0, fg)],
        100 | -100 => vec![stop(0.0, fg), stop(1.0, bg)],
        f => vec![stop(0.0, fg), stop(f64::from(f.abs()) / 100.0, bg), stop(1.0, fg)],
    }
}

/// The DrawingML angle (0° runs left to right, turning clockwise) of a linear fill's `fillAngle`,
/// for stops from [`gradient_stops`]: the gradient starts on the left at -90°, the top at -180°,
/// the right at -270°, turning clockwise as the angle falls. 0° starts at the top too: that is how
/// LibreOffice reads it, and how it writes a top-to-bottom gradient.
fn linear_angle(a: f64) -> f64 {
    if a == 0.0 { 90.0 } else { (270.0 - a).rem_euclid(360.0) }
}

/// `msosptPictureFrame`.
const PICTURE_FRAME: u16 = 75;

fn no_fill_no_line<'a>() -> Props<'a> {
    let mut p = Props::default();
    p.set(prop::FILL_BOOLS, 0x0010_0000); // fFilled set to false
    p.set(prop::LINE_BOOLS, 0x0008_0000); // fLine set to false
    p
}

/// Maps anchor coordinates to slide points: master units at the top level, a group's child units
/// inside it (`x * sx + ox`).
#[derive(Clone, Copy, Debug)]
struct Map {
    sx: f64,
    sy: f64,
    ox: f64,
    oy: f64,
}

const TOP: Map = Map { sx: 1.0 / MU, sy: 1.0 / MU, ox: 0.0, oy: 0.0 };

impl Map {
    /// The map for the children of a group placed at `at` (points) whose child space is `rect`.
    fn into_group(at: &Xfrm, rect: &draw::Anchor) -> Map {
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let sx = if w.abs() > f64::EPSILON { at.w / w } else { 0.0 };
        let sy = if h.abs() > f64::EPSILON { at.h / h } else { 0.0 };
        Map { sx, sy, ox: at.x - rect.left * sx, oy: at.y - rect.top * sy }
    }
}

/// A box in points from an anchor. A shape turned by 45–135° or 225–315° stores the box of its
/// turned outline: swap it back.
fn xfrm(a: &draw::Anchor, m: Map, rot: f64, flip_h: bool, flip_v: bool) -> Xfrm {
    let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
    let (l, t, r, b) = (finite(a.left * m.sx + m.ox), finite(a.top * m.sy + m.oy), finite(a.right * m.sx + m.ox), finite(a.bottom * m.sy + m.oy));
    let (mut x, mut y, mut w, mut h) = (l.min(r), t.min(b), (r - l).abs(), (b - t).abs());
    let rot = if rot.is_finite() { rot.rem_euclid(360.0) } else { 0.0 };
    if (45.0..135.0).contains(&rot) || (225.0..315.0).contains(&rot) {
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        std::mem::swap(&mut w, &mut h);
        x = cx - w / 2.0;
        y = cy - h / 2.0;
    }
    Xfrm { x, y, w, h, rot, flip_h, flip_v }
}

fn default_name(raw: &RawShape) -> String {
    match raw.shape_type {
        75 => "Picture".into(),
        202 => "Text Box".into(),
        20 => "Line".into(),
        _ if raw.is_group => "Group".into(),
        _ => format!("Shape {}", raw.spid & 0x3FF),
    }
}

/// `PlaceholderEnum` as a DeckCraft placeholder type.
fn ph_type(kind: u8) -> Option<PhType> {
    Some(match kind {
        0x01 | 0x0D | 0x11 => PhType::Title,
        0x03 | 0x0F => PhType::CtrTitle,
        0x04 | 0x10 => PhType::SubTitle,
        0x02 | 0x06 | 0x0C | 0x0E | 0x12 => PhType::Body,
        0x07 => PhType::Date,
        0x08 => PhType::SlideNum,
        0x09 => PhType::Footer,
        0x0A => PhType::Header,
        0x05 | 0x0B => PhType::SlideImage,
        0x13 | 0x19 => PhType::Obj,
        0x14 => PhType::Chart,
        0x15 => PhType::Table,
        0x16 => PhType::ClipArt,
        0x17 => PhType::Diagram,
        0x18 => PhType::Media,
        0x1A => PhType::Picture,
        _ => return None,
    })
}

fn same_family(a: PhType, b: PhType) -> bool {
    let title = |t| matches!(t, PhType::Title | PhType::CtrTitle);
    a == b || (title(a) && title(b))
}

/// `SlideLayoutType` as a DeckCraft layout.
fn layout_kind(geom: u32) -> LayoutType {
    match geom {
        0 => LayoutType::Title,
        7 => LayoutType::TitleOnly,
        8 | 9 | 10 | 11 | 13 | 14 => LayoutType::TwoContent,
        16 => LayoutType::Blank,
        17 => LayoutType::VerticalTitleAndText,
        18 => LayoutType::TitleAndVerticalText,
        _ => LayoutType::TitleAndContent,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// [MS-ODRAW] `fillFocus` is where the back colour sits; LibreOffice draws 0° from the top
    /// (#1137 review: the linear gradient's colours were reversed, the radial one inside out).
    #[test]
    fn shaded_fills_put_the_back_colour_at_the_focus() {
        let (fg, bg) = (ColorRef::rgb(Rgba::rgb(255, 0, 0)), ColorRef::rgb(Rgba::rgb(0, 0, 255)));
        let at = |stops: &[deckcraft_model::style::GradientStop]| stops.iter().map(|s| (s.pos, s.color == fg)).collect::<Vec<_>>();
        assert_eq!(at(&gradient_stops(0, &fg, &bg)), [(0.0, false), (1.0, true)]);
        assert_eq!(at(&gradient_stops(100, &fg, &bg)), [(0.0, true), (1.0, false)]);
        assert_eq!(at(&gradient_stops(-50, &fg, &bg)), [(0.0, true), (0.5, false), (1.0, true)]);
        // 0° and -180° start at the top, -90° on the left, -270° on the right, -315° at the
        // bottom right (DrawingML: 90° runs top to bottom, 0° left to right).
        for (a, dm) in [(0.0, 90.0), (-90.0, 0.0), (-180.0, 90.0), (-270.0, 180.0), (-315.0, 225.0), (-45.0, 315.0)] {
            assert_eq!(linear_angle(a), dm, "{a}");
        }
    }
}
