//! PPTX import and export: Office Open XML PresentationML / DrawingML, delegated to the shared
//! `craft-pptx` crate.
//!
//! [`import`] reads a package leniently (unknown parts are skipped, malformed parts are logged and
//! ignored, every size and depth is bounded) into the DeckCraft model; [`export`] writes a valid
//! package that PowerPoint opens without repair.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod cfb;
mod convert;
mod xml;

pub use craft_pptx::MAX_TAG_BYTES;
/// The bounded XML reader the importer uses, for callers that read kept fragments (a shape's
/// 3-D settings) instead of scanning strings.
pub use xml::{Doc as XmlDoc, El as XmlEl, parse as parse_xml};

use deckcraft_model::{Presentation, Shape, ShapeKind, defaults};

#[derive(Debug, thiserror::Error)]
pub enum PptxError {
    #[error("not a PPTX file: {0}")]
    NotPptx(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("write failed: {0}")]
    Write(String),
    #[error("malformed XML: {0}")]
    Xml(String),
}

impl From<craft_pptx::Error> for PptxError {
    fn from(e: craft_pptx::Error) -> Self {
        match e {
            craft_pptx::Error::NotPptx(m) => PptxError::NotPptx(m),
            craft_pptx::Error::Unsupported(m) => PptxError::Unsupported(m),
            craft_pptx::Error::Write(m) => PptxError::Write(m),
            craft_pptx::Error::Xml(m) => PptxError::Xml(m),
        }
    }
}

struct DeckDefaults;

impl craft_pptx::Defaults for DeckDefaults {
    fn master(&self, size: deckcraft_geom::Size, after: u32) -> (craft_pptx::Master, u32) {
        let (m, last) = defaults::build_master(size, Default::default(), after);
        (convert::master_to_craft(&m), last)
    }

    fn theme(&self) -> craft_drawingml::theme::Theme {
        convert::theme_to_craft(&deckcraft_model::theme::Theme::default())
    }

    fn master_styles(&self) -> (craft_drawingml::text::ListStyle, craft_drawingml::text::ListStyle, craft_drawingml::text::ListStyle) {
        // An imported empty master style must stay empty on save; new decks already carry their
        // populated master styles in the model.
        Default::default()
    }

    fn default_text_style(&self) -> craft_drawingml::text::ListStyle {
        convert::list_style_to_craft(&defaults::default_text_style())
    }

    fn application(&self) -> &str {
        "DeckCraft"
    }
}

/// Does this look like an OOXML package (zip with `[Content_Types].xml`)?
///
/// The part is looked up in the archive's central directory, not in the leading bytes: writers may
/// store it anywhere in the archive (recent PowerPoint builds put it last, megabytes in).
pub fn sniff(bytes: &[u8]) -> bool {
    craft_pptx::sniff(bytes)
}

/// Is this a password-protected presentation (an encrypted package inside a compound file)?
pub fn is_encrypted(bytes: &[u8]) -> bool {
    cfb::is_encrypted(bytes)
}

const MAX_SLIDE_ENTRIES: usize = 10_000;

/// Checks `ppt/presentation.xml` for > 10,000 slide-list entries (refused instead of truncated)
/// and collects any `sldId` entries whose relationship is missing so `import_checked` /
/// `import_report` report them as lost slides.
fn preflight_slides(bytes: &[u8]) -> Result<Vec<String>, PptxError> {
    let Ok(mut pkg) = craft_opc::LazyPackage::open(bytes, &craft_pptx::LIMITS) else {
        return Ok(vec![]);
    };
    let root_rels = pkg.rels("").unwrap_or_default();
    let pres_path = root_rels.first("officeDocument").map(|r| r.target.clone()).unwrap_or_else(|| "ppt/presentation.xml".into());
    let Ok(Some(pres_bytes)) = pkg.read_limited(&pres_path, craft_pptx::MAX_XML_PART as u64) else {
        return Ok(vec![]);
    };
    let Ok(pres_el) = craft_drawingml::xmlx::parse(&pres_bytes) else {
        return Ok(vec![]);
    };
    let pres_rels = pkg.rels(&pres_path).unwrap_or_default();
    let mut lost = Vec::new();
    if let Some(list) = pres_el.child(craft_xml::Local("sldIdLst")) {
        let mut count = 0usize;
        for s in list.children(craft_xml::Local("sldId")) {
            count += 1;
            if count > MAX_SLIDE_ENTRIES {
                return Err(PptxError::Unsupported(format!("presentation has more than {MAX_SLIDE_ENTRIES} slides")));
            }
            let rid = s.attr_local("r:id").unwrap_or("");
            if pres_rels.target(rid).is_none() {
                let id = s.attr_local("id").and_then(craft_xml::value::to_u32_lossy).unwrap_or(0);
                lost.push(format!("slide {id} (missing relationship {rid})"));
            }
        }
    }
    Ok(lost)
}

fn has_unfolded_paragraph_builds(p: &Presentation) -> bool {
    p.slides.iter().any(|s| s.animations.windows(2).any(|w| w[0].paragraph.is_some() && w[1].paragraph.is_some() && w[0].shape == w[1].shape))
}

#[derive(Default)]
struct SlideTimingInfo {
    opt_out: std::collections::HashSet<u32>,
    offsets: std::collections::HashMap<(u32, u32), u32>,
}

fn timing_ms(e: &craft_xml::El, name: &str) -> Option<u32> {
    let v = e.attr_local(name)?.trim();
    if v == "indefinite" {
        return None;
    }
    v.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0).map(|n| n.min(u32::MAX as f64) as u32)
}

fn collect_timing_offsets(e: &craft_xml::El, offset: u32, out: &mut std::collections::HashMap<(u32, u32), u32>, depth: usize) {
    if depth > 64 {
        return;
    }
    for c in e.elements() {
        if c.is(craft_xml::Local("cTn")) && c.attr_local("presetClass").is_some() {
            if let Some(sp) = c.find(craft_xml::Local("spTgt"))
                && let Some(spid) = sp.attr_local("spid").and_then(craft_xml::value::to_u32_lossy)
                && let Some(p) = sp
                    .path(&[craft_xml::Local("txEl"), craft_xml::Local("pRg")])
                    .and_then(|r| r.attr_local("st"))
                    .and_then(craft_xml::value::to_u32_lossy)
            {
                out.entry((spid, p)).or_insert(offset);
            }
            continue;
        }
        let mut next_offset = offset;
        if c.is(craft_xml::Local("cTn"))
            && let Some(cond) = c.path(&[craft_xml::Local("stCondLst"), craft_xml::Local("cond")])
        {
            if cond.attr_local("delay").map(str::trim) == Some("indefinite") {
                next_offset = 0;
            } else {
                next_offset = next_offset.saturating_add(timing_ms(cond, "delay").unwrap_or(0));
            }
        }
        collect_timing_offsets(c, next_offset, out, depth + 1);
    }
}

fn defers_start(st: &craft_xml::El) -> bool {
    let is_auto_begin = |c: &craft_xml::El| {
        c.attr_local("evt") == Some("onBegin")
            && c.attr_local("delay").map(str::trim) == Some("0")
            && c.child(craft_xml::Local("tgtEl")).is_none()
            && c.child(craft_xml::Local("tn")).is_none()
            && c.child(craft_xml::Local("rtn")).is_none()
    };
    let has_on_begin = st.children(craft_xml::Local("cond")).any(is_auto_begin);
    st.children(craft_xml::Local("cond"))
        .any(|c| (c.attr_local("evt").is_some() && !is_auto_begin(c)) || (!has_on_begin && c.attr_local("delay").map(str::trim) != Some("0")))
}

fn is_main_seq_auto(st: &craft_xml::El, main_seq_id: Option<u32>) -> bool {
    let Some(ms_id) = main_seq_id else {
        return false;
    };
    let is_indefinite = |d: &craft_xml::El| {
        d.attr_local("evt").is_none()
            && d.attr_local("delay").map(str::trim) == Some("indefinite")
            && d.child(craft_xml::Local("tgtEl")).is_none()
            && d.child(craft_xml::Local("tn")).is_none()
            && d.child(craft_xml::Local("rtn")).is_none()
    };
    let is_main_seq_on_begin = |d: &craft_xml::El| {
        d.attr_local("evt") == Some("onBegin")
            && d.attr_local("delay").map(str::trim) == Some("0")
            && d.child(craft_xml::Local("tgtEl")).is_none()
            && d.child(craft_xml::Local("rtn")).is_none()
            && d.child(craft_xml::Local("tn")).and_then(|tn| tn.attr_local("val")).and_then(craft_xml::value::to_u32_lossy) == Some(ms_id)
    };
    st.children(craft_xml::Local("cond")).any(is_indefinite)
        && st.children(craft_xml::Local("cond")).any(is_main_seq_on_begin)
        && st.children(craft_xml::Local("cond")).all(|d| is_indefinite(d) || is_main_seq_on_begin(d))
}

fn collect_media_nodes<'a>(e: &'a craft_xml::El, deferred: bool, out: &mut Vec<(&'a craft_xml::El, bool)>, depth: usize) {
    if depth > 64 || out.len() >= 10_000 {
        return;
    }
    let mut deferred = deferred;
    if e.is(craft_xml::Local("cTn"))
        && let Some(st) = e.child(craft_xml::Local("stCondLst"))
        && defers_start(st)
    {
        deferred = true;
    }
    if e.is(craft_xml::Local("video")) || e.is(craft_xml::Local("audio")) {
        out.push((e, deferred));
        return;
    }
    for c in e.elements() {
        collect_media_nodes(c, deferred, out, depth + 1);
    }
}

#[derive(Clone, Copy)]
struct MediaPlayAt {
    trigger: Option<u32>,
    main_seq_id: Option<u32>,
    offset: u32,
    deferred: bool,
}

fn collect_media_plays<'a>(e: &'a craft_xml::El, at: MediaPlayAt, out: &mut Vec<(&'a craft_xml::El, MediaPlayAt)>, depth: usize) {
    if depth > 64 || out.len() > 10_000 {
        return;
    }
    for c in e.elements() {
        if c.is(craft_xml::Local("cTn")) && c.attr_local("presetClass").is_some() {
            let deferred = at.deferred || c.child(craft_xml::Local("stCondLst")).is_some_and(defers_start);
            out.push((c, MediaPlayAt { deferred, ..at }));
            continue;
        }
        let mut next = at;
        if c.is(craft_xml::Local("seq"))
            && let Some(ctn) = c.child(craft_xml::Local("cTn"))
            && ctn.attr_local("nodeType") == Some("interactiveSeq")
        {
            next.trigger = ctn
                .path(&[craft_xml::Local("stCondLst"), craft_xml::Local("cond"), craft_xml::Local("tgtEl"), craft_xml::Local("spTgt")])
                .and_then(|s| s.attr_local("spid"))
                .and_then(craft_xml::value::to_u32_lossy);
            next.main_seq_id = None;
        }
        if c.is(craft_xml::Local("cTn")) {
            let is_main_seq = c.attr_local("nodeType") == Some("mainSeq");
            if let Some(st) = c.child(craft_xml::Local("stCondLst")) {
                if !(!is_main_seq && !next.deferred && is_main_seq_auto(st, next.main_seq_id)) && defers_start(st) {
                    next.deferred = true;
                }
                if let Some(cond) = st.child(craft_xml::Local("cond")) {
                    if cond.attr_local("delay").map(str::trim) == Some("indefinite") {
                        next.offset = 0;
                    } else {
                        next.offset = next.offset.saturating_add(timing_ms(cond, "delay").unwrap_or(0));
                    }
                }
            }
            next.main_seq_id = if is_main_seq && !next.deferred { c.attr_local("id").and_then(craft_xml::value::to_u32_lossy) } else { None };
        }
        collect_media_plays(c, next, out, depth + 1);
    }
}

fn apply_media_timing(timing: &craft_xml::El, shapes: &mut [Shape]) {
    let mut nodes = Vec::new();
    collect_media_nodes(timing, false, &mut nodes, 0);
    for (node, deferred) in nodes {
        let Some(cm) = node.child(craft_xml::Local("cMediaNode")) else { continue };
        let Some(spid) = cm
            .path(&[craft_xml::Local("tgtEl"), craft_xml::Local("spTgt")])
            .and_then(|t| t.attr_local("spid"))
            .and_then(craft_xml::value::to_u32_lossy)
        else {
            continue;
        };
        let Some(ShapeKind::Media(clip)) = deckcraft_model::find_shape_mut(shapes, deckcraft_model::ShapeId(spid)).map(|s| &mut s.kind) else {
            continue;
        };
        if cm.child(craft_xml::Local("cTn")).and_then(|c| c.attr_local("fill")) == Some("remove") {
            clip.rewind = true;
        }
        if cm.attr_local("numSld").and_then(craft_xml::value::to_u32_lossy).is_some_and(|n| n > 1) {
            clip.play_across_slides = true;
        }
        if !deferred
            && cm
                .path(&[craft_xml::Local("cTn"), craft_xml::Local("stCondLst"), craft_xml::Local("cond")])
                .is_some_and(|c| c.attr_local("evt").is_none() && c.attr_local("delay").map(str::trim) == Some("0"))
        {
            clip.autoplay = true;
        }
    }
    let root_deferred = timing
        .path(&[craft_xml::Local("tnLst"), craft_xml::Local("par"), craft_xml::Local("cTn"), craft_xml::Local("stCondLst")])
        .is_some_and(defers_start);
    if !root_deferred && let Some(tn) = timing.child(craft_xml::Local("tnLst")) {
        let mut found = Vec::new();
        collect_media_plays(tn, MediaPlayAt { trigger: None, main_seq_id: None, offset: 0, deferred: false }, &mut found, 0);
        for (c, f) in found.into_iter().filter(|(_, f)| f.trigger.is_none()) {
            if c.attr_local("nodeType") == Some("clickEffect") || f.deferred {
                break;
            }
            let delay = c.path(&[craft_xml::Local("stCondLst"), craft_xml::Local("cond")]).and_then(|d| timing_ms(d, "delay")).unwrap_or(0);
            if c.attr_local("presetClass") == Some("mediacall")
                && c.attr_local("presetID").and_then(craft_xml::value::to_u32_lossy) == Some(1)
                && f.offset == 0
                && delay == 0
                && let Some(spid) = c.find(craft_xml::Local("spTgt")).and_then(|t| t.attr_local("spid")).and_then(craft_xml::value::to_u32_lossy)
                && let Some(ShapeKind::Media(clip)) = deckcraft_model::find_shape_mut(shapes, deckcraft_model::ShapeId(spid)).map(|s| &mut s.kind)
            {
                clip.autoplay = true;
            }
        }
    }
}

fn has_media_shapes(shapes: &[Shape]) -> bool {
    shapes.iter().any(|s| match &s.kind {
        ShapeKind::Media(_) => true,
        ShapeKind::Group { children, .. } => has_media_shapes(children),
        _ => false,
    })
}

fn post_import_timing(bytes: &[u8], pres: &mut Presentation) {
    let need_builds = has_unfolded_paragraph_builds(pres);
    let need_media = pres.slides.iter().any(|s| has_media_shapes(&s.shapes));
    if !need_builds && !need_media {
        return;
    }
    let Ok(mut pkg) = craft_opc::LazyPackage::open(bytes, &craft_pptx::LIMITS) else {
        return;
    };
    let root_rels = pkg.rels("").unwrap_or_default();
    let pres_path = root_rels.first("officeDocument").map(|r| r.target.clone()).unwrap_or_else(|| "ppt/presentation.xml".into());
    let Ok(Some(pres_bytes)) = pkg.read_limited(&pres_path, craft_pptx::MAX_XML_PART as u64) else {
        return;
    };
    let Ok(pres_el) = craft_drawingml::xmlx::parse(&pres_bytes) else {
        return;
    };
    let pres_rels = pkg.rels(&pres_path).unwrap_or_default();
    let Some(list) = pres_el.child(craft_xml::Local("sldIdLst")) else {
        return;
    };
    for (i, s) in list.children(craft_xml::Local("sldId")).take(MAX_SLIDE_ENTRIES).enumerate() {
        let Some(slide_arc) = pres.slides.get_mut(i) else { break };
        let Some(target) = s.attr_local("r:id").and_then(|rid| pres_rels.target(rid)) else { continue };
        let Ok(Some(sb)) = pkg.read_limited(target, craft_pptx::MAX_XML_PART as u64) else { continue };
        let Ok(se) = craft_drawingml::xmlx::parse(&sb) else { continue };
        let Some(timing) = se.child(craft_xml::Local("timing")) else { continue };
        if need_media && has_media_shapes(&slide_arc.shapes) {
            apply_media_timing(timing, &mut std::sync::Arc::make_mut(slide_arc).shapes);
        }
        if need_builds {
            let mut info = SlideTimingInfo::default();
            collect_timing_offsets(timing, 0, &mut info.offsets, 0);
            if let Some(bld_lst) = timing.child(craft_xml::Local("bldLst")) {
                for b in bld_lst.children(craft_xml::Local("bldP")) {
                    if b.attr_local("build").unwrap_or("") != "p"
                        && let Some(spid) = b.attr_local("spid").and_then(craft_xml::value::to_u32_lossy)
                    {
                        info.opt_out.insert(spid);
                    }
                }
            }
            fold_slide_paragraph_builds(std::sync::Arc::make_mut(slide_arc), &info);
        }
    }
}

fn span_ms(a: &deckcraft_model::anim::Animation) -> u32 {
    let iterations = if a.repeat == u32::MAX || a.repeat <= 1 { 1 } else { a.repeat.min(1_000) };
    a.duration_ms.saturating_mul(if a.auto_reverse { 2 } else { 1 }).saturating_mul(iterations)
}

fn flat_starts(anims: &[&deckcraft_model::anim::Animation]) -> Vec<u32> {
    let (mut begin, mut end) = (0u32, 0u32);
    anims
        .iter()
        .map(|a| {
            if a.start == deckcraft_model::anim::AnimStart::AfterPrevious {
                begin = end;
            }
            let start = begin.saturating_add(a.delay_ms);
            end = end.max(start.saturating_add(span_ms(a)));
            start
        })
        .collect()
}

fn plays_as_build(effects: &[&deckcraft_model::anim::Animation], offsets: &[u32], text: &deckcraft_model::text::TextBody) -> bool {
    use deckcraft_model::anim::{AnimStart, Animation, BuildRole, build_filled, paragraph_build_roles};
    let Some(first) = effects.first() else { return false };
    let off0 = offsets.first().copied().unwrap_or(0);
    let flat = flat_starts(effects);
    let parent_starts_match = offsets
        .iter()
        .zip(effects.iter())
        .zip(flat.iter())
        .all(|((&off, a), &ft)| off == off0 || off.saturating_add(a.delay_ms) == off0.saturating_add(ft));
    if !parent_starts_match || (first.start != AnimStart::OnClick && (span_ms(first) != 500 || first.delay_ms != 0 || off0 != 0)) {
        return false;
    }
    let roles = paragraph_build_roles(text);
    let expected = roles.iter().enumerate().filter(|(i, r)| match r {
        BuildRole::Top => true,
        BuildRole::Member(_) => text.paragraphs.get(*i).is_some_and(build_filled),
        BuildRole::Unowned => false,
    });
    let settings = |a: &Animation| Animation { start: AnimStart::OnClick, paragraph: None, ..a.clone() };
    let base = settings(first);
    let mut matched = 0;
    for (k, (i, role)) in expected.enumerate() {
        let Some(a) = effects.get(k) else { return false };
        let start_ok = match role {
            BuildRole::Top if k == 0 => true,
            BuildRole::Top if first.start == AnimStart::OnClick => a.start == AnimStart::OnClick,
            BuildRole::Top => a.start == AnimStart::AfterPrevious,
            _ => a.start == AnimStart::WithPrevious,
        };
        if a.paragraph != u32::try_from(i).ok() || !start_ok || settings(a) != base {
            return false;
        }
        matched = k + 1;
    }
    matched == effects.len()
}

fn fold_slide_paragraph_builds(slide: &mut deckcraft_model::Slide, info: &SlideTimingInfo) {
    use deckcraft_model::anim::{Animation, TextBuild};
    let same = |x: &Animation, y: &Animation| {
        x.paragraph.is_some()
            && y.paragraph.is_some()
            && x.text_build == TextBuild::AsOne
            && y.text_build == TextBuild::AsOne
            && x.shape == y.shape
            && x.trigger == y.trigger
            && !info.opt_out.contains(&x.shape.0)
    };
    let mut folded = Vec::with_capacity(slide.animations.len());
    for run in slide.animations.chunk_by(same) {
        let effects: Vec<&Animation> = run.iter().collect();
        let offsets: Vec<u32> = run.iter().map(|a| a.paragraph.and_then(|p| info.offsets.get(&(a.shape.0, p)).copied()).unwrap_or(0)).collect();
        let text = effects.first().and_then(|a| deckcraft_model::find_shape(&slide.shapes, a.shape)).and_then(|s| s.text.as_ref());
        if run.len() > 1
            && let (Some(first), Some(text)) = (effects.first(), text)
            && !info.opt_out.contains(&first.shape.0)
            && plays_as_build(&effects, &offsets, text)
        {
            folded.push(Animation { text_build: TextBuild::ByParagraph, paragraph: None, ..(*first).clone() });
        } else {
            folded.extend(run.iter().cloned());
        }
    }
    slide.animations = folded;
}

/// Read a `.pptx` / `.potx` / `.ppsx` package.
pub fn import(bytes: &[u8]) -> Result<Presentation, PptxError> {
    preflight_slides(bytes)?;
    let mut pres = craft_pptx::import(bytes, &DeckDefaults).map(convert::pres_from_craft)?;
    post_import_timing(bytes, &mut pres);
    Ok(pres)
}

/// [`import`], plus the part names of slides the file lists but that couldn't be read and were
/// left out.
/// Files with more than 10,000 slide-list entries return [`PptxError::Unsupported`] instead of a truncated presentation.
pub fn import_checked(bytes: &[u8]) -> Result<(Presentation, Vec<String>), PptxError> {
    let (p, lost, _) = import_report(bytes)?;
    Ok((p, lost))
}

/// [`import_checked`], plus warnings for content cut to fit a limit (a table past the cell cap
/// keeps its leading rows); the file still opens.
pub fn import_report(bytes: &[u8]) -> Result<(Presentation, Vec<String>, Vec<String>), PptxError> {
    let mut missing = preflight_slides(bytes)?;
    let (p, lost, warnings) = craft_pptx::import_report(bytes, &DeckDefaults)?;
    missing.extend(lost);
    let mut pres = convert::pres_from_craft(p);
    post_import_timing(bytes, &mut pres);
    Ok((pres, missing, warnings))
}

/// [`import`] with a lower presentation-wide chart point budget, so tests reach the over-budget
/// path without a multi-million-point chart.
#[doc(hidden)]
pub fn import_with_chart_budget(bytes: &[u8], points: usize) -> Result<Presentation, PptxError> {
    preflight_slides(bytes)?;
    let mut pres = craft_pptx::import_with_chart_budget(bytes, &DeckDefaults, points).map(convert::pres_from_craft)?;
    post_import_timing(bytes, &mut pres);
    Ok(pres)
}

/// Read the theme of a `.thmx` file, or of a `.pptx` / `.potx`'s first master, with its media.
pub fn import_theme(bytes: &[u8]) -> Result<(deckcraft_model::theme::Theme, Vec<deckcraft_model::MediaItem>), PptxError> {
    let (theme, media) = craft_pptx::import_theme(bytes)?;
    Ok((convert::theme_from_craft(theme), media.into_iter().map(convert::media_item_from_craft).collect()))
}

fn shapes_have_over_cap_table(shapes: &[Shape]) -> bool {
    shapes.iter().any(|s| match &s.kind {
        ShapeKind::Table(t) => t.over_cap(),
        ShapeKind::Group { children, .. } => shapes_have_over_cap_table(children),
        _ => false,
    })
}

fn check_table_caps(p: &Presentation) -> Result<(), PptxError> {
    let over = p.slides.iter().any(|s| shapes_have_over_cap_table(&s.shapes))
        || p.masters.iter().any(|m| shapes_have_over_cap_table(&m.shapes) || m.layouts.iter().any(|l| shapes_have_over_cap_table(&l.shapes)))
        || p.notes_master.as_ref().is_some_and(|m| shapes_have_over_cap_table(&m.shapes))
        || p.handout_master.as_ref().is_some_and(|m| shapes_have_over_cap_table(&m.shapes));
    if over {
        return Err(PptxError::Unsupported(format!(
            "a table has more than {} cells; export refused rather than drop rows",
            deckcraft_model::table::MAX_CELLS
        )));
    }
    Ok(())
}

fn media_needs_timing(clip: &deckcraft_model::MediaClip) -> bool {
    clip.autoplay
        || (clip.volume - 1.0).abs() > 1e-6
        || clip.hide_while_not_playing
        || clip.loop_play
        || clip.rewind
        || clip.play_across_slides
        || (clip.video && clip.full_screen)
}

fn assign_spids<'a>(shapes: &'a [Shape], mut visit: impl FnMut(u32, &'a Shape)) {
    let mut used = std::collections::HashSet::new();
    let mut max = 1u32;
    deckcraft_model::walk(shapes, &mut |s, _| {
        if s.id.0 >= 2 && s.id.0 < i32::MAX as u32 {
            max = max.max(s.id.0);
        }
    });
    let mut next = max;
    deckcraft_model::walk(shapes, &mut |s, _| {
        let spid = if s.id.0 >= 2 && s.id.0 < i32::MAX as u32 && !used.contains(&s.id.0) {
            s.id.0
        } else {
            next = next.saturating_add(1);
            next
        };
        used.insert(spid);
        visit(spid, s);
    });
}

fn collect_export_media(shapes: &[Shape]) -> Vec<(u32, &deckcraft_model::MediaClip)> {
    let mut media = Vec::new();
    assign_spids(shapes, |spid, s| {
        if let ShapeKind::Media(clip) = &s.kind
            && media_needs_timing(clip)
        {
            media.push((spid, clip));
        }
    });
    media
}

fn collect_grp_fill_pic_spids(shapes: &[Shape]) -> std::collections::HashSet<u32> {
    let mut spids = std::collections::HashSet::new();
    assign_spids(shapes, |spid, s| {
        if s.fill == Some(deckcraft_model::style::Fill::Group)
            && matches!(s.kind, ShapeKind::Picture { .. } | ShapeKind::Opaque { preview: Some(_), .. })
        {
            spids.insert(spid);
        }
    });
    spids
}

fn write_media_node(w: &mut craft_xml::Writer, id: &mut u32, spid: u32, clip: &deckcraft_model::MediaClip) {
    let tag = if clip.video { "p:video" } else { "p:audio" };
    let mut t = w.tag(tag);
    if clip.video && clip.full_screen {
        t = t.attr("fullScrn", "1");
    }
    t.open();
    let vol = (clip.volume.clamp(0.0, 1.0) * 100_000.0).round() as u32;
    let mut cm = w.tag("p:cMediaNode").attr("vol", vol);
    if clip.volume <= 1e-6 {
        cm = cm.attr("mute", "1");
    }
    if clip.hide_while_not_playing {
        cm = cm.attr("showWhenStopped", "0");
    }
    if clip.play_across_slides {
        cm = cm.attr("numSld", "999");
    }
    cm.open();
    *id = id.saturating_add(1);
    let mut ctn = w.tag("p:cTn").attr("id", *id).attr("fill", if clip.rewind { "remove" } else { "hold" }).attr("display", "0");
    if clip.loop_play {
        ctn = ctn.attr("repeatCount", "indefinite");
    }
    ctn.open();
    w.open("p:stCondLst", &[]);
    w.tag("p:cond").attr("delay", if clip.autoplay { "0" } else { "indefinite" }).empty();
    w.close();
    w.close();
    w.open("p:tgtEl", &[]);
    w.tag("p:spTgt").attr("spid", spid).empty();
    w.close();
    w.close();
    w.close();
}

fn max_ctn_id(e: &craft_xml::El, max_id: &mut u32, depth: usize) {
    if depth > 64 {
        return;
    }
    if e.is(craft_xml::Local("cTn"))
        && let Some(id) = e.attr_local("id").and_then(craft_xml::value::to_u32_lossy)
    {
        *max_id = (*max_id).max(id);
    }
    for c in e.elements() {
        max_ctn_id(c, max_id, depth + 1);
    }
}

fn patch_slide_media_timing(xml_bytes: &[u8], media: &[(u32, &deckcraft_model::MediaClip)]) -> Vec<u8> {
    let Ok(mut xml) = String::from_utf8(xml_bytes.to_vec()) else {
        return xml_bytes.to_vec();
    };
    let mut next_id = 1u32;
    if let Ok(el) = craft_drawingml::xmlx::parse(xml_bytes) {
        max_ctn_id(&el, &mut next_id, 0);
    }
    let mut w = craft_xml::Writer::new();
    for &(spid, clip) in media {
        write_media_node(&mut w, &mut next_id, spid, clip);
    }
    let nodes = w.finish();
    if let Some(tn_end) = xml.rfind("</p:tnLst>")
        && let Some(pos) = xml[..tn_end].rfind("</p:childTnLst>")
    {
        xml.insert_str(pos, &nodes);
    } else if let Some(pos) = xml.rfind("</p:sld>") {
        let timing = format!(
            r#"<p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst>{nodes}</p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>"#
        );
        xml.insert_str(pos, &timing);
    }
    xml.into_bytes()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeOwner {
    Sp,
    CxnSp,
    Pic,
}

/// Returns the owning shape kind and closing-tag byte offset for a `<p:cNvPr id="…">` at
/// `start..after`, or `None` if it belongs to a non-shape container (`<p:grpSp>`,
/// `<p:graphicFrame>`) or would cross into another shape.
fn owning_shape_end(xml: &str, start: usize, after: usize) -> Option<(ShapeOwner, usize)> {
    let before = &xml[..start];
    let (nv_pos, owner) = [(ShapeOwner::Sp, "<p:nvSpPr>"), (ShapeOwner::CxnSp, "<p:nvCxnSpPr>"), (ShapeOwner::Pic, "<p:nvPicPr>")]
        .into_iter()
        .filter_map(|(owner, tag)| before.rfind(tag).map(|pos| (pos, owner)))
        .max_by_key(|(pos, _)| *pos)?;
    let between = &before[nv_pos..];
    if ["<p:nvGrpSpPr>", "<p:nvGraphicFramePr>", "</p:nvSpPr>", "</p:nvCxnSpPr>", "</p:nvPicPr>"].iter().any(|t| between.contains(t)) {
        return None;
    }
    let close_tag = match owner {
        ShapeOwner::Sp => "</p:sp>",
        ShapeOwner::CxnSp => "</p:cxnSp>",
        ShapeOwner::Pic => "</p:pic>",
    };
    let end_rel = xml[after..].find(close_tag)?;
    let span = &xml[after..after + end_rel];
    if ["<p:sp>", "<p:sp ", "<p:cxnSp>", "<p:cxnSp ", "<p:pic>", "<p:pic ", "<p:grpSp>", "<p:grpSp ", "<p:graphicFrame>", "<p:graphicFrame "]
        .iter()
        .any(|t| span.contains(t))
    {
        return None;
    }
    Some((owner, after + end_rel))
}

fn patch_pic_grp_fill(xml_bytes: &[u8], spids: &std::collections::HashSet<u32>) -> Vec<u8> {
    let Ok(mut xml) = String::from_utf8(xml_bytes.to_vec()) else {
        return xml_bytes.to_vec();
    };
    for &spid in spids {
        let needle = format!(r#"<p:cNvPr id="{spid}""#);
        let mut search_from = 0;
        while let Some(rel) = xml[search_from..].find(&needle) {
            let start = search_from + rel;
            let after = start + needle.len();
            let boundary_ok = xml.as_bytes().get(after).is_some_and(|b| matches!(b, b' ' | b'/' | b'>'));
            if boundary_ok
                && let Some((ShapeOwner::Pic, pic_end)) = owning_shape_end(&xml, start, after)
                && let Some(sp_pr_rel) = xml[after..pic_end].find("<p:spPr>")
            {
                let sp_pr_start = after + sp_pr_rel + "<p:spPr>".len();
                if let Some(sp_pr_end_rel) = xml[sp_pr_start..pic_end].find("</p:spPr>") {
                    let sp_pr_end = sp_pr_start + sp_pr_end_rel;
                    let inner = &xml[sp_pr_start..sp_pr_end];
                    if !inner.contains("<a:grpFill") {
                        let insert_rel = ["<a:ln", "<a:effectLst", "<a:effectDag", "<a:scene3d", "<a:sp3d", "<a:extLst"]
                            .iter()
                            .filter_map(|tag| inner.find(tag))
                            .min()
                            .unwrap_or(sp_pr_end_rel);
                        xml.insert_str(sp_pr_start + insert_rel, "<a:grpFill/>");
                    }
                    break;
                }
            }
            search_from = after;
        }
    }
    xml.into_bytes()
}

fn collect_large_geom_adjs(shapes: &[Shape]) -> Vec<(u32, &[f64])> {
    let mut out = Vec::new();
    assign_spids(shapes, |spid, s| {
        if let deckcraft_model::Geom::Preset { adj, .. } = &s.geom
            && adj.iter().any(|v| v.is_finite() && (v.round() < -2.0e9 || v.round() > 2.0e9))
        {
            out.push((spid, adj.as_slice()));
        }
    });
    out
}

fn patch_shape_av_lst(xml: &mut String, after: usize, shape_end: usize, adj: &[f64]) {
    let Some(av_rel) = xml[after..shape_end].find("<a:avLst>") else { return };
    let av_start = after + av_rel + "<a:avLst>".len();
    let Some(av_end_rel) = xml[av_start..shape_end].find("</a:avLst>") else { return };
    let mut av_end = av_start + av_end_rel;
    for (i, &v) in adj.iter().take(16).enumerate() {
        if !v.is_finite() || (v.round() >= -2.0e9 && v.round() <= 2.0e9) {
            continue;
        }
        let clamped = v.round().clamp(-2.0e9, 2.0e9) as i64;
        let actual = v.round() as i64;
        let single_name = format!(r#"<a:gd name="adj" fmla="val {clamped}"/>"#);
        let idx_name = format!(r#"<a:gd name="adj{}" fmla="val {clamped}"/>"#, i + 1);
        let (target, repl) = if i == 0 && xml[av_start..av_end].contains(&single_name) {
            (single_name, format!(r#"<a:gd name="adj" fmla="val {actual}"/>"#))
        } else {
            (idx_name, format!(r#"<a:gd name="adj{}" fmla="val {actual}"/>"#, i + 1))
        };
        if let Some(rel) = xml[av_start..av_end].find(&target) {
            let pos = av_start + rel;
            xml.replace_range(pos..pos + target.len(), &repl);
            av_end = av_end + repl.len() - target.len();
        }
    }
}

fn patch_large_geom_adjs(xml_bytes: &[u8], adjs: &[(u32, &[f64])]) -> Vec<u8> {
    let Ok(mut xml) = String::from_utf8(xml_bytes.to_vec()) else {
        return xml_bytes.to_vec();
    };
    for &(spid, adj) in adjs {
        let needle = format!(r#"<p:cNvPr id="{spid}""#);
        let mut search_from = 0;
        while let Some(rel) = xml[search_from..].find(&needle) {
            let start = search_from + rel;
            let after = start + needle.len();
            if xml.as_bytes().get(after).is_some_and(|b| matches!(b, b' ' | b'/' | b'>'))
                && let Some((_, shape_end)) = owning_shape_end(&xml, start, after)
            {
                patch_shape_av_lst(&mut xml, after, shape_end, adj);
                break;
            }
            search_from = after;
        }
    }
    xml.into_bytes()
}

fn patch_exported_slides(p: &Presentation, bytes: Vec<u8>) -> Result<Vec<u8>, PptxError> {
    let slide_media: Vec<Vec<(u32, &deckcraft_model::MediaClip)>> = p.slides.iter().map(|s| collect_export_media(&s.shapes)).collect();
    let slide_grp_pics: Vec<std::collections::HashSet<u32>> = p.slides.iter().map(|s| collect_grp_fill_pic_spids(&s.shapes)).collect();
    let slide_adjs: Vec<Vec<(u32, &[f64])>> = p.slides.iter().map(|s| collect_large_geom_adjs(&s.shapes)).collect();
    if slide_media.iter().all(Vec::is_empty) && slide_grp_pics.iter().all(std::collections::HashSet::is_empty) && slide_adjs.iter().all(Vec::is_empty)
    {
        return Ok(bytes);
    }
    let pkg = craft_opc::Package::open(&bytes, &craft_pptx::LIMITS).map_err(|e| PptxError::Write(e.to_string()))?;
    let root_rels = pkg.rels("");
    let pres_path = root_rels.first("officeDocument").map(|r| r.target.clone()).unwrap_or_else(|| "ppt/presentation.xml".into());
    let pres_rels = pkg.rels(&pres_path);
    let mut patches: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    if let Some(pres_bytes) = pkg.get(&pres_path)
        && let Ok(pres_el) = craft_drawingml::xmlx::parse(pres_bytes)
        && let Some(list) = pres_el.child(craft_xml::Local("sldIdLst"))
    {
        for (i, s) in list.children(craft_xml::Local("sldId")).enumerate() {
            let media = slide_media.get(i).map(Vec::as_slice).unwrap_or(&[]);
            let grp_pics = slide_grp_pics.get(i);
            let adjs = slide_adjs.get(i).map(Vec::as_slice).unwrap_or(&[]);
            let need_grp = grp_pics.is_some_and(|g| !g.is_empty());
            if media.is_empty() && !need_grp && adjs.is_empty() {
                continue;
            }
            if let Some(target) = s.attr_local("r:id").and_then(|rid| pres_rels.target(rid))
                && let Some(sb) = pkg.get(target)
            {
                let mut patched = if media.is_empty() { sb.to_vec() } else { patch_slide_media_timing(sb, media) };
                if let Some(g) = grp_pics
                    && !g.is_empty()
                {
                    patched = patch_pic_grp_fill(&patched, g);
                }
                if !adjs.is_empty() {
                    patched = patch_large_geom_adjs(&patched, adjs);
                }
                patches.insert(target.to_string(), patched);
            }
        }
    }
    if patches.is_empty() {
        return Ok(bytes);
    }
    let entries: Vec<(String, Vec<u8>)> = pkg
        .names()
        .filter_map(|name| {
            let data = patches.remove(name).or_else(|| pkg.get(name).map(<[u8]>::to_vec))?;
            Some((name.to_string(), data))
        })
        .collect();
    craft_opc::zip_entries(&entries).map_err(|e| PptxError::Write(e.to_string()))
}

/// Write a `.pptx` package.
pub fn export(p: &Presentation) -> Result<Vec<u8>, PptxError> {
    check_table_caps(p)?;
    let bytes = craft_pptx::export(&convert::pres_to_craft(p), &DeckDefaults)?;
    patch_exported_slides(p, bytes)
}

/// Write a package for `ext`: `potx` (template) and `ppsx` (slide show) differ from `pptx` only in
/// the main part's content type.
pub fn export_as(p: &Presentation, ext: &str) -> Result<Vec<u8>, PptxError> {
    check_table_caps(p)?;
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let bytes = craft_pptx::export_as(&convert::pres_to_craft(p), &DeckDefaults, &ext)?;
    patch_exported_slides(p, bytes)
}

#[cfg(test)]
mod import_theme_tests {
    use std::io::{Read, Write};

    use deckcraft_model::Presentation;

    fn custom() -> Presentation {
        let mut p = Presentation::default();
        let m = std::sync::Arc::make_mut(&mut p.masters[0]);
        m.theme.name = "Harbour".into();
        m.theme.fonts.major.latin = "Lato".into();
        m.theme.fonts.minor.latin = "Source Sans 3".into();
        m.theme.colors.set(deckcraft_color::SchemeSlot::Accent1, deckcraft_color::Rgba::rgb(0x12, 0x34, 0x56));
        p
    }

    /// A `.thmx`: the deck's theme part as the package's main document.
    fn thmx(pptx: &[u8]) -> Vec<u8> {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(pptx)).unwrap();
        let mut theme = String::new();
        z.by_name("ppt/theme/theme1.xml").unwrap().read_to_string(&mut theme).unwrap();
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        out.start_file("[Content_Types].xml", o).unwrap();
        out.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/theme/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/></Types>"#).unwrap();
        out.start_file("_rels/.rels", o).unwrap();
        out.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="theme/theme/theme1.xml"/></Relationships>"#).unwrap();
        out.start_file("theme/theme/theme1.xml", o).unwrap();
        out.write_all(theme.as_bytes()).unwrap();
        out.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_the_theme_of_a_pptx_and_of_a_thmx() {
        let pptx = crate::export(&custom()).unwrap();
        for bytes in [pptx.clone(), thmx(&pptx)] {
            let (t, _) = crate::import_theme(&bytes).unwrap();
            assert_eq!(t.name, "Harbour");
            assert_eq!(t.fonts.major.latin, "Lato");
            assert_eq!(t.fonts.minor.latin, "Source Sans 3");
            assert_eq!(t.colors.get(deckcraft_color::SchemeSlot::Accent1), deckcraft_color::Rgba::rgb(0x12, 0x34, 0x56));
        }
    }

    #[test]
    fn non_theme_packages_are_errors() {
        assert!(crate::import_theme(b"not a zip").is_err());
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        out.start_file("[Content_Types].xml", zip::write::SimpleFileOptions::default()).unwrap();
        out.write_all(b"<Types/>").unwrap();
        assert!(crate::import_theme(&out.finish().unwrap().into_inner()).is_err());
    }
}

#[cfg(test)]
mod table_direction_tests {
    use deckcraft_model::table::{Cell, Table};
    use deckcraft_model::{Presentation, Shape, ShapeId, ShapeKind, defaults};
    use std::sync::Arc;

    fn table(p: &Presentation) -> &Table {
        p.slides[0]
            .shapes
            .iter()
            .find_map(|shape| match &shape.kind {
                ShapeKind::Table(table) => Some(table),
                _ => None,
            })
            .expect("table shape")
    }

    #[test]
    fn table_direction_and_column_order_survive_two_pptx_saves() {
        for rtl in [Some(true), Some(false), None] {
            let mut p = defaults::new_presentation(None);
            let mut t = Table::new(1, 2, 200.0, 30.0);
            t.rtl = rtl;
            t.rows[0].cells[0] = Cell::new("First column");
            t.rows[0].cells[1] = Cell::new("Second column");
            let shape = Shape { id: ShapeId(p.alloc_id().expect("id")), kind: ShapeKind::Table(t), ..Default::default() };
            Arc::make_mut(&mut p.slides[0]).shapes.push(shape);

            let mut roundtrip = crate::import(&crate::export(&p).expect("export")).expect("import");
            for _ in 0..2 {
                let t = table(&roundtrip);
                assert_eq!(t.rtl, rtl);
                assert_eq!(t.rows[0].cells[0].text.paragraphs[0].runs[0].text, "First column");
                assert_eq!(t.rows[0].cells[1].text.paragraphs[0].runs[0].text, "Second column");
                roundtrip = crate::import(&crate::export(&roundtrip).expect("export again")).expect("import again");
            }
        }
    }

    #[test]
    fn over_cap_table_export_is_refused() {
        let mut p = defaults::new_presentation(None);
        let mut t = Table::new(1, 2, 200.0, 30.0);
        t.rows.resize_with(deckcraft_model::table::MAX_CELLS / 2 + 1, || deckcraft_model::table::Row {
            height: 20.0,
            cells: vec![Cell::new("x"), Cell::new("y")],
        });
        let shape = Shape { id: ShapeId(p.alloc_id().expect("id")), kind: ShapeKind::Table(t), ..Default::default() };
        Arc::make_mut(&mut p.slides[0]).shapes.push(shape);
        let err = crate::export(&p).expect_err("over-cap table must be refused");
        assert!(matches!(err, crate::PptxError::Unsupported(_)));
    }

    #[test]
    fn over_cap_table_on_notes_and_handout_master_is_refused() {
        let mut t = Table::new(1, 2, 200.0, 30.0);
        t.rows.resize_with(deckcraft_model::table::MAX_CELLS / 2 + 1, || deckcraft_model::table::Row {
            height: 20.0,
            cells: vec![Cell::new("x"), Cell::new("y")],
        });
        let mut p_notes = defaults::new_presentation(None);
        let shape = Shape { id: ShapeId(p_notes.alloc_id().expect("id")), kind: ShapeKind::Table(t.clone()), ..Default::default() };
        p_notes.notes_master = Some(Arc::new(deckcraft_model::Master { name: "Notes".into(), shapes: vec![shape], ..Default::default() }));
        assert!(matches!(crate::export(&p_notes), Err(crate::PptxError::Unsupported(_))));

        let mut p_handout = defaults::new_presentation(None);
        let shape = Shape { id: ShapeId(p_handout.alloc_id().expect("id")), kind: ShapeKind::Table(t), ..Default::default() };
        p_handout.handout_master = Some(Arc::new(deckcraft_model::Master { name: "Handout".into(), shapes: vec![shape], ..Default::default() }));
        assert!(matches!(crate::export(&p_handout), Err(crate::PptxError::Unsupported(_))));
    }
}

#[cfg(test)]
mod shape_owner_scoping_tests {
    use std::collections::HashSet;

    #[test]
    fn patch_pic_grp_fill_does_not_cross_into_following_pic() {
        let xml = concat!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="Box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/></p:sp>"#,
            r#"<p:pic><p:nvPicPr><p:cNvPr id="6" name="Pic"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:spPr><a:ln/></p:spPr></p:pic>"#,
        );
        let spids = HashSet::from([5]);
        let out = String::from_utf8(super::patch_pic_grp_fill(xml.as_bytes(), &spids)).expect("utf8");
        assert!(!out.contains("<a:grpFill/>"), "non-pic owner id=5 must not patch pic id=6: {out}");
    }

    #[test]
    fn patch_large_geom_adjs_does_not_leak_from_group_into_child_shape() {
        let xml = concat!(
            r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="10" name="Group"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>"#,
            r#"<p:sp><p:nvSpPr><p:cNvPr id="11" name="Child"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>"#,
            r#"<p:spPr><a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val 2000000000"/></a:avLst></a:prstGeom></p:spPr></p:sp>"#,
            r#"</p:grpSp>"#,
        );
        let adjs = [(10_u32, &[3_000_000_000.0_f64][..])];
        let out = String::from_utf8(super::patch_large_geom_adjs(xml.as_bytes(), &adjs)).expect("utf8");
        assert!(out.contains(r#"fmla="val 2000000000""#), "group id=10 must not patch child id=11: {out}");

        let child_adjs = [(11_u32, &[3_000_000_000.0_f64][..])];
        let patched = String::from_utf8(super::patch_large_geom_adjs(xml.as_bytes(), &child_adjs)).expect("utf8");
        assert!(patched.contains(r#"fmla="val 3000000000""#), "child id=11 should be patched: {patched}");
    }
}
