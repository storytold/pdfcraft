//! Bidirectional conversion between DeckCraft's document model (`deckcraft_model`) and the
//! shared PresentationML/DrawingML model (`craft_pptx` / `craft_drawingml`).

use std::sync::Arc;

use craft_drawingml::chart as cd_chart;
use craft_drawingml::diagram as cd_dgm;
use craft_drawingml::style as cd_style;
use craft_drawingml::table as cd_table;
use craft_drawingml::text as cd_text;
use craft_drawingml::theme as cd_theme;
use craft_pptx::anim as cp_anim;
use deckcraft_model::anim as dm_anim;
use deckcraft_model::chart as dm_chart;
use deckcraft_model::style as dm_style;
use deckcraft_model::table as dm_table;
use deckcraft_model::text as dm_text;
use deckcraft_model::theme as dm_theme;

// ---------------------------------------------------------------------------
// IDs
// ---------------------------------------------------------------------------

fn media_id_to(id: deckcraft_model::MediaId) -> craft_pptx::MediaId {
    craft_pptx::MediaId(id.0)
}
fn media_id_from(id: craft_pptx::MediaId) -> deckcraft_model::MediaId {
    deckcraft_model::MediaId(id.0)
}
fn slide_id_to(id: deckcraft_model::SlideId) -> craft_pptx::SlideId {
    craft_pptx::SlideId(id.0)
}
fn slide_id_from(id: craft_pptx::SlideId) -> deckcraft_model::SlideId {
    deckcraft_model::SlideId(id.0)
}
fn shape_id_to(id: deckcraft_model::ShapeId) -> craft_pptx::ShapeId {
    craft_pptx::ShapeId(id.0)
}
fn shape_id_from(id: craft_pptx::ShapeId) -> deckcraft_model::ShapeId {
    deckcraft_model::ShapeId(id.0)
}
fn layout_id_to(id: deckcraft_model::LayoutId) -> craft_pptx::LayoutId {
    craft_pptx::LayoutId(id.0)
}
fn layout_id_from(id: craft_pptx::LayoutId) -> deckcraft_model::LayoutId {
    deckcraft_model::LayoutId(id.0)
}
fn master_id_to(id: deckcraft_model::MasterId) -> craft_pptx::MasterId {
    craft_pptx::MasterId(id.0)
}
fn master_id_from(id: craft_pptx::MasterId) -> deckcraft_model::MasterId {
    deckcraft_model::MasterId(id.0)
}

// ---------------------------------------------------------------------------
// Style
// ---------------------------------------------------------------------------

fn color_base_to(b: &dm_style::ColorBase) -> cd_style::ColorBase {
    match b {
        dm_style::ColorBase::Rgb { rgb } => cd_style::ColorBase::Rgb { rgb: *rgb },
        dm_style::ColorBase::Scheme { slot } => cd_style::ColorBase::Scheme { slot: *slot },
        dm_style::ColorBase::Preset { name } => cd_style::ColorBase::Preset { name: name.clone() },
        dm_style::ColorBase::System { name, last } => cd_style::ColorBase::System { name: name.clone(), last: *last },
    }
}

fn color_base_from(b: cd_style::ColorBase) -> dm_style::ColorBase {
    match b {
        cd_style::ColorBase::Rgb { rgb } => dm_style::ColorBase::Rgb { rgb },
        cd_style::ColorBase::Scheme { slot } => dm_style::ColorBase::Scheme { slot },
        cd_style::ColorBase::Preset { name } => dm_style::ColorBase::Preset { name },
        cd_style::ColorBase::System { name, last } => dm_style::ColorBase::System { name, last },
    }
}

fn color_ref_to(c: &dm_style::ColorRef) -> cd_style::ColorRef {
    cd_style::ColorRef { base: color_base_to(&c.base), mods: c.mods.clone() }
}

fn color_ref_from(c: cd_style::ColorRef) -> dm_style::ColorRef {
    dm_style::ColorRef { base: color_base_from(c.base), mods: c.mods }
}

fn gradient_flip_to(f: dm_style::GradientFlip) -> cd_style::GradientFlip {
    match f {
        dm_style::GradientFlip::None => cd_style::GradientFlip::None,
        dm_style::GradientFlip::X => cd_style::GradientFlip::X,
        dm_style::GradientFlip::Y => cd_style::GradientFlip::Y,
        dm_style::GradientFlip::Xy => cd_style::GradientFlip::Xy,
    }
}

fn gradient_flip_from(f: cd_style::GradientFlip) -> dm_style::GradientFlip {
    match f {
        cd_style::GradientFlip::None => dm_style::GradientFlip::None,
        cd_style::GradientFlip::X => dm_style::GradientFlip::X,
        cd_style::GradientFlip::Y => dm_style::GradientFlip::Y,
        cd_style::GradientFlip::Xy => dm_style::GradientFlip::Xy,
    }
}

fn gradient_shape_to(s: &dm_style::GradientShape) -> cd_style::GradientShape {
    match s {
        dm_style::GradientShape::Linear { angle, scaled } => cd_style::GradientShape::Linear { angle: *angle, scaled: *scaled },
        dm_style::GradientShape::Path { path, focus } => cd_style::GradientShape::Path { path: path.clone(), focus: *focus },
    }
}

fn gradient_shape_from(s: cd_style::GradientShape) -> dm_style::GradientShape {
    match s {
        cd_style::GradientShape::Linear { angle, scaled } => dm_style::GradientShape::Linear { angle, scaled },
        cd_style::GradientShape::Path { path, focus } => dm_style::GradientShape::Path { path, focus },
    }
}

fn gradient_to(g: &dm_style::Gradient) -> cd_style::Gradient {
    cd_style::Gradient {
        stops: g.stops.iter().map(|s| cd_style::GradientStop { pos: s.pos, color: color_ref_to(&s.color) }).collect(),
        shape: Some(gradient_shape_to(&g.shape)),
        flip: g.flip.map(gradient_flip_to),
        rotate_with_shape: Some(g.rotate_with_shape),
        tile: g.tile,
    }
}

fn gradient_from(g: cd_style::Gradient) -> dm_style::Gradient {
    dm_style::Gradient {
        stops: g.stops.into_iter().map(|s| dm_style::GradientStop { pos: s.pos, color: color_ref_from(s.color) }).collect(),
        shape: g.shape.map(gradient_shape_from).unwrap_or(dm_style::GradientShape::Linear { angle: 0.0, scaled: false }),
        flip: g.flip.map(gradient_flip_from),
        rotate_with_shape: g.rotate_with_shape.unwrap_or(true),
        tile: g.tile,
    }
}

fn picture_mode_to(m: &dm_style::PictureMode) -> cd_style::PictureMode {
    match m {
        dm_style::PictureMode::Stretch { fill_rect } => cd_style::PictureMode::Stretch { fill_rect: *fill_rect },
        dm_style::PictureMode::Tile { tx, ty, sx, sy, flip, align } => {
            cd_style::PictureMode::Tile { tx: *tx, ty: *ty, sx: *sx, sy: *sy, flip: flip.clone(), align: align.clone() }
        }
    }
}

fn picture_mode_from(m: cd_style::PictureMode) -> dm_style::PictureMode {
    match m {
        cd_style::PictureMode::Stretch { fill_rect } => dm_style::PictureMode::Stretch { fill_rect },
        cd_style::PictureMode::Tile { tx, ty, sx, sy, flip, align } => dm_style::PictureMode::Tile { tx, ty, sx, sy, flip, align },
    }
}

fn picture_adjust_to(a: &dm_style::PictureAdjust) -> cd_style::PictureAdjust {
    cd_style::PictureAdjust {
        brightness: a.brightness,
        contrast: a.contrast,
        saturation: a.saturation,
        grayscale: a.grayscale,
        duotone: a.duotone.as_ref().map(|(d, l)| (color_ref_to(d), color_ref_to(l))),
        clear_color: a.clear_color,
        sharpen: a.sharpen,
        bilevel: a.bilevel,
        alpha_inv: a.alpha_inv,
    }
}

fn picture_adjust_from(a: cd_style::PictureAdjust) -> dm_style::PictureAdjust {
    dm_style::PictureAdjust {
        brightness: a.brightness,
        contrast: a.contrast,
        saturation: a.saturation,
        grayscale: a.grayscale,
        duotone: a.duotone.map(|(d, l)| (color_ref_from(d), color_ref_from(l))),
        clear_color: a.clear_color,
        sharpen: a.sharpen,
        bilevel: a.bilevel,
        alpha_inv: a.alpha_inv,
    }
}

fn picture_fill_to(p: &dm_style::PictureFill) -> cd_style::PictureFill {
    cd_style::PictureFill {
        media: media_id_to(p.media),
        svg: None,
        crop: p.crop,
        mode: picture_mode_to(&p.mode),
        alpha: p.alpha,
        adjust: picture_adjust_to(&p.adjust),
        upright: p.upright,
        artistic: p.artistic.as_ref().map(|a| cd_style::Artistic { effect: a.effect.clone(), original: media_id_to(a.original) }),
    }
}

fn picture_fill_from(p: cd_style::PictureFill) -> dm_style::PictureFill {
    dm_style::PictureFill {
        media: media_id_from(p.media),
        crop: p.crop,
        mode: picture_mode_from(p.mode),
        alpha: p.alpha,
        adjust: picture_adjust_from(p.adjust),
        upright: p.upright,
        artistic: p.artistic.map(|a| dm_style::Artistic { effect: a.effect, original: media_id_from(a.original) }),
    }
}

fn fill_to(f: &dm_style::Fill) -> cd_style::Fill {
    match f {
        dm_style::Fill::None => cd_style::Fill::None,
        dm_style::Fill::Solid { color } => cd_style::Fill::Solid { color: color_ref_to(color) },
        dm_style::Fill::Gradient(g) => cd_style::Fill::Gradient(gradient_to(g)),
        dm_style::Fill::Picture(p) => cd_style::Fill::Picture(picture_fill_to(p)),
        dm_style::Fill::Pattern(p) => {
            cd_style::Fill::Pattern(cd_style::PatternFill { preset: p.preset.clone(), fg: color_ref_to(&p.fg), bg: color_ref_to(&p.bg) })
        }
        dm_style::Fill::Group => cd_style::Fill::Group,
        dm_style::Fill::Background => cd_style::Fill::Background,
    }
}

fn fill_from(f: cd_style::Fill) -> dm_style::Fill {
    match f {
        cd_style::Fill::None => dm_style::Fill::None,
        cd_style::Fill::Solid { color } => dm_style::Fill::Solid { color: color_ref_from(color) },
        cd_style::Fill::Gradient(g) => dm_style::Fill::Gradient(gradient_from(g)),
        cd_style::Fill::Picture(p) => dm_style::Fill::Picture(picture_fill_from(p)),
        cd_style::Fill::Pattern(p) => {
            dm_style::Fill::Pattern(dm_style::PatternFill { preset: p.preset, fg: color_ref_from(p.fg), bg: color_ref_from(p.bg) })
        }
        cd_style::Fill::Group => dm_style::Fill::Group,
        cd_style::Fill::Background => dm_style::Fill::Background,
    }
}

fn line_to(l: &dm_style::Line) -> cd_style::Line {
    cd_style::Line {
        fill: l.fill.as_ref().map(fill_to),
        width: l.width,
        dash: l.dash.as_ref().map(|d| match d {
            dm_style::Dash::Preset { name } => cd_style::Dash::Preset { name: name.clone() },
            dm_style::Dash::Custom { pattern } => cd_style::Dash::Custom { pattern: pattern.clone() },
        }),
        cap: l.cap.map(|c| match c {
            dm_style::LineCap::Flat => cd_style::LineCap::Flat,
            dm_style::LineCap::Round => cd_style::LineCap::Round,
            dm_style::LineCap::Square => cd_style::LineCap::Square,
        }),
        join: l.join.map(|j| match j {
            dm_style::LineJoin::Round => cd_style::LineJoin::Round,
            dm_style::LineJoin::Bevel => cd_style::LineJoin::Bevel,
            dm_style::LineJoin::Miter => cd_style::LineJoin::Miter,
        }),
        miter_limit: l.miter_limit,
        align: l.align.map(|a| match a {
            dm_style::LineAlign::Center => cd_style::LineAlign::Center,
            dm_style::LineAlign::Inside => cd_style::LineAlign::Inside,
        }),
        compound: l.compound.map(|c| match c {
            dm_style::Compound::Single => cd_style::Compound::Single,
            dm_style::Compound::Double => cd_style::Compound::Double,
            dm_style::Compound::ThickThin => cd_style::Compound::ThickThin,
            dm_style::Compound::ThinThick => cd_style::Compound::ThinThick,
            dm_style::Compound::Triple => cd_style::Compound::Triple,
        }),
        head: l.head.as_ref().map(|e| cd_style::LineEnd { kind: e.kind.clone(), w: e.w.clone(), len: e.len.clone() }),
        tail: l.tail.as_ref().map(|e| cd_style::LineEnd { kind: e.kind.clone(), w: e.w.clone(), len: e.len.clone() }),
    }
}

fn line_from(l: cd_style::Line) -> dm_style::Line {
    dm_style::Line {
        fill: l.fill.map(fill_from),
        width: l.width,
        dash: l.dash.map(|d| match d {
            cd_style::Dash::Preset { name } => dm_style::Dash::Preset { name },
            cd_style::Dash::Custom { pattern } => dm_style::Dash::Custom { pattern },
        }),
        cap: l.cap.map(|c| match c {
            cd_style::LineCap::Flat => dm_style::LineCap::Flat,
            cd_style::LineCap::Round => dm_style::LineCap::Round,
            cd_style::LineCap::Square => dm_style::LineCap::Square,
        }),
        join: l.join.map(|j| match j {
            cd_style::LineJoin::Round => dm_style::LineJoin::Round,
            cd_style::LineJoin::Bevel => dm_style::LineJoin::Bevel,
            cd_style::LineJoin::Miter => dm_style::LineJoin::Miter,
        }),
        miter_limit: l.miter_limit,
        align: l.align.map(|a| match a {
            cd_style::LineAlign::Center => dm_style::LineAlign::Center,
            cd_style::LineAlign::Inside => dm_style::LineAlign::Inside,
        }),
        compound: l.compound.map(|c| match c {
            cd_style::Compound::Single => dm_style::Compound::Single,
            cd_style::Compound::Double => dm_style::Compound::Double,
            cd_style::Compound::ThickThin => dm_style::Compound::ThickThin,
            cd_style::Compound::ThinThick => dm_style::Compound::ThinThick,
            cd_style::Compound::Triple => dm_style::Compound::Triple,
        }),
        head: l.head.map(|e| dm_style::LineEnd { kind: e.kind, w: e.w, len: e.len }),
        tail: l.tail.map(|e| dm_style::LineEnd { kind: e.kind, w: e.w, len: e.len }),
    }
}

fn shadow_to(s: &dm_style::Shadow) -> cd_style::Shadow {
    cd_style::Shadow {
        color: color_ref_to(&s.color),
        blur: s.blur,
        dist: s.dist,
        dir: s.dir,
        inner: s.inner,
        sx: s.sx,
        sy: s.sy,
        kx: s.kx,
        ky: s.ky,
        align: s.align.clone(),
        rotate_with_shape: s.rotate_with_shape,
    }
}

fn shadow_from(s: cd_style::Shadow) -> dm_style::Shadow {
    dm_style::Shadow {
        color: color_ref_from(s.color),
        blur: s.blur,
        dist: s.dist,
        dir: s.dir,
        inner: s.inner,
        sx: s.sx,
        sy: s.sy,
        kx: s.kx,
        ky: s.ky,
        align: s.align,
        rotate_with_shape: s.rotate_with_shape,
    }
}

fn glow_to(g: &dm_style::Glow) -> cd_style::Glow {
    cd_style::Glow { color: color_ref_to(&g.color), radius: g.radius }
}

fn glow_from(g: cd_style::Glow) -> dm_style::Glow {
    dm_style::Glow { color: color_ref_from(g.color), radius: g.radius }
}

fn reflection_to(r: &dm_style::Reflection) -> cd_style::Reflection {
    cd_style::Reflection {
        blur: r.blur,
        start_alpha: r.start_alpha,
        end_alpha: r.end_alpha,
        end_pos: r.end_pos,
        dist: r.dist,
        st_pos: r.st_pos,
        dir: r.dir,
        fade_dir: r.fade_dir,
        sx: r.sx,
        sy: r.sy,
        kx: r.kx,
        ky: r.ky,
        align: r.align.clone(),
        rotate_with_shape: r.rotate_with_shape,
    }
}

fn reflection_from(r: cd_style::Reflection) -> dm_style::Reflection {
    dm_style::Reflection {
        blur: r.blur,
        start_alpha: r.start_alpha,
        end_alpha: r.end_alpha,
        end_pos: r.end_pos,
        dist: r.dist,
        st_pos: r.st_pos,
        dir: r.dir,
        fade_dir: r.fade_dir,
        sx: r.sx,
        sy: r.sy,
        kx: r.kx,
        ky: r.ky,
        align: r.align,
        rotate_with_shape: r.rotate_with_shape,
    }
}

fn effects_to(e: &dm_style::Effects) -> cd_style::Effects {
    cd_style::Effects {
        outer_shadow: e.outer_shadow.as_ref().map(shadow_to),
        inner_shadow: e.inner_shadow.as_ref().map(shadow_to),
        glow: e.glow.as_ref().map(glow_to),
        soft_edge: e.soft_edge,
        reflection: e.reflection.as_ref().map(reflection_to),
        bevel: e.bevel.clone(),
        raw3d: e.raw3d.clone(),
        fill_overlay: e.fill_overlay.as_ref().map(|o| cd_style::FillOverlay { blend: o.blend.clone(), fill: fill_to(&o.fill) }),
    }
}

fn effects_from(e: cd_style::Effects) -> dm_style::Effects {
    dm_style::Effects {
        outer_shadow: e.outer_shadow.map(shadow_from),
        inner_shadow: e.inner_shadow.map(shadow_from),
        glow: e.glow.map(glow_from),
        soft_edge: e.soft_edge,
        reflection: e.reflection.map(reflection_from),
        bevel: e.bevel,
        raw3d: e.raw3d,
        fill_overlay: e.fill_overlay.map(|o| dm_style::FillOverlay { blend: o.blend, fill: fill_from(o.fill) }),
    }
}

fn shape_style_to(s: &dm_style::ShapeStyle) -> cd_style::ShapeStyle {
    cd_style::ShapeStyle {
        line_ref: (s.line_ref.0, color_ref_to(&s.line_ref.1)),
        fill_ref: (s.fill_ref.0, color_ref_to(&s.fill_ref.1)),
        effect_ref: (s.effect_ref.0, color_ref_to(&s.effect_ref.1)),
        font_ref: (s.font_ref.0.clone(), s.font_ref.1.as_ref().map(color_ref_to)),
    }
}

fn shape_style_from(s: cd_style::ShapeStyle) -> dm_style::ShapeStyle {
    dm_style::ShapeStyle {
        line_ref: (s.line_ref.0, color_ref_from(s.line_ref.1)),
        fill_ref: (s.fill_ref.0, color_ref_from(s.fill_ref.1)),
        effect_ref: (s.effect_ref.0, color_ref_from(s.effect_ref.1)),
        font_ref: (s.font_ref.0, s.font_ref.1.map(color_ref_from)),
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

fn anchor_to(a: dm_text::Anchor) -> cd_text::Anchor {
    match a {
        dm_text::Anchor::Top => cd_text::Anchor::Top,
        dm_text::Anchor::Middle => cd_text::Anchor::Middle,
        dm_text::Anchor::Bottom => cd_text::Anchor::Bottom,
        dm_text::Anchor::Justified => cd_text::Anchor::Justified,
        dm_text::Anchor::Distributed => cd_text::Anchor::Distributed,
    }
}

fn anchor_from(a: cd_text::Anchor) -> dm_text::Anchor {
    match a {
        cd_text::Anchor::Top => dm_text::Anchor::Top,
        cd_text::Anchor::Middle => dm_text::Anchor::Middle,
        cd_text::Anchor::Bottom => dm_text::Anchor::Bottom,
        cd_text::Anchor::Justified => dm_text::Anchor::Justified,
        cd_text::Anchor::Distributed => dm_text::Anchor::Distributed,
    }
}

fn text_dir_to(d: dm_text::TextDir) -> cd_text::TextDir {
    match d {
        dm_text::TextDir::Horizontal => cd_text::TextDir::Horizontal,
        dm_text::TextDir::Vertical => cd_text::TextDir::Vertical,
        dm_text::TextDir::Vertical270 => cd_text::TextDir::Vertical270,
        dm_text::TextDir::Stacked => cd_text::TextDir::Stacked,
        dm_text::TextDir::EaVertical => cd_text::TextDir::EaVertical,
        dm_text::TextDir::MongolianVertical => cd_text::TextDir::MongolianVertical,
        dm_text::TextDir::StackedRtl => cd_text::TextDir::StackedRtl,
    }
}

fn text_dir_from(d: cd_text::TextDir) -> dm_text::TextDir {
    match d {
        cd_text::TextDir::Horizontal => dm_text::TextDir::Horizontal,
        cd_text::TextDir::Vertical => dm_text::TextDir::Vertical,
        cd_text::TextDir::Vertical270 => dm_text::TextDir::Vertical270,
        cd_text::TextDir::Stacked => dm_text::TextDir::Stacked,
        cd_text::TextDir::EaVertical => dm_text::TextDir::EaVertical,
        cd_text::TextDir::MongolianVertical => dm_text::TextDir::MongolianVertical,
        cd_text::TextDir::StackedRtl => dm_text::TextDir::StackedRtl,
    }
}

fn text_overflow_to(o: dm_text::TextOverflow) -> cd_text::TextOverflow {
    match o {
        dm_text::TextOverflow::Overflow => cd_text::TextOverflow::Overflow,
        dm_text::TextOverflow::Clip => cd_text::TextOverflow::Clip,
        dm_text::TextOverflow::Ellipsis => cd_text::TextOverflow::Ellipsis,
    }
}

fn text_overflow_from(o: cd_text::TextOverflow) -> dm_text::TextOverflow {
    match o {
        cd_text::TextOverflow::Overflow => dm_text::TextOverflow::Overflow,
        cd_text::TextOverflow::Clip => dm_text::TextOverflow::Clip,
        cd_text::TextOverflow::Ellipsis => dm_text::TextOverflow::Ellipsis,
    }
}

fn body_props_to(b: &dm_text::BodyProps) -> cd_text::BodyProps {
    cd_text::BodyProps {
        inset_l: b.inset_l,
        inset_t: b.inset_t,
        inset_r: b.inset_r,
        inset_b: b.inset_b,
        anchor: b.anchor.map(anchor_to),
        anchor_ctr: b.anchor_ctr,
        wrap: b.wrap,
        autofit: b.autofit.map(|a| match a {
            dm_text::AutoFit::None => cd_text::AutoFit::None,
            dm_text::AutoFit::Shrink { font_scale, line_reduction } => cd_text::AutoFit::Shrink { font_scale, line_reduction },
            dm_text::AutoFit::Shape => cd_text::AutoFit::Shape,
        }),
        vert: b.vert.map(text_dir_to),
        rot: b.rot,
        columns: b.columns,
        col_spacing: b.col_spacing,
        warp: b.warp.clone(),
        warp_adj: b.warp_adj.clone(),
        rtl_col: b.rtl_col,
        upright: b.upright,
        space_first_last: b.space_first_last,
        compat_ln_spc: b.compat_ln_spc,
        vert_overflow: b.vert_overflow.map(text_overflow_to),
        horz_overflow: b.horz_overflow.map(text_overflow_to),
        raw3d: b.raw3d.clone(),
    }
}

fn body_props_from(b: cd_text::BodyProps) -> dm_text::BodyProps {
    dm_text::BodyProps {
        inset_l: b.inset_l,
        inset_t: b.inset_t,
        inset_r: b.inset_r,
        inset_b: b.inset_b,
        anchor: b.anchor.map(anchor_from),
        anchor_ctr: b.anchor_ctr,
        wrap: b.wrap,
        autofit: b.autofit.map(|a| match a {
            cd_text::AutoFit::None => dm_text::AutoFit::None,
            cd_text::AutoFit::Shrink { font_scale, line_reduction } => dm_text::AutoFit::Shrink { font_scale, line_reduction },
            cd_text::AutoFit::Shape => dm_text::AutoFit::Shape,
        }),
        vert: b.vert.map(text_dir_from),
        rot: b.rot,
        columns: b.columns,
        col_spacing: b.col_spacing,
        warp: b.warp,
        warp_adj: b.warp_adj,
        rtl_col: b.rtl_col,
        upright: b.upright,
        space_first_last: b.space_first_last,
        compat_ln_spc: b.compat_ln_spc,
        vert_overflow: b.vert_overflow.map(text_overflow_from),
        horz_overflow: b.horz_overflow.map(text_overflow_from),
        raw3d: b.raw3d,
    }
}

fn align_to(a: dm_text::Align) -> cd_text::Align {
    match a {
        dm_text::Align::Left => cd_text::Align::Left,
        dm_text::Align::Center => cd_text::Align::Center,
        dm_text::Align::Right => cd_text::Align::Right,
        dm_text::Align::Justify => cd_text::Align::Justify,
        dm_text::Align::JustLow => cd_text::Align::JustLow,
        dm_text::Align::Distributed => cd_text::Align::Distributed,
        dm_text::Align::ThaiDistributed => cd_text::Align::ThaiDistributed,
    }
}

fn align_from(a: cd_text::Align) -> dm_text::Align {
    match a {
        cd_text::Align::Left => dm_text::Align::Left,
        cd_text::Align::Center => dm_text::Align::Center,
        cd_text::Align::Right => dm_text::Align::Right,
        cd_text::Align::Justify => dm_text::Align::Justify,
        cd_text::Align::JustLow => dm_text::Align::JustLow,
        cd_text::Align::Distributed => dm_text::Align::Distributed,
        cd_text::Align::ThaiDistributed => dm_text::Align::ThaiDistributed,
    }
}

fn spacing_to(s: dm_text::Spacing) -> cd_text::Spacing {
    match s {
        dm_text::Spacing::Pct(v) => cd_text::Spacing::Pct(v),
        dm_text::Spacing::Pts(v) => cd_text::Spacing::Pts(v),
    }
}

fn spacing_from(s: cd_text::Spacing) -> dm_text::Spacing {
    match s {
        cd_text::Spacing::Pct(v) => dm_text::Spacing::Pct(v),
        cd_text::Spacing::Pts(v) => dm_text::Spacing::Pts(v),
    }
}

fn bullet_to(b: &dm_text::Bullet) -> cd_text::Bullet {
    match b {
        dm_text::Bullet::None => cd_text::Bullet::None,
        dm_text::Bullet::Char { char } => cd_text::Bullet::Char { char: char.clone() },
        dm_text::Bullet::AutoNum { scheme, start_at } => cd_text::Bullet::AutoNum { scheme: scheme.clone(), start_at: *start_at },
        dm_text::Bullet::Picture { media } => cd_text::Bullet::Picture { media: media_id_to(*media) },
    }
}

fn bullet_from(b: cd_text::Bullet) -> dm_text::Bullet {
    match b {
        cd_text::Bullet::None => dm_text::Bullet::None,
        cd_text::Bullet::Char { char } => dm_text::Bullet::Char { char },
        cd_text::Bullet::AutoNum { scheme, start_at } => dm_text::Bullet::AutoNum { scheme, start_at },
        cd_text::Bullet::Picture { media } => dm_text::Bullet::Picture { media: media_id_from(media) },
    }
}

fn para_props_to(p: &dm_text::ParaProps) -> cd_text::ParaProps {
    cd_text::ParaProps {
        align: p.align.map(align_to),
        margin_left: p.margin_left,
        margin_right: p.margin_right,
        indent: p.indent,
        line_spacing: p.line_spacing.map(spacing_to),
        space_before: p.space_before.map(spacing_to),
        space_after: p.space_after.map(spacing_to),
        bullet: p.bullet.as_ref().map(bullet_to),
        bullet_font: p.bullet_font.clone(),
        bullet_font_follow_text: p.bullet_font_follow_text,
        bullet_color: p.bullet_color.as_ref().map(color_ref_to),
        bullet_color_follow_text: p.bullet_color_follow_text,
        bullet_size: p.bullet_size,
        bullet_size_points: p.bullet_size_points,
        bullet_size_follow_text: p.bullet_size_follow_text,
        tabs: p.tabs.as_ref().map(|ts| ts.iter().map(|t| cd_text::TabStop { pos: t.pos, align: t.align.clone() }).collect()),
        default_tab: p.default_tab,
        rtl: p.rtl,
        font_align: p.font_align.clone(),
        east_asian_line_break: p.east_asian_line_break,
        latin_line_break: p.latin_line_break,
        hanging_punct: p.hanging_punct,
    }
}

fn para_props_from(p: cd_text::ParaProps) -> dm_text::ParaProps {
    dm_text::ParaProps {
        align: p.align.map(align_from),
        margin_left: p.margin_left,
        margin_right: p.margin_right,
        indent: p.indent,
        line_spacing: p.line_spacing.map(spacing_from),
        space_before: p.space_before.map(spacing_from),
        space_after: p.space_after.map(spacing_from),
        bullet: p.bullet.map(bullet_from),
        bullet_font: p.bullet_font,
        bullet_font_follow_text: p.bullet_font_follow_text,
        bullet_color: p.bullet_color.map(color_ref_from),
        bullet_color_follow_text: p.bullet_color_follow_text,
        bullet_size: p.bullet_size,
        bullet_size_points: p.bullet_size_points,
        bullet_size_follow_text: p.bullet_size_follow_text,
        tabs: p.tabs.map(|ts| ts.into_iter().map(|t| dm_text::TabStop { pos: t.pos, align: t.align }).collect()),
        default_tab: p.default_tab,
        rtl: p.rtl,
        font_align: p.font_align,
        east_asian_line_break: p.east_asian_line_break,
        latin_line_break: p.latin_line_break,
        hanging_punct: p.hanging_punct,
    }
}

fn action_to(a: &dm_text::Action) -> cd_text::Action {
    match a {
        dm_text::Action::Url { url } => cd_text::Action::Url { url: url.clone() },
        dm_text::Action::Slide { slide } => cd_text::Action::Slide { slide: slide_id_to(*slide) },
        dm_text::Action::NextSlide => cd_text::Action::NextSlide,
        dm_text::Action::PreviousSlide => cd_text::Action::PreviousSlide,
        dm_text::Action::FirstSlide => cd_text::Action::FirstSlide,
        dm_text::Action::LastSlide => cd_text::Action::LastSlide,
        dm_text::Action::EndShow => cd_text::Action::EndShow,
        dm_text::Action::LastViewed => cd_text::Action::LastViewed,
        dm_text::Action::CustomShow { name, returns } => cd_text::Action::CustomShow { name: name.clone(), returns: *returns },
        dm_text::Action::Program { path } => cd_text::Action::Program { path: path.clone() },
        dm_text::Action::PlayMedia => cd_text::Action::PlayMedia,
    }
}

fn action_from(a: cd_text::Action) -> dm_text::Action {
    match a {
        cd_text::Action::Url { url } => dm_text::Action::Url { url },
        cd_text::Action::Slide { slide } => dm_text::Action::Slide { slide: slide_id_from(slide) },
        cd_text::Action::NextSlide => dm_text::Action::NextSlide,
        cd_text::Action::PreviousSlide => dm_text::Action::PreviousSlide,
        cd_text::Action::FirstSlide => dm_text::Action::FirstSlide,
        cd_text::Action::LastSlide => dm_text::Action::LastSlide,
        cd_text::Action::EndShow => dm_text::Action::EndShow,
        cd_text::Action::LastViewed => dm_text::Action::LastViewed,
        cd_text::Action::CustomShow { name, returns } => dm_text::Action::CustomShow { name, returns },
        cd_text::Action::Program { path } => dm_text::Action::Program { path },
        cd_text::Action::PlayMedia => dm_text::Action::PlayMedia,
    }
}

fn hyperlink_to(h: &dm_text::Hyperlink) -> cd_text::Hyperlink {
    cd_text::Hyperlink {
        action: action_to(&h.action),
        tooltip: h.tooltip.clone(),
        highlight_click: h.highlight_click,
        sound: h.sound.map(media_id_to),
        stop_sound: h.stop_sound,
        zoom_return: h.zoom_return.map(slide_id_to),
    }
}

fn hyperlink_from(h: cd_text::Hyperlink) -> dm_text::Hyperlink {
    dm_text::Hyperlink {
        action: action_from(h.action),
        tooltip: h.tooltip,
        highlight_click: h.highlight_click,
        sound: h.sound.map(media_id_from),
        stop_sound: h.stop_sound,
        zoom_return: h.zoom_return.map(slide_id_from),
    }
}

fn run_props_to(r: &dm_text::RunProps) -> cd_text::RunProps {
    cd_text::RunProps {
        font: r.font.clone(),
        font_ea: r.font_ea.clone(),
        font_cs: r.font_cs.clone(),
        font_sym: r.font_sym.clone(),
        size: r.size,
        bold: r.bold,
        italic: r.italic,
        underline: r.underline.clone(),
        underline_color: r.underline_color.as_ref().map(color_ref_to),
        strike: r.strike.map(|s| match s {
            dm_text::Strike::None => cd_text::Strike::None,
            dm_text::Strike::Single => cd_text::Strike::Single,
            dm_text::Strike::Double => cd_text::Strike::Double,
        }),
        baseline: r.baseline,
        caps: r.caps.map(|c| match c {
            dm_text::Caps::None => cd_text::Caps::None,
            dm_text::Caps::Small => cd_text::Caps::Small,
            dm_text::Caps::All => cd_text::Caps::All,
        }),
        spacing: r.spacing,
        kern: r.kern,
        fill: r.fill.as_ref().map(fill_to),
        outline: r.outline.as_ref().map(line_to),
        highlight: r.highlight.as_ref().map(color_ref_to),
        link: r.link.as_ref().map(hyperlink_to),
        hover: r.hover.as_ref().map(hyperlink_to),
        lang: r.lang.clone(),
        alt_lang: r.alt_lang.clone(),
        shadow: r.shadow.as_ref().map(shadow_to),
        inner_shadow: r.inner_shadow.as_ref().map(shadow_to),
        glow: r.glow.as_ref().map(glow_to),
        reflection: r.reflection.as_ref().map(reflection_to),
        soft_edge: r.soft_edge,
        no_proof: r.no_proof,
        normalize_height: r.normalize_height,
    }
}

fn run_props_from(r: cd_text::RunProps) -> dm_text::RunProps {
    dm_text::RunProps {
        font: r.font,
        font_ea: r.font_ea,
        font_cs: r.font_cs,
        font_sym: r.font_sym,
        size: r.size,
        bold: r.bold,
        italic: r.italic,
        underline: r.underline,
        underline_color: r.underline_color.map(color_ref_from),
        strike: r.strike.map(|s| match s {
            cd_text::Strike::None => dm_text::Strike::None,
            cd_text::Strike::Single => dm_text::Strike::Single,
            cd_text::Strike::Double => dm_text::Strike::Double,
        }),
        baseline: r.baseline,
        caps: r.caps.map(|c| match c {
            cd_text::Caps::None => dm_text::Caps::None,
            cd_text::Caps::Small => dm_text::Caps::Small,
            cd_text::Caps::All => dm_text::Caps::All,
        }),
        spacing: r.spacing,
        kern: r.kern,
        fill: r.fill.map(fill_from),
        outline: r.outline.map(line_from),
        highlight: r.highlight.map(color_ref_from),
        link: r.link.map(hyperlink_from),
        hover: r.hover.map(hyperlink_from),
        lang: r.lang,
        alt_lang: r.alt_lang,
        shadow: r.shadow.map(shadow_from),
        inner_shadow: r.inner_shadow.map(shadow_from),
        glow: r.glow.map(glow_from),
        reflection: r.reflection.map(reflection_from),
        soft_edge: r.soft_edge,
        no_proof: r.no_proof,
        normalize_height: r.normalize_height,
    }
}

fn run_to(r: &dm_text::Run) -> cd_text::Run {
    cd_text::Run {
        text: r.text.clone(),
        props: run_props_to(&r.props),
        kind: match &r.kind {
            dm_text::RunKind::Text => cd_text::RunKind::Text,
            dm_text::RunKind::Break => cd_text::RunKind::Break,
            dm_text::RunKind::Field { field } => cd_text::RunKind::Field { field: field.clone() },
            dm_text::RunKind::Math { omml } => cd_text::RunKind::Math { omml: omml.clone() },
        },
    }
}

fn run_from(r: cd_text::Run) -> dm_text::Run {
    dm_text::Run {
        text: r.text,
        props: run_props_from(r.props),
        kind: match r.kind {
            cd_text::RunKind::Text => dm_text::RunKind::Text,
            cd_text::RunKind::Break => dm_text::RunKind::Break,
            cd_text::RunKind::Field { field } => dm_text::RunKind::Field { field },
            cd_text::RunKind::Math { omml } => dm_text::RunKind::Math { omml },
        },
    }
}

fn paragraph_to(p: &dm_text::Paragraph) -> cd_text::Paragraph {
    cd_text::Paragraph {
        level: p.level,
        props: para_props_to(&p.props),
        runs: p.runs.iter().map(run_to).collect(),
        def_props: run_props_to(&p.def_props),
        end_props: run_props_to(&p.end_props),
    }
}

fn paragraph_from(p: cd_text::Paragraph) -> dm_text::Paragraph {
    dm_text::Paragraph {
        level: p.level,
        props: para_props_from(p.props),
        runs: p.runs.into_iter().map(run_from).collect(),
        def_props: run_props_from(p.def_props),
        end_props: run_props_from(p.end_props),
    }
}

pub(crate) fn list_style_to_craft(l: &dm_text::ListStyle) -> cd_text::ListStyle {
    cd_text::ListStyle {
        levels: l
            .levels
            .iter()
            .map(|opt| opt.as_ref().map(|lv| cd_text::LevelStyle { para: para_props_to(&lv.para), run: run_props_to(&lv.run) }))
            .collect(),
        default_level: None,
    }
}

fn list_style_from(l: cd_text::ListStyle) -> dm_text::ListStyle {
    dm_text::ListStyle {
        levels: l
            .levels
            .into_iter()
            .map(|opt| opt.map(|lv| dm_text::LevelStyle { para: para_props_from(lv.para), run: run_props_from(lv.run) }))
            .collect(),
    }
}

fn text_body_to(t: &dm_text::TextBody) -> cd_text::TextBody {
    cd_text::TextBody {
        body: body_props_to(&t.body),
        list_style: list_style_to_craft(&t.list_style),
        paragraphs: t.paragraphs.iter().map(paragraph_to).collect(),
    }
}

fn text_body_from(t: cd_text::TextBody) -> dm_text::TextBody {
    dm_text::TextBody {
        body: body_props_from(t.body),
        list_style: list_style_from(t.list_style),
        paragraphs: t.paragraphs.into_iter().map(paragraph_from).collect(),
    }
}

// ---------------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------------

fn cell_to(c: &dm_table::Cell) -> cd_table::Cell {
    cd_table::Cell {
        text: text_body_to(&c.text),
        fill: c.fill.as_ref().map(fill_to),
        borders: [
            c.borders[0].as_ref().map(line_to),
            c.borders[1].as_ref().map(line_to),
            c.borders[2].as_ref().map(line_to),
            c.borders[3].as_ref().map(line_to),
        ],
        diag_down: c.diag_down.as_ref().map(line_to),
        diag_up: c.diag_up.as_ref().map(line_to),
        margins: c.margins,
        anchor: c.anchor.map(anchor_to),
        grid_span: c.grid_span,
        row_span: c.row_span,
        h_merge: c.h_merge,
        v_merge: c.v_merge,
        vertical: c.vertical.map(text_dir_to),
        anchor_ctr: c.anchor_ctr,
        horz_overflow: c.horz_overflow.map(text_overflow_to),
    }
}

fn cell_from(c: cd_table::Cell) -> dm_table::Cell {
    let [b0, b1, b2, b3] = c.borders;
    dm_table::Cell {
        text: text_body_from(c.text),
        fill: c.fill.map(fill_from),
        borders: [b0.map(line_from), b1.map(line_from), b2.map(line_from), b3.map(line_from)],
        diag_down: c.diag_down.map(line_from),
        diag_up: c.diag_up.map(line_from),
        margins: c.margins,
        anchor: c.anchor.map(anchor_from),
        grid_span: c.grid_span,
        row_span: c.row_span,
        h_merge: c.h_merge,
        v_merge: c.v_merge,
        vertical: c.vertical.map(text_dir_from),
        anchor_ctr: c.anchor_ctr,
        horz_overflow: c.horz_overflow.map(text_overflow_from),
    }
}

fn table_to(t: &dm_table::Table) -> cd_table::Table {
    cd_table::Table {
        cols: t.cols.clone(),
        rows: t.rows.iter().map(|r| cd_table::Row { height: r.height, cells: r.cells.iter().map(cell_to).collect() }).collect(),
        style: t.style.clone(),
        first_row: t.first_row,
        last_row: t.last_row,
        first_col: t.first_col,
        last_col: t.last_col,
        band_row: t.band_row,
        band_col: t.band_col,
        rtl: t.rtl,
        background: t.background.as_ref().map(fill_to),
    }
}

fn table_from(t: cd_table::Table) -> dm_table::Table {
    dm_table::Table {
        cols: t.cols,
        rows: t.rows.into_iter().map(|r| dm_table::Row { height: r.height, cells: r.cells.into_iter().map(cell_from).collect() }).collect(),
        style: t.style,
        first_row: t.first_row,
        last_row: t.last_row,
        first_col: t.first_col,
        last_col: t.last_col,
        band_row: t.band_row,
        band_col: t.band_col,
        rtl: t.rtl,
        background: t.background.map(fill_from),
    }
}

fn table_border_to(b: &dm_table::TableBorder) -> cd_table::TableBorder {
    match b {
        dm_table::TableBorder::Line { line } => cd_table::TableBorder::Line { line: line_to(line) },
        dm_table::TableBorder::Ref { idx, color } => cd_table::TableBorder::Ref { idx: *idx, color: color_ref_to(color) },
    }
}

fn table_border_from(b: cd_table::TableBorder) -> dm_table::TableBorder {
    match b {
        cd_table::TableBorder::Line { line } => dm_table::TableBorder::Line { line: line_from(line) },
        cd_table::TableBorder::Ref { idx, color } => dm_table::TableBorder::Ref { idx, color: color_ref_from(color) },
    }
}

fn table_part_style_to(p: &dm_table::TablePartStyle) -> cd_table::TablePartStyle {
    cd_table::TablePartStyle {
        fill: p.fill.as_ref().map(fill_to),
        fill_ref: p.fill_ref.as_ref().map(|(idx, c)| (*idx, color_ref_to(c))),
        borders: [
            p.borders[0].as_ref().map(table_border_to),
            p.borders[1].as_ref().map(table_border_to),
            p.borders[2].as_ref().map(table_border_to),
            p.borders[3].as_ref().map(table_border_to),
            p.borders[4].as_ref().map(table_border_to),
            p.borders[5].as_ref().map(table_border_to),
        ],
        text_color: p.text_color.as_ref().map(color_ref_to),
        bold: p.bold,
        italic: p.italic,
    }
}

fn table_part_style_from(p: cd_table::TablePartStyle) -> dm_table::TablePartStyle {
    let [b0, b1, b2, b3, b4, b5] = p.borders;
    dm_table::TablePartStyle {
        fill: p.fill.map(fill_from),
        fill_ref: p.fill_ref.map(|(idx, c)| (idx, color_ref_from(c))),
        borders: [
            b0.map(table_border_from),
            b1.map(table_border_from),
            b2.map(table_border_from),
            b3.map(table_border_from),
            b4.map(table_border_from),
            b5.map(table_border_from),
        ],
        text_color: p.text_color.map(color_ref_from),
        bold: p.bold,
        italic: p.italic,
    }
}

fn table_part_to(p: &dm_table::TablePart) -> cd_table::TablePart {
    match p {
        dm_table::TablePart::WholeTbl => cd_table::TablePart::WholeTbl,
        dm_table::TablePart::Band1H => cd_table::TablePart::Band1H,
        dm_table::TablePart::Band2H => cd_table::TablePart::Band2H,
        dm_table::TablePart::Band1V => cd_table::TablePart::Band1V,
        dm_table::TablePart::Band2V => cd_table::TablePart::Band2V,
        dm_table::TablePart::FirstRow => cd_table::TablePart::FirstRow,
        dm_table::TablePart::LastRow => cd_table::TablePart::LastRow,
        dm_table::TablePart::FirstCol => cd_table::TablePart::FirstCol,
        dm_table::TablePart::LastCol => cd_table::TablePart::LastCol,
        dm_table::TablePart::NwCell => cd_table::TablePart::NwCell,
        dm_table::TablePart::NeCell => cd_table::TablePart::NeCell,
        dm_table::TablePart::SwCell => cd_table::TablePart::SwCell,
        dm_table::TablePart::SeCell => cd_table::TablePart::SeCell,
    }
}

fn table_part_from(p: cd_table::TablePart) -> dm_table::TablePart {
    match p {
        cd_table::TablePart::WholeTbl => dm_table::TablePart::WholeTbl,
        cd_table::TablePart::Band1H => dm_table::TablePart::Band1H,
        cd_table::TablePart::Band2H => dm_table::TablePart::Band2H,
        cd_table::TablePart::Band1V => dm_table::TablePart::Band1V,
        cd_table::TablePart::Band2V => dm_table::TablePart::Band2V,
        cd_table::TablePart::FirstRow => dm_table::TablePart::FirstRow,
        cd_table::TablePart::LastRow => dm_table::TablePart::LastRow,
        cd_table::TablePart::FirstCol => dm_table::TablePart::FirstCol,
        cd_table::TablePart::LastCol => dm_table::TablePart::LastCol,
        cd_table::TablePart::NwCell => dm_table::TablePart::NwCell,
        cd_table::TablePart::NeCell => dm_table::TablePart::NeCell,
        cd_table::TablePart::SwCell => dm_table::TablePart::SwCell,
        cd_table::TablePart::SeCell => dm_table::TablePart::SeCell,
    }
}

fn table_style_def_to(d: &dm_table::TableStyleDef) -> cd_table::TableStyleDef {
    cd_table::TableStyleDef {
        id: d.id.clone(),
        name: d.name.clone(),
        background: d.background.as_ref().map(table_part_style_to),
        parts: d.parts.iter().map(|(k, v)| (table_part_to(k), table_part_style_to(v))).collect(),
        raw: d.raw.clone(),
    }
}

fn table_style_def_from(d: cd_table::TableStyleDef) -> dm_table::TableStyleDef {
    dm_table::TableStyleDef {
        id: d.id,
        name: d.name,
        background: d.background.map(table_part_style_from),
        parts: d.parts.into_iter().map(|(k, v)| (table_part_from(k), table_part_style_from(v))).collect(),
        raw: d.raw,
    }
}

// ---------------------------------------------------------------------------
// Chart
// ---------------------------------------------------------------------------

fn chart_type_to(k: dm_chart::ChartType) -> cd_chart::ChartType {
    match k {
        dm_chart::ChartType::Column => cd_chart::ChartType::Column,
        dm_chart::ChartType::StackedColumn => cd_chart::ChartType::StackedColumn,
        dm_chart::ChartType::PercentColumn => cd_chart::ChartType::PercentColumn,
        dm_chart::ChartType::Bar => cd_chart::ChartType::Bar,
        dm_chart::ChartType::StackedBar => cd_chart::ChartType::StackedBar,
        dm_chart::ChartType::PercentBar => cd_chart::ChartType::PercentBar,
        dm_chart::ChartType::Line => cd_chart::ChartType::Line,
        dm_chart::ChartType::LineMarkers => cd_chart::ChartType::LineMarkers,
        dm_chart::ChartType::StackedLine => cd_chart::ChartType::StackedLine,
        dm_chart::ChartType::Area => cd_chart::ChartType::Area,
        dm_chart::ChartType::StackedArea => cd_chart::ChartType::StackedArea,
        dm_chart::ChartType::Pie => cd_chart::ChartType::Pie,
        dm_chart::ChartType::Doughnut => cd_chart::ChartType::Doughnut,
        dm_chart::ChartType::Scatter => cd_chart::ChartType::Scatter,
        dm_chart::ChartType::Bubble => cd_chart::ChartType::Bubble,
        dm_chart::ChartType::Radar => cd_chart::ChartType::Radar,
        dm_chart::ChartType::FilledRadar => cd_chart::ChartType::FilledRadar,
        dm_chart::ChartType::Stock => cd_chart::ChartType::Stock,
        dm_chart::ChartType::Surface => cd_chart::ChartType::Surface,
        dm_chart::ChartType::Histogram => cd_chart::ChartType::Histogram,
        dm_chart::ChartType::Pareto => cd_chart::ChartType::Pareto,
        dm_chart::ChartType::BoxWhisker => cd_chart::ChartType::BoxWhisker,
        dm_chart::ChartType::Waterfall => cd_chart::ChartType::Waterfall,
        dm_chart::ChartType::Funnel => cd_chart::ChartType::Funnel,
        dm_chart::ChartType::Treemap => cd_chart::ChartType::Treemap,
        dm_chart::ChartType::Sunburst => cd_chart::ChartType::Sunburst,
        dm_chart::ChartType::RegionMap => cd_chart::ChartType::RegionMap,
        dm_chart::ChartType::Combo => cd_chart::ChartType::Combo,
    }
}

fn chart_type_from(k: cd_chart::ChartType) -> dm_chart::ChartType {
    match k {
        cd_chart::ChartType::Column => dm_chart::ChartType::Column,
        cd_chart::ChartType::StackedColumn => dm_chart::ChartType::StackedColumn,
        cd_chart::ChartType::PercentColumn => dm_chart::ChartType::PercentColumn,
        cd_chart::ChartType::Bar => dm_chart::ChartType::Bar,
        cd_chart::ChartType::StackedBar => dm_chart::ChartType::StackedBar,
        cd_chart::ChartType::PercentBar => dm_chart::ChartType::PercentBar,
        cd_chart::ChartType::Line => dm_chart::ChartType::Line,
        cd_chart::ChartType::LineMarkers => dm_chart::ChartType::LineMarkers,
        cd_chart::ChartType::StackedLine => dm_chart::ChartType::StackedLine,
        cd_chart::ChartType::Area => dm_chart::ChartType::Area,
        cd_chart::ChartType::StackedArea => dm_chart::ChartType::StackedArea,
        cd_chart::ChartType::Pie => dm_chart::ChartType::Pie,
        cd_chart::ChartType::Doughnut => dm_chart::ChartType::Doughnut,
        cd_chart::ChartType::Scatter => dm_chart::ChartType::Scatter,
        cd_chart::ChartType::Bubble => dm_chart::ChartType::Bubble,
        cd_chart::ChartType::Radar => dm_chart::ChartType::Radar,
        cd_chart::ChartType::FilledRadar => dm_chart::ChartType::FilledRadar,
        cd_chart::ChartType::Stock => dm_chart::ChartType::Stock,
        cd_chart::ChartType::Surface => dm_chart::ChartType::Surface,
        cd_chart::ChartType::Histogram => dm_chart::ChartType::Histogram,
        cd_chart::ChartType::Pareto => dm_chart::ChartType::Pareto,
        cd_chart::ChartType::BoxWhisker => dm_chart::ChartType::BoxWhisker,
        cd_chart::ChartType::Waterfall => dm_chart::ChartType::Waterfall,
        cd_chart::ChartType::Funnel => dm_chart::ChartType::Funnel,
        cd_chart::ChartType::Treemap => dm_chart::ChartType::Treemap,
        cd_chart::ChartType::Sunburst => dm_chart::ChartType::Sunburst,
        cd_chart::ChartType::RegionMap => dm_chart::ChartType::RegionMap,
        cd_chart::ChartType::Combo => dm_chart::ChartType::Combo,
    }
}

fn trendline_type_to(t: dm_chart::TrendlineType) -> cd_chart::TrendlineType {
    match t {
        dm_chart::TrendlineType::Exp => cd_chart::TrendlineType::Exp,
        dm_chart::TrendlineType::Linear => cd_chart::TrendlineType::Linear,
        dm_chart::TrendlineType::Log => cd_chart::TrendlineType::Log,
        dm_chart::TrendlineType::MovingAvg => cd_chart::TrendlineType::MovingAvg,
        dm_chart::TrendlineType::Poly => cd_chart::TrendlineType::Poly,
        dm_chart::TrendlineType::Power => cd_chart::TrendlineType::Power,
    }
}

fn trendline_type_from(t: cd_chart::TrendlineType) -> dm_chart::TrendlineType {
    match t {
        cd_chart::TrendlineType::Exp => dm_chart::TrendlineType::Exp,
        cd_chart::TrendlineType::Linear => dm_chart::TrendlineType::Linear,
        cd_chart::TrendlineType::Log => dm_chart::TrendlineType::Log,
        cd_chart::TrendlineType::MovingAvg => dm_chart::TrendlineType::MovingAvg,
        cd_chart::TrendlineType::Poly => dm_chart::TrendlineType::Poly,
        cd_chart::TrendlineType::Power => dm_chart::TrendlineType::Power,
    }
}

fn series_to(s: &dm_chart::Series) -> cd_chart::Series {
    cd_chart::Series {
        name: s.name.clone(),
        values: s.values.clone(),
        x_values: s.x_values.clone(),
        sizes: s.sizes.clone(),
        fill: s.fill.as_ref().map(fill_to),
        line: s.line.as_ref().map(line_to),
        point_fills: s.point_fills.iter().map(|(i, f)| (*i, fill_to(f))).collect(),
        data_labels: s.data_labels,
        smooth: s.smooth,
        marker: s.marker.clone(),
        kind: s.kind.map(chart_type_to),
        secondary_axis: s.secondary_axis,
        number_format: s.number_format.clone(),
        trendlines: s
            .trendlines
            .iter()
            .map(|t| cd_chart::Trendline {
                kind: trendline_type_to(t.kind),
                name: t.name.clone(),
                order: t.order,
                period: t.period,
                forward: t.forward,
                backward: t.backward,
                intercept: t.intercept,
                disp_eq: t.disp_eq,
                disp_r_sqr: t.disp_r_sqr,
            })
            .collect(),
        error_bars: s.error_bars.as_ref().map(|e| cd_chart::ErrorBars {
            bar_type: e.bar_type.clone(),
            val_type: e.val_type.clone(),
            value: e.value,
            plus: e.plus.clone(),
            minus: e.minus.clone(),
            end_cap: e.end_cap,
        }),
        refs: cd_chart::SeriesRefs::default(),
    }
}

fn series_from(s: cd_chart::Series) -> dm_chart::Series {
    dm_chart::Series {
        name: s.name,
        values: s.values,
        x_values: s.x_values,
        sizes: s.sizes,
        fill: s.fill.map(fill_from),
        line: s.line.map(line_from),
        point_fills: s.point_fills.into_iter().map(|(i, f)| (i, fill_from(f))).collect(),
        data_labels: s.data_labels,
        smooth: s.smooth,
        marker: s.marker,
        kind: s.kind.map(chart_type_from),
        secondary_axis: s.secondary_axis,
        number_format: s.number_format,
        trendlines: s
            .trendlines
            .into_iter()
            .map(|t| dm_chart::Trendline {
                kind: trendline_type_from(t.kind),
                name: t.name,
                order: t.order,
                period: t.period,
                forward: t.forward,
                backward: t.backward,
                intercept: t.intercept,
                disp_eq: t.disp_eq,
                disp_r_sqr: t.disp_r_sqr,
            })
            .collect(),
        error_bars: s.error_bars.map(|e| dm_chart::ErrorBars {
            bar_type: e.bar_type,
            val_type: e.val_type,
            value: e.value,
            plus: e.plus,
            minus: e.minus,
            end_cap: e.end_cap,
        }),
    }
}

fn chart_kept_to(k: &dm_chart::ChartKept) -> cd_chart::ChartKept {
    cd_chart::ChartKept {
        rels: k
            .rels
            .iter()
            .map(|r| cd_chart::ChartRel {
                id: r.id.clone(),
                rel_type: r.rel_type.clone(),
                target: r.target.clone(),
                part: r.part.clone(),
                external: r.external,
            })
            .collect(),
        parts: k
            .parts
            .iter()
            .map(|p| cd_chart::ChartPart {
                name: p.name.clone(),
                content_type: p.content_type.clone(),
                bytes: p.bytes.clone(),
                native_entry: p.native_entry.clone(),
            })
            .collect(),
    }
}

fn chart_kept_from(k: cd_chart::ChartKept) -> dm_chart::ChartKept {
    dm_chart::ChartKept {
        rels: k
            .rels
            .into_iter()
            .map(|r| dm_chart::ChartRel { id: r.id, rel_type: r.rel_type, target: r.target, part: r.part, external: r.external })
            .collect(),
        parts: k
            .parts
            .into_iter()
            .map(|p| dm_chart::ChartPart { name: p.name, content_type: p.content_type, bytes: p.bytes, native_entry: p.native_entry })
            .collect(),
    }
}

fn chart_to(c: &dm_chart::Chart) -> cd_chart::Chart {
    let mut out = cd_chart::Chart {
        category_format: Default::default(),
        date_1904: false,
        kind: chart_type_to(c.kind),
        title: c.title.clone(),
        categories: c.categories.clone(),
        series: c.series.iter().map(series_to).collect(),
        legend: c.legend.clone(),
        legend_overlay: c.legend_overlay,
        legend_entries: c
            .legend_entries
            .iter()
            .map(|e| cd_chart::LegendEntry { idx: e.idx, size: e.size, bold: e.bold, color: e.color.as_ref().map(color_ref_to) })
            .collect(),
        text_size: c.text_size,
        title_size: c.title_size,
        data_labels: c.data_labels,
        gridlines: c.gridlines,
        minor_gridlines: c.minor_gridlines,
        axis_titles: c.axis_titles.clone(),
        style: c.style,
        palette: c.palette.clone(),
        plot_fill: c.plot_fill.as_ref().map(fill_to),
        chart_fill: c.chart_fill.as_ref().map(fill_to),
        text_color: c.text_color.as_ref().map(color_ref_to),
        hole_size: c.hole_size,
        gap_width: c.gap_width,
        overlap: c.overlap,
        vary_colors: c.vary_colors,
        y_min: c.y_min,
        y_max: c.y_max,
        y_major: c.y_major,
        y_log_base: c.y_log_base,
        y_format: c.y_format.clone(),
        y_hidden: c.y_hidden,
        cat_reversed: c.cat_reversed,
        y2_min: c.y2_min,
        y2_max: c.y2_max,
        y2_major: c.y2_major,
        y2_format: c.y2_format.clone(),
        y2_hidden: c.y2_hidden,
        lang: c.lang.clone(),
        subtotals: c.subtotals.clone(),
        category_levels: c.category_levels.clone(),
        inclusive_quartiles: c.inclusive_quartiles,
        up_down_bars: c.up_down_bars,
        bin_count: c.bin_count,
        bin_size: c.bin_size,
        data_table: None,
        first_slice_angle: 0,
        pivot_source: None,
        raw: c.raw.clone(),
        verbatim: c.verbatim,
        kept: chart_kept_to(&c.kept),
    };
    if !c.verbatim
        && let Some(raw) = &c.raw
        && let Some(parsed) = cd_chart::read::read_chart_xml(raw.as_bytes()).or_else(|| cd_chart::chartex::read_chartex_xml(raw.as_bytes()))
    {
        out.data_table = parsed.data_table;
        out.first_slice_angle = parsed.first_slice_angle;
        out.pivot_source = parsed.pivot_source;
        for (dst, src) in out.series.iter_mut().zip(parsed.series) {
            dst.refs = src.refs;
        }
    }
    out
}

fn apply_series_dlbls_delete(raw: &str, series: &mut [dm_chart::Series]) {
    let Ok(root) = craft_drawingml::xmlx::parse(raw.as_bytes()) else {
        return;
    };
    let Some(pa) = root.path(&[craft_xml::Local("chart"), craft_xml::Local("plotArea")]) else {
        return;
    };
    let sers = pa.elements().flat_map(|g| g.children(craft_xml::Local("ser")));
    for (ser_el, dst) in sers.zip(series) {
        let deleted = ser_el
            .path(&[craft_xml::Local("dLbls"), craft_xml::Local("delete")])
            .map(|v| v.attr_local("val").and_then(craft_xml::value::to_bool).unwrap_or(true));
        if deleted == Some(true) {
            dst.data_labels = Some(false);
        }
    }
}

fn chart_from(c: cd_chart::Chart) -> dm_chart::Chart {
    let mut out = dm_chart::Chart {
        kind: chart_type_from(c.kind),
        title: c.title,
        categories: c.categories,
        series: c.series.into_iter().map(series_from).collect(),
        legend: c.legend,
        legend_overlay: c.legend_overlay,
        legend_entries: c
            .legend_entries
            .into_iter()
            .map(|e| dm_chart::LegendEntry { idx: e.idx, size: e.size, bold: e.bold, color: e.color.map(color_ref_from) })
            .collect(),
        text_size: c.text_size,
        title_size: c.title_size,
        data_labels: c.data_labels,
        gridlines: c.gridlines,
        minor_gridlines: c.minor_gridlines,
        axis_titles: c.axis_titles,
        style: c.style,
        palette: c.palette,
        plot_fill: c.plot_fill.map(fill_from),
        chart_fill: c.chart_fill.map(fill_from),
        text_color: c.text_color.map(color_ref_from),
        hole_size: c.hole_size,
        gap_width: c.gap_width,
        overlap: c.overlap,
        vary_colors: c.vary_colors,
        y_min: c.y_min,
        y_max: c.y_max,
        y_major: c.y_major,
        y_log_base: c.y_log_base,
        y_format: c.y_format,
        y_hidden: c.y_hidden,
        cat_reversed: c.cat_reversed,
        y2_min: c.y2_min,
        y2_max: c.y2_max,
        y2_major: c.y2_major,
        y2_format: c.y2_format,
        y2_hidden: c.y2_hidden,
        lang: c.lang,
        subtotals: c.subtotals,
        category_levels: c.category_levels,
        inclusive_quartiles: c.inclusive_quartiles,
        up_down_bars: c.up_down_bars,
        bin_count: c.bin_count,
        bin_size: c.bin_size,
        raw: c.raw,
        verbatim: c.verbatim,
        kept: chart_kept_from(c.kept),
    };
    if let Some(raw) = &out.raw {
        apply_series_dlbls_delete(raw, &mut out.series);
    }
    out
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

fn font_set_to(f: &dm_theme::FontSet) -> cd_theme::FontSet {
    cd_theme::FontSet {
        latin: f.latin.clone(),
        ea: f.ea.clone(),
        cs: f.cs.clone(),
        latin_attrs: cd_theme::TypefaceAttrs::default(),
        ea_attrs: cd_theme::TypefaceAttrs::default(),
        cs_attrs: cd_theme::TypefaceAttrs::default(),
        scripts: f.scripts.iter().map(|s| cd_theme::ScriptFont { script: s.script.clone(), typeface: s.typeface.clone() }).collect(),
    }
}

fn font_set_from(f: cd_theme::FontSet) -> dm_theme::FontSet {
    dm_theme::FontSet {
        latin: f.latin,
        ea: f.ea,
        cs: f.cs,
        scripts: f.scripts.into_iter().map(|s| dm_theme::ScriptFont { script: s.script, typeface: s.typeface }).collect(),
    }
}

pub(crate) fn theme_to_craft(t: &dm_theme::Theme) -> cd_theme::Theme {
    cd_theme::Theme {
        name: t.name.clone(),
        colors: t.colors.clone(),
        fonts: cd_theme::FontScheme { name: t.fonts.name.clone(), major: font_set_to(&t.fonts.major), minor: font_set_to(&t.fonts.minor) },
        format: cd_theme::FormatScheme {
            name: t.format.name.clone(),
            fills: t.format.fills.iter().map(fill_to).collect(),
            lines: t.format.lines.iter().map(line_to).collect(),
            effects: t.format.effects.iter().map(effects_to).collect(),
            bg_fills: t.format.bg_fills.iter().map(fill_to).collect(),
        },
        sys_colors: vec![],
        raw_extra: t.raw_extra.clone(),
    }
}

pub(crate) fn theme_from_craft(t: cd_theme::Theme) -> dm_theme::Theme {
    dm_theme::Theme {
        name: t.name,
        colors: t.colors,
        fonts: dm_theme::FontScheme { name: t.fonts.name, major: font_set_from(t.fonts.major), minor: font_set_from(t.fonts.minor) },
        format: dm_theme::FormatScheme {
            name: t.format.name,
            fills: t.format.fills.into_iter().map(fill_from).collect(),
            lines: t.format.lines.into_iter().map(line_from).collect(),
            effects: t.format.effects.into_iter().map(effects_from).collect(),
            bg_fills: t.format.bg_fills.into_iter().map(fill_from).collect(),
        },
        raw_extra: t.raw_extra,
    }
}

// ---------------------------------------------------------------------------
// Animations & Transitions
// ---------------------------------------------------------------------------

fn transition_to(t: &dm_anim::Transition) -> cp_anim::Transition {
    cp_anim::Transition {
        kind: t.kind.clone(),
        option: t.option.clone(),
        duration_ms: t.duration_ms,
        advance_on_click: t.advance_on_click,
        advance_after_ms: t.advance_after_ms,
        sound: t.sound.map(media_id_to),
        sound_loop: t.sound_loop,
        stop_sound: t.stop_sound,
        raw: t.raw.clone(),
    }
}

fn transition_from(t: cp_anim::Transition) -> dm_anim::Transition {
    dm_anim::Transition {
        kind: t.kind,
        option: t.option,
        duration_ms: t.duration_ms,
        advance_on_click: t.advance_on_click,
        advance_after_ms: t.advance_after_ms,
        sound: t.sound.map(media_id_from),
        sound_loop: t.sound_loop,
        stop_sound: t.stop_sound,
        raw: t.raw,
    }
}

fn anim_class_to(c: dm_anim::AnimClass) -> cp_anim::AnimClass {
    match c {
        dm_anim::AnimClass::Entrance => cp_anim::AnimClass::Entrance,
        dm_anim::AnimClass::Emphasis => cp_anim::AnimClass::Emphasis,
        dm_anim::AnimClass::Exit => cp_anim::AnimClass::Exit,
        dm_anim::AnimClass::Path => cp_anim::AnimClass::Path,
        dm_anim::AnimClass::Media => cp_anim::AnimClass::Media,
    }
}

fn anim_class_from(c: cp_anim::AnimClass) -> dm_anim::AnimClass {
    match c {
        cp_anim::AnimClass::Entrance => dm_anim::AnimClass::Entrance,
        cp_anim::AnimClass::Emphasis => dm_anim::AnimClass::Emphasis,
        cp_anim::AnimClass::Exit => dm_anim::AnimClass::Exit,
        cp_anim::AnimClass::Path => dm_anim::AnimClass::Path,
        cp_anim::AnimClass::Media => dm_anim::AnimClass::Media,
    }
}

fn anim_start_to(s: dm_anim::AnimStart) -> cp_anim::AnimStart {
    match s {
        dm_anim::AnimStart::OnClick => cp_anim::AnimStart::OnClick,
        dm_anim::AnimStart::WithPrevious => cp_anim::AnimStart::WithPrevious,
        dm_anim::AnimStart::AfterPrevious => cp_anim::AnimStart::AfterPrevious,
    }
}

fn anim_start_from(s: cp_anim::AnimStart) -> dm_anim::AnimStart {
    match s {
        cp_anim::AnimStart::OnClick => dm_anim::AnimStart::OnClick,
        cp_anim::AnimStart::WithPrevious => dm_anim::AnimStart::WithPrevious,
        cp_anim::AnimStart::AfterPrevious => dm_anim::AnimStart::AfterPrevious,
    }
}

fn text_build_to(b: dm_anim::TextBuild) -> cp_anim::TextBuild {
    match b {
        dm_anim::TextBuild::AsOne => cp_anim::TextBuild::AsOne,
        dm_anim::TextBuild::ByParagraph => cp_anim::TextBuild::ByParagraph,
        dm_anim::TextBuild::ByWord => cp_anim::TextBuild::ByWord,
        dm_anim::TextBuild::ByLetter => cp_anim::TextBuild::ByLetter,
    }
}

fn text_build_from(b: cp_anim::TextBuild) -> dm_anim::TextBuild {
    match b {
        cp_anim::TextBuild::AsOne => dm_anim::TextBuild::AsOne,
        cp_anim::TextBuild::ByParagraph => dm_anim::TextBuild::ByParagraph,
        cp_anim::TextBuild::ByWord => dm_anim::TextBuild::ByWord,
        cp_anim::TextBuild::ByLetter => dm_anim::TextBuild::ByLetter,
    }
}

fn animation_to(a: &dm_anim::Animation) -> cp_anim::Animation {
    cp_anim::Animation {
        shape: shape_id_to(a.shape),
        class: anim_class_to(a.class),
        effect: a.effect.clone(),
        option: a.option.clone(),
        start: anim_start_to(a.start),
        duration_ms: a.duration_ms,
        delay_ms: a.delay_ms,
        repeat: a.repeat,
        rewind: a.rewind,
        text_build: text_build_to(a.text_build),
        paragraph: a.paragraph,
        path: a.path.clone(),
        trigger: a.trigger.map(shape_id_to),
        amount: a.amount,
        color: a.color.as_ref().map(color_ref_to),
        preset_id: a.preset_id,
        preset_subtype: a.preset_subtype,
        after: a.after.clone(),
        sound: a.sound.map(media_id_to),
        smooth_start: a.smooth_start,
        smooth_end: a.smooth_end,
        bounce_end: a.bounce_end,
        auto_reverse: a.auto_reverse,
    }
}

fn animation_from(a: cp_anim::Animation) -> dm_anim::Animation {
    dm_anim::Animation {
        shape: shape_id_from(a.shape),
        class: anim_class_from(a.class),
        effect: a.effect,
        option: a.option,
        start: anim_start_from(a.start),
        duration_ms: a.duration_ms,
        delay_ms: a.delay_ms,
        repeat: a.repeat,
        rewind: a.rewind,
        text_build: text_build_from(a.text_build),
        paragraph: a.paragraph,
        path: a.path,
        trigger: a.trigger.map(shape_id_from),
        amount: a.amount,
        color: a.color.map(color_ref_from),
        preset_id: a.preset_id,
        preset_subtype: a.preset_subtype,
        after: a.after,
        sound: a.sound.map(media_id_from),
        smooth_start: a.smooth_start,
        smooth_end: a.smooth_end,
        bounce_end: a.bounce_end,
        auto_reverse: a.auto_reverse,
    }
}

// ---------------------------------------------------------------------------
// Shapes, Slides, Masters, Presentation
// ---------------------------------------------------------------------------

fn ph_type_to(p: deckcraft_model::PhType) -> craft_pptx::PhType {
    match p {
        deckcraft_model::PhType::Title => craft_pptx::PhType::Title,
        deckcraft_model::PhType::CtrTitle => craft_pptx::PhType::CtrTitle,
        deckcraft_model::PhType::SubTitle => craft_pptx::PhType::SubTitle,
        deckcraft_model::PhType::Body => craft_pptx::PhType::Body,
        deckcraft_model::PhType::Obj => craft_pptx::PhType::Obj,
        deckcraft_model::PhType::Chart => craft_pptx::PhType::Chart,
        deckcraft_model::PhType::Table => craft_pptx::PhType::Table,
        deckcraft_model::PhType::ClipArt => craft_pptx::PhType::ClipArt,
        deckcraft_model::PhType::Diagram => craft_pptx::PhType::Diagram,
        deckcraft_model::PhType::Media => craft_pptx::PhType::Media,
        deckcraft_model::PhType::Picture => craft_pptx::PhType::Picture,
        deckcraft_model::PhType::Date => craft_pptx::PhType::Date,
        deckcraft_model::PhType::Footer => craft_pptx::PhType::Footer,
        deckcraft_model::PhType::SlideNum => craft_pptx::PhType::SlideNum,
        deckcraft_model::PhType::Header => craft_pptx::PhType::Header,
        deckcraft_model::PhType::SlideImage => craft_pptx::PhType::SlideImage,
    }
}

fn ph_type_from(p: craft_pptx::PhType) -> deckcraft_model::PhType {
    match p {
        craft_pptx::PhType::Title => deckcraft_model::PhType::Title,
        craft_pptx::PhType::CtrTitle => deckcraft_model::PhType::CtrTitle,
        craft_pptx::PhType::SubTitle => deckcraft_model::PhType::SubTitle,
        craft_pptx::PhType::Body => deckcraft_model::PhType::Body,
        craft_pptx::PhType::Obj => deckcraft_model::PhType::Obj,
        craft_pptx::PhType::Chart => deckcraft_model::PhType::Chart,
        craft_pptx::PhType::Table => deckcraft_model::PhType::Table,
        craft_pptx::PhType::ClipArt => deckcraft_model::PhType::ClipArt,
        craft_pptx::PhType::Diagram => deckcraft_model::PhType::Diagram,
        craft_pptx::PhType::Media => deckcraft_model::PhType::Media,
        craft_pptx::PhType::Picture => deckcraft_model::PhType::Picture,
        craft_pptx::PhType::Date => deckcraft_model::PhType::Date,
        craft_pptx::PhType::Footer => deckcraft_model::PhType::Footer,
        craft_pptx::PhType::SlideNum => deckcraft_model::PhType::SlideNum,
        craft_pptx::PhType::Header => deckcraft_model::PhType::Header,
        craft_pptx::PhType::SlideImage => deckcraft_model::PhType::SlideImage,
    }
}

fn geom_to(g: &deckcraft_model::Geom) -> craft_pptx::Geom {
    match g {
        deckcraft_model::Geom::Preset { name, adj } => craft_pptx::Geom::Preset { name: name.clone(), adj: adj.clone() },
        deckcraft_model::Geom::Custom { paths } => craft_pptx::Geom::Custom {
            paths: paths.iter().map(|p| craft_pptx::CustomPath { w: p.w, h: p.h, d: p.d.clone(), fill: p.fill, stroke: p.stroke }).collect(),
        },
    }
}

fn geom_from(g: craft_pptx::Geom) -> deckcraft_model::Geom {
    match g {
        craft_pptx::Geom::Preset { name, adj } => deckcraft_model::Geom::Preset { name, adj },
        craft_pptx::Geom::Custom { paths } => deckcraft_model::Geom::Custom {
            paths: paths.into_iter().map(|p| deckcraft_model::CustomPath { w: p.w, h: p.h, d: p.d, fill: p.fill, stroke: p.stroke }).collect(),
        },
    }
}

fn diagram_to(d: &deckcraft_model::Diagram) -> cd_dgm::Diagram {
    cd_dgm::Diagram {
        kind: d.kind.clone(),
        nodes: d
            .nodes
            .iter()
            .map(|n| cd_dgm::DiagramNode {
                text: n.text.clone(),
                level: n.level,
                picture: n.picture.map(media_id_to),
                assistant: n.assistant,
                title: n.title.clone(),
            })
            .collect(),
        style: d.style.clone(),
        colors: d.colors.clone(),
        imported: d.imported,
        layout_id: d.layout_id.clone(),
        layout_xml: d.layout_xml.clone(),
        style_xml: d.style_xml.clone(),
        colors_xml: d.colors_xml.clone(),
    }
}

fn diagram_from(d: cd_dgm::Diagram) -> deckcraft_model::Diagram {
    deckcraft_model::Diagram {
        kind: d.kind,
        nodes: d
            .nodes
            .into_iter()
            .map(|n| deckcraft_model::DiagramNode {
                text: n.text,
                level: n.level,
                picture: n.picture.map(media_id_from),
                assistant: n.assistant,
                title: n.title,
            })
            .collect(),
        style: d.style,
        colors: d.colors,
        imported: d.imported,
        layout_id: d.layout_id,
        layout_xml: d.layout_xml,
        style_xml: d.style_xml,
        colors_xml: d.colors_xml,
    }
}

fn media_clip_to(m: &deckcraft_model::MediaClip) -> craft_pptx::MediaClip {
    craft_pptx::MediaClip {
        media: media_id_to(m.media),
        video: m.video,
        poster: m.poster.map(media_id_to),
        crop: m.crop,
        trim_start_ms: m.trim_start_ms,
        trim_end_ms: m.trim_end_ms,
        fade_in_ms: m.fade_in_ms,
        fade_out_ms: m.fade_out_ms,
        volume: m.volume,
        autoplay: m.autoplay,
        loop_play: m.loop_play,
        rewind: m.rewind,
        play_across_slides: m.play_across_slides,
        hide_while_not_playing: m.hide_while_not_playing,
        full_screen: m.full_screen,
        bookmarks: m.bookmarks.clone(),
        duration_ms: m.duration_ms,
        width: m.width,
        height: m.height,
    }
}

fn media_clip_from(m: craft_pptx::MediaClip) -> deckcraft_model::MediaClip {
    deckcraft_model::MediaClip {
        media: media_id_from(m.media),
        video: m.video,
        poster: m.poster.map(media_id_from),
        crop: m.crop,
        trim_start_ms: m.trim_start_ms,
        trim_end_ms: m.trim_end_ms,
        fade_in_ms: m.fade_in_ms,
        fade_out_ms: m.fade_out_ms,
        volume: m.volume,
        autoplay: m.autoplay,
        loop_play: m.loop_play,
        rewind: m.rewind,
        play_across_slides: m.play_across_slides,
        hide_while_not_playing: m.hide_while_not_playing,
        full_screen: m.full_screen,
        bookmarks: m.bookmarks,
        duration_ms: m.duration_ms,
        width: m.width,
        height: m.height,
    }
}

fn shape_kind_to(k: &deckcraft_model::ShapeKind) -> craft_pptx::ShapeKind {
    match k {
        deckcraft_model::ShapeKind::Shape => craft_pptx::ShapeKind::Shape,
        deckcraft_model::ShapeKind::Picture { fill } => craft_pptx::ShapeKind::Picture { fill: picture_fill_to(fill) },
        deckcraft_model::ShapeKind::Group { children, child } => {
            craft_pptx::ShapeKind::Group { children: children.iter().map(shape_to).collect(), child: *child }
        }
        deckcraft_model::ShapeKind::Connector { start, end } => craft_pptx::ShapeKind::Connector {
            start: start.map(|(id, idx)| (shape_id_to(id), idx)),
            end: end.map(|(id, idx)| (shape_id_to(id), idx)),
        },
        deckcraft_model::ShapeKind::Table(t) => craft_pptx::ShapeKind::Table(table_to(t)),
        deckcraft_model::ShapeKind::Chart(c) => craft_pptx::ShapeKind::Chart(Box::new(chart_to(c))),
        deckcraft_model::ShapeKind::Media(m) => craft_pptx::ShapeKind::Media(media_clip_to(m)),
        deckcraft_model::ShapeKind::Ink { strokes } => craft_pptx::ShapeKind::Ink {
            strokes: strokes
                .iter()
                .map(|s| craft_pptx::InkStroke { points: s.points.clone(), color: s.color, width: s.width, highlighter: s.highlighter })
                .collect(),
        },
        deckcraft_model::ShapeKind::Model3d { media, poster } => {
            craft_pptx::ShapeKind::Model3d { media: media_id_to(*media), poster: poster.map(media_id_to) }
        }
        deckcraft_model::ShapeKind::Opaque { xml, preview, label, parts } => craft_pptx::ShapeKind::Opaque {
            xml: xml.clone(),
            preview: preview.map(media_id_to),
            label: label.clone(),
            parts: parts.iter().map(|(r, t, m)| (r.clone(), t.clone(), media_id_to(*m))).collect(),
        },
    }
}

fn shape_kind_from(k: craft_pptx::ShapeKind) -> deckcraft_model::ShapeKind {
    match k {
        craft_pptx::ShapeKind::Shape => deckcraft_model::ShapeKind::Shape,
        craft_pptx::ShapeKind::Picture { fill } => deckcraft_model::ShapeKind::Picture { fill: picture_fill_from(fill) },
        craft_pptx::ShapeKind::Group { children, child } => {
            deckcraft_model::ShapeKind::Group { children: children.into_iter().map(shape_from).collect(), child }
        }
        craft_pptx::ShapeKind::Connector { start, end } => deckcraft_model::ShapeKind::Connector {
            start: start.map(|(id, idx)| (shape_id_from(id), idx)),
            end: end.map(|(id, idx)| (shape_id_from(id), idx)),
        },
        craft_pptx::ShapeKind::Table(t) => deckcraft_model::ShapeKind::Table(table_from(t)),
        craft_pptx::ShapeKind::Chart(c) => deckcraft_model::ShapeKind::Chart(Box::new(chart_from(*c))),
        craft_pptx::ShapeKind::Media(m) => deckcraft_model::ShapeKind::Media(media_clip_from(m)),
        craft_pptx::ShapeKind::Ink { strokes } => deckcraft_model::ShapeKind::Ink {
            strokes: strokes
                .into_iter()
                .map(|s| deckcraft_model::InkStroke { points: s.points, color: s.color, width: s.width, highlighter: s.highlighter })
                .collect(),
        },
        craft_pptx::ShapeKind::Model3d { media, poster } => {
            deckcraft_model::ShapeKind::Model3d { media: media_id_from(media), poster: poster.map(media_id_from) }
        }
        craft_pptx::ShapeKind::Opaque { xml, preview, label, parts } => deckcraft_model::ShapeKind::Opaque {
            xml,
            preview: preview.map(media_id_from),
            label,
            parts: parts.into_iter().map(|(r, t, m)| (r, t, media_id_from(m))).collect(),
        },
    }
}

fn shape_to(s: &deckcraft_model::Shape) -> craft_pptx::Shape {
    craft_pptx::Shape {
        id: shape_id_to(s.id),
        name: s.name.clone(),
        xfrm: s.xfrm,
        kind: shape_kind_to(&s.kind),
        geom: geom_to(&s.geom),
        ph: s.ph.as_ref().map(|p| craft_pptx::Placeholder {
            kind: ph_type_to(p.kind),
            idx: p.idx,
            vertical: p.vertical,
            size: p.size.clone(),
            has_custom_prompt: p.has_custom_prompt,
        }),
        fill: s.fill.as_ref().map(fill_to),
        line: s.line.as_ref().map(line_to),
        effects: s.effects.as_ref().map(effects_to),
        style: s.style.as_ref().map(shape_style_to),
        text: s.text.as_ref().map(text_body_to),
        hidden: s.hidden,
        locked: s.locked,
        text_box: s.text_box,
        descr: s.descr.clone(),
        title: s.title.clone(),
        decorative: s.decorative,
        click: s.click.as_ref().map(hyperlink_to),
        hover: s.hover.as_ref().map(hyperlink_to),
        tags: s.tags.clone(),
        raw_ext: s.raw_ext.clone(),
        diagram: s.diagram.as_ref().map(diagram_to),
    }
}

fn shape_from(s: craft_pptx::Shape) -> deckcraft_model::Shape {
    deckcraft_model::Shape {
        id: shape_id_from(s.id),
        name: s.name,
        xfrm: s.xfrm,
        kind: shape_kind_from(s.kind),
        geom: geom_from(s.geom),
        ph: s.ph.map(|p| deckcraft_model::Placeholder {
            kind: ph_type_from(p.kind),
            idx: p.idx,
            vertical: p.vertical,
            size: p.size,
            has_custom_prompt: p.has_custom_prompt,
        }),
        fill: s.fill.map(fill_from),
        line: s.line.map(line_from),
        effects: s.effects.map(effects_from),
        style: s.style.map(shape_style_from),
        text: s.text.map(text_body_from),
        hidden: s.hidden,
        locked: s.locked,
        text_box: s.text_box,
        descr: s.descr,
        title: s.title,
        decorative: s.decorative,
        click: s.click.map(hyperlink_from),
        hover: s.hover.map(hyperlink_from),
        tags: s.tags,
        raw_ext: s.raw_ext,
        diagram: s.diagram.map(diagram_from),
    }
}

fn background_to(b: &deckcraft_model::Background) -> craft_pptx::Background {
    match b {
        deckcraft_model::Background::Fill { fill } => craft_pptx::Background::Fill { fill: fill_to(fill) },
        deckcraft_model::Background::Ref { idx, color } => craft_pptx::Background::Ref { idx: *idx, color: color_ref_to(color) },
    }
}

fn background_from(b: craft_pptx::Background) -> deckcraft_model::Background {
    match b {
        craft_pptx::Background::Fill { fill } => deckcraft_model::Background::Fill { fill: fill_from(fill) },
        craft_pptx::Background::Ref { idx, color } => deckcraft_model::Background::Ref { idx, color: color_ref_from(color) },
    }
}

fn comment_to(c: &deckcraft_model::Comment) -> craft_pptx::Comment {
    craft_pptx::Comment {
        author: c.author.clone(),
        initials: c.initials.clone(),
        text: c.text.clone(),
        date: c.date.clone(),
        x: c.x,
        y: c.y,
        resolved: c.resolved,
        replies: c.replies.iter().map(comment_to).collect(),
        shape: c.shape.map(shape_id_to),
    }
}

fn comment_from(c: craft_pptx::Comment) -> deckcraft_model::Comment {
    deckcraft_model::Comment {
        author: c.author,
        initials: c.initials,
        text: c.text,
        date: c.date,
        x: c.x,
        y: c.y,
        resolved: c.resolved,
        replies: c.replies.into_iter().map(comment_from).collect(),
        shape: c.shape.map(shape_id_from),
    }
}

fn slide_to(s: &deckcraft_model::Slide) -> craft_pptx::Slide {
    craft_pptx::Slide {
        id: slide_id_to(s.id),
        layout: layout_id_to(s.layout),
        name: s.name.clone(),
        shapes: s.shapes.iter().map(shape_to).collect(),
        background: s.background.as_ref().map(background_to),
        hidden: s.hidden,
        show_master_shapes: s.show_master_shapes,
        transition: s.transition.as_ref().map(transition_to),
        animations: s.animations.iter().map(animation_to).collect(),
        notes: text_body_to(&s.notes),
        has_notes_slide: s.has_notes_slide,
        comments: s.comments.iter().map(comment_to).collect(),
        color_map: s.color_map.clone(),
        tags: s.tags.clone(),
        raw_ext: s.raw_ext.clone(),
    }
}

fn slide_from(s: craft_pptx::Slide) -> deckcraft_model::Slide {
    deckcraft_model::Slide {
        id: slide_id_from(s.id),
        layout: layout_id_from(s.layout),
        name: s.name,
        shapes: s.shapes.into_iter().map(shape_from).collect(),
        background: s.background.map(background_from),
        hidden: s.hidden,
        show_master_shapes: s.show_master_shapes,
        transition: s.transition.map(transition_from),
        animations: s.animations.into_iter().map(animation_from).collect(),
        notes: text_body_from(s.notes),
        has_notes_slide: s.has_notes_slide,
        comments: s.comments.into_iter().map(comment_from).collect(),
        color_map: s.color_map,
        tags: s.tags,
        raw_ext: s.raw_ext,
    }
}

fn layout_type_to(k: deckcraft_model::LayoutType) -> craft_pptx::LayoutType {
    match k {
        deckcraft_model::LayoutType::Title => craft_pptx::LayoutType::Title,
        deckcraft_model::LayoutType::TitleAndContent => craft_pptx::LayoutType::TitleAndContent,
        deckcraft_model::LayoutType::SectionHeader => craft_pptx::LayoutType::SectionHeader,
        deckcraft_model::LayoutType::TwoContent => craft_pptx::LayoutType::TwoContent,
        deckcraft_model::LayoutType::Comparison => craft_pptx::LayoutType::Comparison,
        deckcraft_model::LayoutType::TitleOnly => craft_pptx::LayoutType::TitleOnly,
        deckcraft_model::LayoutType::Blank => craft_pptx::LayoutType::Blank,
        deckcraft_model::LayoutType::ContentWithCaption => craft_pptx::LayoutType::ContentWithCaption,
        deckcraft_model::LayoutType::PictureWithCaption => craft_pptx::LayoutType::PictureWithCaption,
        deckcraft_model::LayoutType::TitleAndVerticalText => craft_pptx::LayoutType::TitleAndVerticalText,
        deckcraft_model::LayoutType::VerticalTitleAndText => craft_pptx::LayoutType::VerticalTitleAndText,
        deckcraft_model::LayoutType::Custom => craft_pptx::LayoutType::Custom,
    }
}

fn layout_type_from(k: craft_pptx::LayoutType) -> deckcraft_model::LayoutType {
    match k {
        craft_pptx::LayoutType::Title => deckcraft_model::LayoutType::Title,
        craft_pptx::LayoutType::TitleAndContent => deckcraft_model::LayoutType::TitleAndContent,
        craft_pptx::LayoutType::SectionHeader => deckcraft_model::LayoutType::SectionHeader,
        craft_pptx::LayoutType::TwoContent => deckcraft_model::LayoutType::TwoContent,
        craft_pptx::LayoutType::Comparison => deckcraft_model::LayoutType::Comparison,
        craft_pptx::LayoutType::TitleOnly => deckcraft_model::LayoutType::TitleOnly,
        craft_pptx::LayoutType::Blank => deckcraft_model::LayoutType::Blank,
        craft_pptx::LayoutType::ContentWithCaption => deckcraft_model::LayoutType::ContentWithCaption,
        craft_pptx::LayoutType::PictureWithCaption => deckcraft_model::LayoutType::PictureWithCaption,
        craft_pptx::LayoutType::TitleAndVerticalText => deckcraft_model::LayoutType::TitleAndVerticalText,
        craft_pptx::LayoutType::VerticalTitleAndText => deckcraft_model::LayoutType::VerticalTitleAndText,
        craft_pptx::LayoutType::Custom => deckcraft_model::LayoutType::Custom,
    }
}

fn layout_to(l: &deckcraft_model::Layout) -> craft_pptx::Layout {
    craft_pptx::Layout {
        id: layout_id_to(l.id),
        name: l.name.clone(),
        kind: layout_type_to(l.kind),
        shapes: l.shapes.iter().map(shape_to).collect(),
        background: l.background.as_ref().map(background_to),
        show_master_shapes: l.show_master_shapes,
        preserve: l.preserve,
        color_map: l.color_map.clone(),
        raw_ext: l.raw_ext.clone(),
    }
}

fn layout_from(l: craft_pptx::Layout) -> deckcraft_model::Layout {
    deckcraft_model::Layout {
        id: layout_id_from(l.id),
        name: l.name,
        kind: layout_type_from(l.kind),
        shapes: l.shapes.into_iter().map(shape_from).collect(),
        background: l.background.map(background_from),
        show_master_shapes: l.show_master_shapes,
        preserve: l.preserve,
        color_map: l.color_map,
        raw_ext: l.raw_ext,
    }
}

pub(crate) fn master_to_craft(m: &deckcraft_model::Master) -> craft_pptx::Master {
    craft_pptx::Master {
        id: master_id_to(m.id),
        name: m.name.clone(),
        theme: theme_to_craft(&m.theme),
        shapes: m.shapes.iter().map(shape_to).collect(),
        background: m.background.as_ref().map(background_to),
        title_style: list_style_to_craft(&m.title_style),
        body_style: list_style_to_craft(&m.body_style),
        other_style: list_style_to_craft(&m.other_style),
        layouts: m.layouts.iter().map(layout_to).collect(),
        color_map: m.color_map.clone(),
        preserve: m.preserve,
        raw_ext: m.raw_ext.clone(),
    }
}

fn master_from(m: craft_pptx::Master) -> deckcraft_model::Master {
    deckcraft_model::Master {
        id: master_id_from(m.id),
        name: m.name,
        theme: theme_from_craft(m.theme),
        shapes: m.shapes.into_iter().map(shape_from).collect(),
        background: m.background.map(background_from),
        title_style: list_style_from(m.title_style),
        body_style: list_style_from(m.body_style),
        other_style: list_style_from(m.other_style),
        layouts: m.layouts.into_iter().map(layout_from).collect(),
        color_map: m.color_map,
        preserve: m.preserve,
        raw_ext: m.raw_ext,
    }
}

pub(crate) fn media_item_from_craft(m: craft_pptx::MediaItem) -> deckcraft_model::MediaItem {
    deckcraft_model::MediaItem { id: media_id_from(m.id), name: m.name, content_type: m.content_type, data: m.data, link: m.link }
}

fn media_item_to(m: &deckcraft_model::MediaItem) -> craft_pptx::MediaItem {
    craft_pptx::MediaItem {
        id: media_id_to(m.id),
        name: m.name.clone(),
        content_type: m.content_type.clone(),
        data: Arc::clone(&m.data),
        link: m.link.clone(),
    }
}

pub(crate) fn pres_to_craft(p: &deckcraft_model::Presentation) -> craft_pptx::Presentation {
    craft_pptx::Presentation {
        slide_size: p.slide_size,
        notes_size: p.notes_size,
        slides: p.slides.iter().map(|s| Arc::new(slide_to(s))).collect(),
        masters: p.masters.iter().map(|m| Arc::new(master_to_craft(m))).collect(),
        notes_master: p.notes_master.as_ref().map(|m| Arc::new(master_to_craft(m))),
        handout_master: p.handout_master.as_ref().map(|m| Arc::new(master_to_craft(m))),
        sections: p
            .sections
            .iter()
            .map(|s| craft_pptx::Section { name: s.name.clone(), slides: s.slides.iter().copied().map(slide_id_to).collect() })
            .collect(),
        custom_shows: p
            .custom_shows
            .iter()
            .map(|c| craft_pptx::CustomShow { name: c.name.clone(), slides: c.slides.iter().copied().map(slide_id_to).collect() })
            .collect(),
        show: craft_pptx::ShowSettings {
            show_type: p.show.show_type.clone(),
            loop_until_esc: p.show.loop_until_esc,
            without_narration: p.show.without_narration,
            without_animation: p.show.without_animation,
            use_timings: p.show.use_timings,
            range: p.show.range,
            custom_show: p.show.custom_show.clone(),
            pen_color: p.show.pen_color,
            laser_color: p.show.laser_color,
            show_media_controls: p.show.show_media_controls,
        },
        header_footer: craft_pptx::HeaderFooter {
            date: p.header_footer.date,
            slide_number: p.header_footer.slide_number,
            footer: p.header_footer.footer,
            header: p.header_footer.header,
            footer_text: p.header_footer.footer_text.clone(),
            date_text: p.header_footer.date_text.clone(),
            date_format: p.header_footer.date_format.clone(),
            hide_on_title: p.header_footer.hide_on_title,
        },
        props: craft_pptx::Properties {
            title: p.props.title.clone(),
            subject: p.props.subject.clone(),
            author: p.props.author.clone(),
            keywords: p.props.keywords.clone(),
            comments: p.props.comments.clone(),
            category: p.props.category.clone(),
            company: p.props.company.clone(),
            created: p.props.created.clone(),
            modified: p.props.modified.clone(),
            last_modified_by: p.props.last_modified_by.clone(),
            revision: p.props.revision,
        },
        default_text_style: list_style_to_craft(&p.default_text_style),
        media: p.media.iter().map(media_item_to).collect(),
        first_slide_number: p.first_slide_number,
        next_id: p.next_id,
        embedded_fonts: p
            .embedded_fonts
            .iter()
            .map(|f| craft_pptx::EmbeddedFont {
                typeface: f.typeface.clone(),
                panose: f.panose.clone(),
                pitch_family: f.pitch_family,
                charset: f.charset,
                regular: f.regular.map(media_id_to),
                bold: f.bold.map(media_id_to),
                italic: f.italic.map(media_id_to),
                bold_italic: f.bold_italic.map(media_id_to),
            })
            .collect(),
        table_styles: p.table_styles.iter().map(table_style_def_to).collect(),
        tags: p.tags.clone(),
        kept_parts: p
            .kept_parts
            .iter()
            .map(|k| craft_pptx::KeptPart {
                name: k.name.clone(),
                content_type: k.content_type.clone(),
                xml: k.xml.clone(),
                rel_type: k.rel_type.clone(),
                from_root: k.from_root,
            })
            .collect(),
        guides: p.guides.iter().map(|g| craft_pptx::DrawGuide { vertical: g.vertical, pos: g.pos }).collect(),
        raw_ext: p.raw_ext.clone(),
    }
}

const DATE_KINDS: [&str; 13] = [
    "datetime1",
    "datetime2",
    "datetime3",
    "datetime4",
    "datetime5",
    "datetime6",
    "datetime7",
    "datetime8",
    "datetime9",
    "datetime10",
    "datetime11",
    "datetime12",
    "datetime13",
];

fn infer_date_format(slides: &[Arc<deckcraft_model::Slide>]) -> String {
    for s in slides {
        for sh in &s.shapes {
            if sh.ph_type() != Some(deckcraft_model::PhType::Date) {
                continue;
            }
            if let Some(kind) = sh.text.iter().flat_map(|t| t.paragraphs.iter()).flat_map(|p| p.runs.iter()).find_map(|r| match &r.kind {
                dm_text::RunKind::Field { field } if DATE_KINDS.contains(&field.as_str()) => Some(field.as_str()),
                _ => None,
            }) {
                return kind.to_string();
            }
        }
    }
    String::new()
}

pub(crate) fn pres_from_craft(p: craft_pptx::Presentation) -> deckcraft_model::Presentation {
    let slides: Vec<Arc<deckcraft_model::Slide>> = p.slides.into_iter().map(|s| Arc::new(slide_from((*s).clone()))).collect();
    let mut header_footer = deckcraft_model::HeaderFooter {
        date: p.header_footer.date,
        slide_number: p.header_footer.slide_number,
        footer: p.header_footer.footer,
        header: p.header_footer.header,
        footer_text: p.header_footer.footer_text,
        date_text: p.header_footer.date_text,
        date_format: p.header_footer.date_format,
        hide_on_title: p.header_footer.hide_on_title,
    };
    if header_footer.date && header_footer.date_format.is_empty() && header_footer.date_text.is_empty() {
        header_footer.date_format = infer_date_format(&slides);
    }
    deckcraft_model::Presentation {
        slide_size: p.slide_size,
        notes_size: p.notes_size,
        slides,
        masters: p.masters.into_iter().map(|m| Arc::new(master_from((*m).clone()))).collect(),
        notes_master: p.notes_master.map(|m| Arc::new(master_from((*m).clone()))),
        handout_master: p.handout_master.map(|m| Arc::new(master_from((*m).clone()))),
        sections: p
            .sections
            .into_iter()
            .map(|s| deckcraft_model::Section { name: s.name, slides: s.slides.into_iter().map(slide_id_from).collect() })
            .collect(),
        custom_shows: p
            .custom_shows
            .into_iter()
            .map(|c| deckcraft_model::CustomShow { name: c.name, slides: c.slides.into_iter().map(slide_id_from).collect() })
            .collect(),
        show: deckcraft_model::ShowSettings {
            show_type: p.show.show_type,
            loop_until_esc: p.show.loop_until_esc,
            without_narration: p.show.without_narration,
            without_animation: p.show.without_animation,
            use_timings: p.show.use_timings,
            range: p.show.range,
            custom_show: p.show.custom_show,
            pen_color: p.show.pen_color,
            laser_color: p.show.laser_color,
            show_media_controls: p.show.show_media_controls,
        },
        header_footer,
        props: deckcraft_model::Properties {
            title: p.props.title,
            subject: p.props.subject,
            author: p.props.author,
            keywords: p.props.keywords,
            comments: p.props.comments,
            category: p.props.category,
            company: p.props.company,
            created: p.props.created,
            modified: p.props.modified,
            last_modified_by: p.props.last_modified_by,
            revision: p.props.revision,
        },
        default_text_style: list_style_from(p.default_text_style),
        media: p.media.into_iter().map(media_item_from_craft).collect(),
        first_slide_number: p.first_slide_number,
        next_id: p.next_id,
        embedded_fonts: p
            .embedded_fonts
            .into_iter()
            .map(|f| deckcraft_model::EmbeddedFont {
                typeface: f.typeface,
                panose: f.panose,
                pitch_family: f.pitch_family,
                charset: f.charset,
                regular: f.regular.map(media_id_from),
                bold: f.bold.map(media_id_from),
                italic: f.italic.map(media_id_from),
                bold_italic: f.bold_italic.map(media_id_from),
            })
            .collect(),
        table_styles: p.table_styles.into_iter().map(table_style_def_from).collect(),
        tags: p.tags,
        kept_parts: p
            .kept_parts
            .into_iter()
            .map(|k| deckcraft_model::KeptPart {
                name: k.name,
                content_type: k.content_type,
                xml: k.xml,
                rel_type: k.rel_type,
                from_root: k.from_root,
            })
            .collect(),
        guides: p.guides.into_iter().map(|g| deckcraft_model::DrawGuide { vertical: g.vertical, pos: g.pos }).collect(),
        raw_ext: p.raw_ext,
    }
}
