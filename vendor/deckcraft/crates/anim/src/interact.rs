//! Clicks in a slide show: which shape a click lands on, what it starts (a triggered animation, a
//! link, the next step) and what the show remembers about each slide it has visited.
//!
//! The rules follow PowerPoint, as observed on its slide show:
//! - the topmost visible shape under the pointer decides; a shape without an action on top of one
//!   with an action blocks it (the click then counts as a click on the background);
//! - a shape with a triggered animation plays the next step of it per click, and starts over once all
//!   its steps have played; clicking it while a step plays completes the step at once;
//! - a shape with a link and a triggered animation follows the link at once. The animation's clock
//!   runs only while its slide is shown, so it goes on where it stopped when the show returns;
//! - with "advance on click" off a click never leaves the slide, but it still plays animation steps;
//! - a slide the show returns to by a link or by going back looks as it was left; going forward
//!   to a slide starts it over (#2090).

use deckcraft_model::text::Action;
use deckcraft_model::{Presentation, ShapeId};

use crate::{AnimState, LinkJump, ShowAction, ShowState, StepStamp, Timeline, TriggerPlay};

/// What was played on one slide.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SlideRun {
    /// The show has been on this slide.
    pub visited: bool,
    /// Main-sequence steps played when the show left the slide.
    pub step: usize,
    /// Triggered sequences started by clicks.
    pub triggers: Vec<TriggerRun>,
    /// Steps of triggered sequences started so far (the stamps of [`StepStamp`]).
    pub clock: u64,
}

/// Progress of one triggered sequence.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TriggerRun {
    pub shape: ShapeId,
    /// Clicks whose step has played to its end.
    pub done: usize,
    /// Seconds the current step has played; `None` when no step is playing.
    pub elapsed: Option<f64>,
    /// When each step of this run was started, by step.
    pub stamps: Vec<u64>,
}

/// One shape of the slide for hit testing, in drawing order (back to front; a group before its
/// children), with whether the pointer is inside its box.
#[derive(Clone, Debug, PartialEq)]
pub struct HitShape {
    pub id: ShapeId,
    /// Index of the enclosing group in the same list.
    pub parent: Option<usize>,
    pub is_group: bool,
    /// The shape's own action.
    pub link: Option<Action>,
    pub under_pointer: bool,
}

/// What the pointer is over: a shape that does something when clicked.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub shape: ShapeId,
    pub link: Option<Action>,
}

/// What a click did.
#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    /// Nothing happened.
    None,
    /// A triggered animation started or was completed: redraw.
    Animated,
    /// Navigation inside the show (already applied to the state).
    Show(ShowAction),
    /// Open this URL outside the show.
    Open(String),
    /// End the show.
    Exit,
}

impl From<LinkJump> for Click {
    fn from(j: LinkJump) -> Click {
        match j {
            LinkJump::Show(a) => Click::Show(a),
            LinkJump::Open(u) => Click::Open(u),
            LinkJump::Exit => Click::Exit,
            LinkJump::None => Click::None,
        }
    }
}

fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

impl ShowState {
    fn animated(&self, pres: &Presentation) -> bool {
        !pres.show.without_animation
    }

    /// Does clicking `shape` start a triggered sequence? A sequence can be started again once it has
    /// played to its end.
    fn trigger_live(&self, pres: &Presentation, tl: &Timeline, shape: ShapeId) -> bool {
        self.animated(pres) && tl.trigger_steps(shape) > 0
    }

    /// The playheads of the current slide's triggered sequences at the last tick, for
    /// [`Timeline::state_with`].
    pub fn trigger_plays(&self) -> Vec<TriggerPlay> {
        match self.runs.get(&self.slide) {
            Some(r) => r.triggers.iter().map(|t| TriggerPlay { trigger: t.shape, step: t.done, t: t.elapsed.unwrap_or(0.0) }).collect(),
            None => vec![],
        }
    }

    /// When the steps of the current slide's triggered sequences were started.
    pub fn trigger_stamps(&self) -> Vec<StepStamp> {
        let Some(r) = self.runs.get(&self.slide) else { return vec![] };
        r.triggers.iter().flat_map(|t| t.stamps.iter().enumerate().map(|(step, &at)| StepStamp { trigger: t.shape, step, at })).collect()
    }

    /// Is a triggered animation of the current slide playing (so the picture changes every frame)?
    pub fn trigger_playing(&self) -> bool {
        self.runs.get(&self.slide).is_some_and(|r| r.triggers.iter().any(|t| t.elapsed.is_some()))
    }

    /// Animation state of `shape` on the current slide: main step `step` with `t_main` seconds
    /// played, plus the triggered sequences.
    pub fn anim_state(&self, tl: &Timeline, shape: ShapeId, t_main: f64) -> AnimState {
        tl.state_ordered(shape, None, self.step, t_main, &self.trigger_plays(), &self.trigger_stamps())
    }

    /// Advance the clock of the playing triggered animations to `now` (seconds, any steady clock)
    /// and complete the steps that reached their end. [`Click::Animated`] when the picture changed.
    pub fn tick(&mut self, tl: &Timeline, now: f64) -> Click {
        let dt = match self.last_now {
            Some(last) if now.is_finite() => (now - last).clamp(0.0, 3600.0),
            _ => 0.0,
        };
        if now.is_finite() {
            self.last_now = Some(now);
        }
        let Some(run) = self.runs.get_mut(&self.slide) else { return Click::None };
        let mut changed = false;
        for t in run.triggers.iter_mut() {
            let Some(e) = t.elapsed else { continue };
            let e = finite_or_zero(e) + dt;
            if e >= tl.trigger_step_duration(t.shape, t.done) {
                t.done += 1;
                t.elapsed = None;
                changed = true;
            } else {
                t.elapsed = Some(e);
            }
        }
        if changed { Click::Animated } else { Click::None }
    }

    /// Index of the topmost shape a click lands on: the front-most one under the pointer that is
    /// shown. A group without an action does not count; its children and the space inside its box
    /// are not it.
    pub fn top(&self, pres: &Presentation, tl: &Timeline, shapes: &[HitShape], visible: &dyn Fn(ShapeId) -> bool) -> Option<usize> {
        let shown = |mut i: usize| -> bool {
            // A shape is shown when it and the groups around it are.
            for _ in 0..=shapes.len() {
                let Some(h) = shapes.get(i) else { return false };
                if !visible(h.id) {
                    return false;
                }
                match h.parent {
                    Some(p) => i = p,
                    None => return true,
                }
            }
            false
        };
        (0..shapes.len())
            .rev()
            .find(|&i| shapes.get(i).is_some_and(|h| h.under_pointer && !(h.is_group && !self.interactive(pres, tl, h)) && shown(i)))
    }

    fn interactive(&self, pres: &Presentation, tl: &Timeline, h: &HitShape) -> bool {
        h.link.is_some() || self.trigger_live(pres, tl, h.id)
    }

    /// The shape under the pointer that a click would act on, `None` when a click would be a click
    /// on the background. `visible` tells whether a shape is currently shown (not yet entered or
    /// already exited shapes cannot be clicked).
    pub fn target(&self, pres: &Presentation, tl: &Timeline, shapes: &[HitShape], visible: &dyn Fn(ShapeId) -> bool) -> Option<Target> {
        // The topmost shape, or the group it is in, that has an action. A shape without one blocks
        // what lies beneath it.
        let mut at = self.top(pres, tl, shapes, visible);
        for _ in 0..=shapes.len() {
            let h = shapes.get(at?)?;
            if self.interactive(pres, tl, h) {
                return Some(Target { shape: h.id, link: h.link.clone() });
            }
            at = h.parent;
        }
        None
    }

    /// A mouse click. `shapes` and `visible` describe what is under the pointer (see
    /// [`ShowState::target`]); `last_viewed` is the slide shown before this one.
    pub fn click(
        &mut self,
        pres: &Presentation,
        tl: &Timeline,
        shapes: &[HitShape],
        visible: &dyn Fn(ShapeId) -> bool,
        now: f64,
        last_viewed: Option<usize>,
    ) -> Click {
        self.tick(tl, now);
        if self.ended {
            return Click::None;
        }
        let Some(t) = self.target(pres, tl, shapes, visible) else { return self.background_click(pres) };
        let mut animated = false;
        if self.trigger_live(pres, tl, t.shape) {
            let slide = self.slide;
            let run = self.runs.entry(slide).or_default();
            let at = match run.triggers.iter().position(|r| r.shape == t.shape) {
                Some(p) => p,
                None => {
                    run.triggers.push(TriggerRun { shape: t.shape, done: 0, elapsed: None, stamps: vec![] });
                    run.triggers.len() - 1
                }
            };
            let stamp = run.clock.saturating_add(1);
            if let Some(r) = run.triggers.get_mut(at) {
                if r.elapsed.is_some() {
                    // A click on a shape whose step is playing completes the step.
                    r.done += 1;
                    r.elapsed = None;
                } else {
                    if r.done >= tl.trigger_steps(t.shape) {
                        // All steps have played: the sequence starts over.
                        r.done = 0;
                        r.stamps.clear();
                    }
                    r.stamps.push(stamp);
                    run.clock = stamp;
                    if tl.trigger_step_duration(t.shape, r.done) > 0.0 {
                        r.elapsed = Some(0.0);
                    } else {
                        r.done += 1;
                    }
                }
                animated = true;
            }
        }
        match t.link {
            // The link is followed at once, whatever the animation still has to play.
            Some(a) => self.follow(pres, &a, last_viewed).into(),
            None if animated => Click::Animated,
            None => Click::None,
        }
    }

    /// A click that is on no shape with an action: next animation step, then next slide, unless the
    /// slide has "advance on click" off, which leaves only the animation steps.
    fn background_click(&mut self, pres: &Presentation) -> Click {
        let advance = pres.slides.get(self.slide).and_then(|s| s.transition.as_ref()).is_none_or(|t| t.advance_on_click);
        let steps_left = self.playing || self.step < ShowState::steps(pres, self.slide);
        if advance || steps_left { Click::Show(self.next(pres)) } else { Click::None }
    }
}
