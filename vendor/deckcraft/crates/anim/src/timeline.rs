//! A slide's animation sequence: click steps, effect start times and per-shape state over time.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use deckcraft_model::anim::{BuildRole, TextBuild, paragraph_build_roles};
use deckcraft_model::style::Fill;
use deckcraft_model::{AnimClass, AnimStart, Animation, ColorScheme, Rgba, SchemeSlot, Shape, ShapeId, Slide, Xfrm};

use crate::AnimState;
use crate::effects::{EffectSpec, exit_base, motion_path};

/// Delay between paragraphs of a by-paragraph build that isn't on click (seconds).
pub const PARAGRAPH_STAGGER: f64 = 0.5;
const MAX_EFFECTS: usize = 100_000;
const MAX_ITERATIONS: f64 = 1.0e6;

/// Which sequence an effect belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sequence {
    /// The slide's main click sequence.
    Main,
    /// An interactive sequence started by clicking this shape.
    Trigger(ShapeId),
}

/// Timing of one (sub-)effect, for the Animation Pane timeline and the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectTiming {
    /// Index into `slide.animations`.
    pub anim: usize,
    /// Animated shape and paragraph (`None` = the whole shape).
    pub shape: ShapeId,
    pub paragraph: Option<usize>,
    pub sequence: Sequence,
    /// Click step within its sequence.
    pub step: usize,
    /// Start within the step (seconds, delay included).
    pub start: f64,
    /// One iteration (seconds).
    pub duration: f64,
    /// Repeat count (fractional allowed); 1 = once.
    pub iterations: f64,
    /// Repeats until the next click.
    pub until_next_click: bool,
    pub auto_reverse: bool,
    pub rewind: bool,
    /// Time from start to end (all iterations, reverse included; one iteration if until-click).
    pub span: f64,
    pub class: AnimClass,
}

/// Playhead of a triggered sequence: `step` clicks on the trigger fully played, the current one
/// playing for `t` seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriggerPlay {
    pub trigger: ShapeId,
    pub step: usize,
    pub t: f64,
}

/// When a step of a triggered sequence was started, counted in started steps on the slide. Effects
/// of different triggers apply in this order, so that a shape hidden by one trigger and shown again
/// by another ends up as the later of the two left it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepStamp {
    pub trigger: ShapeId,
    pub step: usize,
    pub at: u64,
}

#[derive(Clone, Debug, Default)]
struct StepInfo {
    auto: bool,
    duration: f64,
}

#[derive(Clone, Debug)]
enum After {
    None,
    Hide,
    HideOnNextClick,
    Dim(Rgba),
}

#[derive(Clone, Debug)]
struct Effect {
    timing: EffectTiming,
    /// Shared by every paragraph effect of one animation, so a large motion path is not copied
    /// per paragraph.
    spec: Arc<EffectSpec>,
    after: After,
}

/// A slide's animations laid out in time.
#[derive(Clone, Debug, Default)]
pub struct Timeline {
    effects: Vec<Effect>,
    main: Vec<StepInfo>,
    triggers: Vec<(ShapeId, Vec<StepInfo>)>,
    /// Base fill colour per shape (for colour emphasis).
    colors: Vec<(ShapeId, Rgba)>,
    /// By-paragraph builds animate first-level paragraphs; deeper paragraphs follow their parent:
    /// (shape, member paragraph, owner paragraph).
    groups: Vec<(ShapeId, usize, usize)>,
    /// Lookups built with the timeline, so evaluating a shape or paragraph costs its own effects
    /// rather than a scan of every effect, group and colour on the slide.
    index: TimelineIndex,
}

#[derive(Clone, Debug, Default)]
struct TimelineIndex {
    /// Effects (indices into `effects`, in order) by target shape and paragraph.
    by_target: HashMap<(ShapeId, Option<usize>), Vec<usize>>,
    /// Effects (indices into `effects`, in order) by target shape.
    by_shape: HashMap<ShapeId, Vec<usize>>,
    /// Owner paragraph of each member paragraph (the first group recorded wins).
    owner: HashMap<(ShapeId, usize), usize>,
    /// Groups (member, owner) by shape, in order.
    groups_by_shape: HashMap<ShapeId, Vec<(usize, usize)>>,
    /// Base colour by shape (the first recorded wins).
    color: HashMap<ShapeId, Rgba>,
}

impl TimelineIndex {
    fn build(effects: &[Effect], groups: &[(ShapeId, usize, usize)], colors: &[(ShapeId, Rgba)]) -> TimelineIndex {
        let mut ix = TimelineIndex::default();
        for (i, e) in effects.iter().enumerate() {
            ix.by_target.entry((e.timing.shape, e.timing.paragraph)).or_default().push(i);
            ix.by_shape.entry(e.timing.shape).or_default().push(i);
        }
        for &(shape, member, owner) in groups {
            ix.owner.entry((shape, member)).or_insert(owner);
            ix.groups_by_shape.entry(shape).or_default().push((member, owner));
        }
        for &(shape, c) in colors {
            ix.color.entry(shape).or_insert(c);
        }
        ix
    }
}

enum Status {
    NotStarted,
    Active(f64),
    Done,
}

fn secs(ms: u32) -> f64 {
    ms as f64 / 1000.0
}

fn finite_or(v: f64, d: f64) -> f64 {
    if v.is_finite() { v } else { d }
}

fn sanitize_xfrm(x: Xfrm, sw: f64, sh: f64) -> Xfrm {
    Xfrm {
        x: finite_or(x.x, 0.0).clamp(-1.0e7, 1.0e7),
        y: finite_or(x.y, 0.0).clamp(-1.0e7, 1.0e7),
        w: finite_or(x.w, sw / 2.0).clamp(0.0, 1.0e7),
        h: finite_or(x.h, sh / 2.0).clamp(0.0, 1.0e7),
        ..x
    }
}

fn default_scheme() -> Option<ColorScheme> {
    deckcraft_model::theme::builtin_color_schemes().into_iter().next()
}

impl Timeline {
    /// Build the timeline of `slide` (slide size in points). Placeholders without their own box
    /// get a centred default box; colours resolve against the default theme.
    pub fn new(slide: &Slide, slide_w: f64, slide_h: f64) -> Timeline {
        Timeline::with(slide, slide_w, slide_h, None, &|_| None)
    }

    /// Build with a colour scheme for theme colours and a lookup for shape boxes (placeholders
    /// inherit their box from the layout; the lookup wins over `shape.xfrm`).
    pub fn with(slide: &Slide, slide_w: f64, slide_h: f64, scheme: Option<&ColorScheme>, xfrm_of: &dyn Fn(ShapeId) -> Option<Xfrm>) -> Timeline {
        let sw = finite_or(slide_w, 960.0).clamp(1.0, 1.0e7);
        let sh = finite_or(slide_h, 540.0).clamp(1.0, 1.0e7);
        let owned;
        let scheme = match scheme {
            Some(s) => Some(s),
            None => {
                owned = default_scheme();
                owned.as_ref()
            }
        };
        let resolve = |c: &deckcraft_model::ColorRef| -> Rgba {
            match scheme {
                Some(s) => c.resolve(s, None),
                None => match &c.base {
                    deckcraft_model::ColorBase::Rgb { rgb } => *rgb,
                    _ => Rgba::rgb(0x15, 0x60, 0x82),
                },
            }
        };
        let accent = scheme.map(|s| s.get(SchemeSlot::Accent1)).unwrap_or(Rgba::rgb(0x15, 0x60, 0x82));

        let mut tl = Timeline::default();
        if slide.animations.is_empty() {
            return tl;
        }
        // Every shape by id, built once: resolving each animation's target by tree search would
        // cost shapes x animations.
        let shapes = shape_index(&slide.shapes);
        // Paragraph targets of each shape, worked out for its first by-paragraph animation only.
        let mut para_cache: HashMap<ShapeId, Arc<Vec<Option<usize>>>> = HashMap::new();
        // Group the animations by sequence in one pass, sequences in order of first appearance
        // (main first), so many distinct triggers cost linear work, not a scan per trigger.
        let mut seqs: Vec<(Sequence, Vec<usize>)> = vec![(Sequence::Main, Vec::new())];
        let mut seq_index: HashMap<Sequence, usize> = HashMap::from([(Sequence::Main, 0)]);
        for (ai, a) in slide.animations.iter().enumerate() {
            let s = a.trigger.map(Sequence::Trigger).unwrap_or(Sequence::Main);
            let i = *seq_index.entry(s).or_insert_with(|| {
                seqs.push((s, Vec::new()));
                seqs.len() - 1
            });
            if let Some((_, members)) = seqs.get_mut(i) {
                members.push(ai);
            }
        }
        // Paragraph groups already recorded (the list itself keeps their order).
        let mut grouped: HashSet<(ShapeId, usize, usize)> = HashSet::new();
        for (seq, members) in seqs {
            if tl.effects.len() >= MAX_EFFECTS {
                break;
            }
            let mut steps: Vec<StepInfo> = Vec::new();
            let mut group_begin = 0.0f64;
            let mut max_end = 0.0f64;
            for ai in members {
                let Some(a) = slide.animations.get(ai) else { continue };
                if tl.effects.len() >= MAX_EFFECTS {
                    break;
                }
                let shape = shapes.get(&a.shape).copied();
                // Paragraph targets of this animation.
                let paras: Arc<Vec<Option<usize>>> = match (a.paragraph, a.text_build) {
                    (Some(p), _) => Arc::new(vec![Some(p as usize)]),
                    (None, TextBuild::ByParagraph) => {
                        let cached = para_cache.entry(a.shape).or_insert_with(|| {
                            let mut v: Vec<Option<usize>> = vec![];
                            if let Some(t) = shape.and_then(|s| s.text.as_ref()) {
                                for (i, role) in paragraph_build_roles(t).into_iter().enumerate() {
                                    match role {
                                        BuildRole::Top => v.push(Some(i)),
                                        BuildRole::Member(o) => {
                                            if grouped.insert((a.shape, i, o)) {
                                                tl.groups.push((a.shape, i, o));
                                            }
                                        }
                                        BuildRole::Unowned => {}
                                    }
                                }
                            }
                            if v.is_empty() {
                                v.push(None)
                            }
                            Arc::new(v)
                        });
                        Arc::clone(cached)
                    }
                    _ => Arc::new(vec![None]),
                };
                let xf = xfrm_of(a.shape).or_else(|| shape.and_then(|s| s.xfrm)).unwrap_or(Xfrm::new(sw / 4.0, sh / 4.0, sw / 2.0, sh / 2.0));
                let xf = sanitize_xfrm(xf, sw, sh);
                let spec = Arc::new(build_spec(a, xf, sw, sh, &resolve));
                let dur = secs(a.duration_ms);
                let delay = secs(a.delay_ms);
                let until_next_click = a.repeat == u32::MAX;
                let iterations = if until_next_click || a.repeat <= 1 { 1.0 } else { (a.repeat as f64).min(MAX_ITERATIONS) };
                let span = dur * if a.auto_reverse { 2.0 } else { 1.0 } * iterations;
                let after = match a.after.as_deref() {
                    Some("hide") => After::Hide,
                    Some("hideOnNextClick") => After::HideOnNextClick,
                    Some(c) => Rgba::from_hex(c).map(After::Dim).unwrap_or(After::None),
                    None => After::None,
                };
                let mut first_start = 0.0;
                for (k, para) in paras.iter().enumerate() {
                    let mode = if k == 0 {
                        a.start
                    } else if a.start == AnimStart::OnClick {
                        AnimStart::OnClick
                    } else {
                        AnimStart::WithPrevious
                    };
                    let first_of_seq = steps.is_empty();
                    let mode = if first_of_seq && matches!(seq, Sequence::Trigger(_)) { AnimStart::OnClick } else { mode };
                    match mode {
                        AnimStart::OnClick => {
                            steps.push(StepInfo { auto: false, duration: 0.0 });
                            group_begin = 0.0;
                            max_end = 0.0;
                        }
                        AnimStart::WithPrevious => {
                            if first_of_seq {
                                steps.push(StepInfo { auto: true, duration: 0.0 });
                            }
                        }
                        AnimStart::AfterPrevious => {
                            if first_of_seq {
                                steps.push(StepInfo { auto: true, duration: 0.0 });
                            }
                            group_begin = max_end;
                        }
                    }
                    let start = if k > 0 && a.start != AnimStart::OnClick { first_start + k as f64 * PARAGRAPH_STAGGER } else { group_begin + delay };
                    if k == 0 {
                        first_start = start;
                    }
                    let end = start + span;
                    max_end = max_end.max(end);
                    let step = steps.len().saturating_sub(1);
                    if let Some(s) = steps.last_mut() {
                        // Media plays on by itself once started: it holds back an After Previous
                        // effect (`max_end`), but not the next click (#116).
                        s.duration = s.duration.max(if a.class == AnimClass::Media { start } else { end });
                    }
                    tl.effects.push(Effect {
                        timing: EffectTiming {
                            anim: ai,
                            shape: a.shape,
                            paragraph: *para,
                            sequence: seq,
                            step,
                            start,
                            duration: dur,
                            iterations,
                            until_next_click,
                            auto_reverse: a.auto_reverse,
                            rewind: a.rewind,
                            span,
                            class: a.class,
                        },
                        spec: Arc::clone(&spec),
                        after: after.clone(),
                    });
                }
            }
            match seq {
                Sequence::Main => tl.main = steps,
                Sequence::Trigger(s) => tl.triggers.push((s, steps)),
            }
        }
        // Base colours of animated shapes.
        let mut colored: HashSet<ShapeId> = HashSet::new();
        for e in &tl.effects {
            let id = e.timing.shape;
            if !colored.insert(id) {
                continue;
            }
            let c = match shapes.get(&id).copied().and_then(|s| s.fill.as_ref()) {
                Some(Fill::Solid { color }) => resolve(color),
                _ => accent,
            };
            tl.colors.push((id, c));
        }
        tl.index = TimelineIndex::build(&tl.effects, &tl.groups, &tl.colors);
        tl
    }

    /// Number of steps in the main sequence (clicks, plus an automatic first step when the first
    /// effect starts with/after previous).
    pub fn steps(&self) -> usize {
        self.main.len()
    }

    /// Does main step `i` start by itself (no click), as the slide appears?
    pub fn step_is_auto(&self, i: usize) -> bool {
        self.main.get(i).is_some_and(|s| s.auto)
    }

    /// Duration of main step `i` in seconds (0 when out of range).
    pub fn step_duration(&self, i: usize) -> f64 {
        self.main.get(i).map(|s| s.duration).unwrap_or(0.0)
    }

    /// Media effects (Play, Pause, Stop) of main step `step` that start after `from` and by `to`
    /// seconds into it, in order: what a show running the step from `from` to `to` sets off.
    pub fn media_cues(&self, step: usize, from: f64, to: f64) -> Vec<EffectTiming> {
        let mut v: Vec<EffectTiming> = self
            .effects
            .iter()
            .map(|e| &e.timing)
            .filter(|e| e.class == AnimClass::Media && e.sequence == Sequence::Main && e.step == step && e.start > from && e.start <= to)
            .cloned()
            .collect();
        v.sort_by(|a, b| a.start.total_cmp(&b.start));
        v
    }

    /// Total duration of the main sequence played without pauses.
    pub fn total_duration(&self) -> f64 {
        self.main.iter().map(|s| s.duration).sum()
    }

    /// Map a time on the "play everything" clock to `(step, t)` for [`Timeline::state`].
    pub fn locate(&self, time: f64) -> (usize, f64) {
        let mut t = finite_or(time, 0.0).max(0.0);
        for (i, s) in self.main.iter().enumerate() {
            if t < s.duration || (t == 0.0 && s.duration == 0.0 && i == 0) {
                return (i, t);
            }
            t -= s.duration;
        }
        (self.main.len(), 0.0)
    }

    /// Where effect `anim` (index into `slide.animations`) starts on the play-everything clock:
    /// Animation Pane ▸ Play From. `None` for a triggered effect or one that isn't on the slide.
    pub fn start_of(&self, anim: usize) -> Option<f64> {
        let e = self
            .effects
            .iter()
            .map(|e| &e.timing)
            .filter(|e| e.anim == anim && e.sequence == Sequence::Main)
            .min_by(|a, b| a.step.cmp(&b.step).then(a.start.total_cmp(&b.start)))?;
        let before: f64 = self.main.iter().take(e.step).map(|s| s.duration).sum();
        Some(before + e.start)
    }

    /// Shapes that start an interactive sequence when clicked.
    pub fn trigger_shapes(&self) -> Vec<ShapeId> {
        self.triggers.iter().map(|(s, _)| *s).collect()
    }

    /// Steps (clicks on the trigger) of a triggered sequence.
    pub fn trigger_steps(&self, trigger: ShapeId) -> usize {
        self.triggers.iter().find(|(s, _)| *s == trigger).map(|(_, v)| v.len()).unwrap_or(0)
    }

    pub fn trigger_step_duration(&self, trigger: ShapeId, i: usize) -> f64 {
        self.triggers.iter().find(|(s, _)| *s == trigger).and_then(|(_, v)| v.get(i)).map(|s| s.duration).unwrap_or(0.0)
    }

    /// All effect timings in sequence order.
    pub fn effects(&self) -> Vec<EffectTiming> {
        self.effects.iter().map(|e| e.timing.clone()).collect()
    }

    /// Is anything animated on this slide?
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// Paragraph indices of `shape` animated separately (by-paragraph builds).
    pub fn paragraph_targets(&self, shape: ShapeId) -> Vec<usize> {
        let mine = self.index.by_shape.get(&shape).map_or(&[][..], Vec::as_slice);
        let mut v: Vec<usize> = mine.iter().filter_map(|&i| self.effects.get(i)).filter_map(|e| e.timing.paragraph).collect();
        let owners: HashSet<usize> = v.iter().copied().collect();
        if let Some(groups) = self.index.groups_by_shape.get(&shape) {
            v.extend(groups.iter().filter(|(_, owner)| owners.contains(owner)).map(|(member, _)| *member));
        }
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Shapes with any effect.
    pub fn animated_shapes(&self) -> Vec<ShapeId> {
        let mut seen: HashSet<ShapeId> = HashSet::new();
        self.effects.iter().map(|e| e.timing.shape).filter(|s| seen.insert(*s)).collect()
    }

    /// State of `shape` (or of one of its paragraphs, `para`) when `step` main steps have been
    /// fully played and step `step` has been playing for `t` seconds (`t <= 0`: not started;
    /// `step >= steps()`: everything done). Paragraph states are relative to the shape's own state
    /// (compose them). Triggered sequences are considered not played.
    pub fn state(&self, shape: ShapeId, para: Option<usize>, step: usize, t: f64) -> AnimState {
        self.state_with(shape, para, step, t, &[])
    }

    /// [`Timeline::state`] with the playheads of triggered sequences.
    pub fn state_with(&self, shape: ShapeId, para: Option<usize>, step: usize, t: f64, triggers: &[TriggerPlay]) -> AnimState {
        self.state_ordered(shape, para, step, t, triggers, &[])
    }

    /// [`Timeline::state_with`] applying the effects of triggered sequences in the order their steps
    /// were started (`stamps`), after those of the main sequence. Without stamps they apply in
    /// Animation Pane order.
    pub fn state_ordered(
        &self,
        shape: ShapeId,
        para: Option<usize>,
        step: usize,
        t: f64,
        triggers: &[TriggerPlay],
        stamps: &[StepStamp],
    ) -> AnimState {
        let para = para.map(|p| self.index.owner.get(&(shape, p)).copied().unwrap_or(p));
        let mine = self.index.by_target.get(&(shape, para)).map_or(&[][..], Vec::as_slice).iter().filter_map(|&i| self.effects.get(i));
        let base_color = self.index.color.get(&shape).copied().unwrap_or(Rgba::rgb(0x15, 0x60, 0x82));
        let mut st = AnimState::default();
        // Initial visibility comes from the first effect in the Animation Pane order.
        let mut ordered: Vec<&Effect> = mine.collect();
        ordered.sort_by_key(|e| e.timing.anim);
        if let Some(e0) = ordered.first()
            && e0.timing.class == AnimClass::Entrance
        {
            st.visible = false;
        }
        // Then play in time order: main sequence first, then triggered sequences as they were clicked.
        if !stamps.is_empty() {
            let when = |e: &Effect| match e.timing.sequence {
                Sequence::Main => 0,
                Sequence::Trigger(tr) => {
                    stamps.iter().find(|s| s.trigger == tr && s.step == e.timing.step).map_or(u64::MAX, |s| s.at.saturating_add(1))
                }
            };
            ordered.sort_by_key(|e| (when(e), e.timing.anim));
        }
        for e in &ordered {
            let (pstep, pt) = match e.timing.sequence {
                Sequence::Main => (step, t),
                Sequence::Trigger(tr) => triggers.iter().find(|p| p.trigger == tr).map(|p| (p.step, p.t)).unwrap_or((0, 0.0)),
            };
            let cur = st.tint.unwrap_or(base_color);
            let revert = e.timing.rewind || e.timing.auto_reverse;
            match status(&e.timing, pstep, pt) {
                Status::NotStarted => {}
                Status::Active(p) => {
                    if e.timing.class == AnimClass::Entrance || e.timing.class == AnimClass::Exit {
                        st.visible = true;
                    }
                    st = st.compose(&e.spec.eval(p, cur));
                }
                Status::Done => {
                    match e.timing.class {
                        AnimClass::Entrance => st.visible = !revert,
                        AnimClass::Exit => st.visible = revert,
                        AnimClass::Emphasis | AnimClass::Path if !revert => {
                            let end = e.spec.end_state(cur);
                            st = st.compose(&end);
                        }
                        _ => {}
                    }
                    match e.after {
                        After::Hide => st.visible = false,
                        After::HideOnNextClick if pstep > e.timing.step => st.visible = false,
                        After::Dim(c) if pstep > e.timing.step => st.tint = Some(c),
                        _ => {}
                    }
                }
            }
        }
        st.sanitized()
    }

    /// State on the "play everything" clock (Animation Pane ▸ Play All, preview).
    pub fn state_at(&self, shape: ShapeId, para: Option<usize>, time: f64) -> AnimState {
        let (s, t) = self.locate(time);
        self.state(shape, para, s, t)
    }
}

fn status(e: &EffectTiming, step: usize, t: f64) -> Status {
    if e.step < step {
        return Status::Done;
    }
    if e.step > step {
        return Status::NotStarted;
    }
    let t = finite_or(t, 0.0);
    if t <= 0.0 {
        return Status::NotStarted;
    }
    let local = t - e.start;
    if local < 0.0 {
        return Status::NotStarted;
    }
    let dur = e.duration;
    if dur.is_nan() || dur <= 0.0 || !dur.is_finite() {
        return Status::Done;
    }
    if !e.until_next_click && local >= e.span {
        return Status::Done;
    }
    let period = if e.auto_reverse { 2.0 * dur } else { dur };
    let c = local % period;
    let c = if c.is_finite() { c } else { 0.0 };
    let p = if c > dur { 2.0 - c / dur } else { c / dur };
    Status::Active(p.clamp(0.0, 1.0))
}

/// Shapes by id in the order `Slide::shape` searches (a shape before its children, groups at most
/// 64 deep); the first shape with an id wins a duplicate.
fn shape_index(shapes: &[Shape]) -> HashMap<ShapeId, &Shape> {
    fn walk<'a>(shapes: &'a [Shape], depth: usize, out: &mut HashMap<ShapeId, &'a Shape>) {
        if depth > 64 {
            return;
        }
        for s in shapes {
            out.entry(s.id).or_insert(s);
            walk(s.children(), depth + 1, out);
        }
    }
    let mut out = HashMap::new();
    walk(shapes, 0, &mut out);
    out
}

fn build_spec(a: &Animation, xf: Xfrm, sw: f64, sh: f64, resolve: &dyn Fn(&deckcraft_model::ColorRef) -> Rgba) -> EffectSpec {
    let effect = if a.class == AnimClass::Exit { exit_base(&a.effect).to_string() } else { a.effect.clone() };
    let path = if a.class == AnimClass::Path { Some(motion_path(&a.effect, &a.option, a.path.as_deref())) } else { None };
    EffectSpec {
        class: a.class,
        effect,
        // Sink Down is Rise Up played backwards, leaving downward.
        option: if a.effect == "sinkDown" { "d".into() } else { a.option.clone() },
        accel: a.smooth_start,
        decel: a.smooth_end,
        bounce: a.bounce_end,
        amount: a.amount,
        color: a.color.as_ref().map(resolve),
        path,
        xf,
        slide_w: sw,
        slide_h: sh,
    }
}
