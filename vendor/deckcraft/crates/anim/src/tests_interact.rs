//! Clicks, triggered animations and what the show remembers about visited slides.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use deckcraft_model::text::Action;
use deckcraft_model::{AnimClass, AnimStart, Animation, Presentation, Shape, ShapeId, Slide, SlideId, Transition, Xfrm};

use crate::{Click, HitShape, LinkJump, ShowAction, ShowState, Target, Timeline};

fn shape(id: u32) -> Shape {
    Shape { id: ShapeId(id), name: format!("S{id}"), xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)), ..Default::default() }
}

/// A triggered exit (`Disappear` when `ms` is 0, else a fade out) of `target`, started by clicking `trigger`.
fn exit_on_click(trigger: u32, target: u32, ms: u32) -> Animation {
    let effect = if ms == 0 { "disappear" } else { "fadeOut" };
    Animation {
        shape: ShapeId(target),
        class: AnimClass::Exit,
        effect: effect.into(),
        start: AnimStart::OnClick,
        duration_ms: ms,
        trigger: Some(ShapeId(trigger)),
        ..Default::default()
    }
}

fn slide(id: u32, shapes: Vec<Shape>, animations: Vec<Animation>, advance_on_click: bool) -> Arc<Slide> {
    let transition = (!advance_on_click).then(|| Transition { advance_on_click: false, ..Default::default() });
    Arc::new(Slide { id: SlideId(id), shapes, animations, transition, ..Default::default() })
}

fn to_slide(n: u32) -> Option<Action> {
    Some(Action::Slide { slide: SlideId(n) })
}

fn hit(id: u32, link: Option<Action>) -> HitShape {
    HitShape { id: ShapeId(id), parent: None, is_group: false, link, under_pointer: true }
}

/// Test driver: a show plus the click helpers the slide show window uses.
struct Run {
    pres: Presentation,
    show: ShowState,
    /// Hit shapes of each slide, back to front.
    hits: Vec<Vec<HitShape>>,
    now: f64,
}

impl Run {
    fn new(pres: Presentation, hits: Vec<Vec<HitShape>>) -> Run {
        let show = ShowState::new(&pres);
        Run { pres, show, hits, now: 0.0 }
    }

    fn tl(&self) -> Timeline {
        ShowState::timeline(&self.pres, self.show.slide)
    }

    fn visible(&self, id: ShapeId) -> bool {
        let st = self.show.anim_state(&self.tl(), id, 0.0);
        st.visible && st.opacity > 0.0
    }

    fn opacity(&self, id: u32) -> f64 {
        self.show.anim_state(&self.tl(), ShapeId(id), 0.0).opacity
    }

    fn click(&mut self) -> Click {
        let (tl, hits) = (self.tl(), self.hits[self.show.slide].clone());
        let vis: Vec<(ShapeId, bool)> = hits.iter().map(|h| (h.id, self.visible(h.id))).collect();
        let visible = move |id: ShapeId| vis.iter().find(|v| v.0 == id).is_none_or(|v| v.1);
        self.show.click(&self.pres, &tl, &hits, &visible, self.now, None)
    }

    fn target(&self) -> Option<Target> {
        let (tl, hits) = (self.tl(), &self.hits[self.show.slide]);
        self.show.target(&self.pres, &tl, hits, &|id| self.visible(id))
    }

    /// Let `secs` pass on the shown slide, in small frames.
    fn wait(&mut self, secs: f64) {
        let end = self.now + secs;
        while self.now < end - 1e-9 {
            self.now = (self.now + 0.05).min(end);
            let tl = self.tl();
            self.show.tick(&tl, self.now);
        }
    }
}

/// The test deck: slide 1 has a text box (3), a linked button that also disappears (2) and a
/// picture on top of both that disappears (4); slide 2 has a button back to slide 1.
fn two_slides(advance_on_click: bool) -> Run {
    let s1 = slide(1000, vec![shape(3), shape(2), shape(4)], vec![exit_on_click(4, 4, 0), exit_on_click(2, 2, 0)], advance_on_click);
    let s2 = slide(1001, vec![shape(1)], vec![], advance_on_click);
    let pres = Presentation { slides: vec![s1, s2], ..Default::default() };
    Run::new(pres, vec![vec![hit(3, None), hit(2, to_slide(1001)), hit(4, None)], vec![hit(1, to_slide(1000))]])
}

#[test]
fn hidden_shapes_stay_hidden_when_the_show_comes_back() {
    let mut r = two_slides(false);
    assert_eq!(r.target().map(|t| t.shape), Some(ShapeId(4)), "the picture is on top");
    // Click the picture: it disappears; the button beneath it is now the target.
    assert_eq!(r.click(), Click::Animated);
    assert!(!r.visible(ShapeId(4)));
    assert_eq!(r.target().map(|t| t.shape), Some(ShapeId(2)));
    // Click the button: its animation starts and the link is followed.
    assert_eq!(r.click(), Click::Show(ShowAction::Slide(1)));
    assert_eq!(r.show.slide, 1);
    // Slide 2: the button goes back.
    assert_eq!(r.click(), Click::Show(ShowAction::Slide(0)));
    assert_eq!(r.show.slide, 0);
    // Both are still gone, and nothing on the slide can be clicked any more.
    assert!(!r.visible(ShapeId(4)) && !r.visible(ShapeId(2)) && r.visible(ShapeId(3)));
    assert_eq!(r.target(), None);
    // "Advance on click" is off: a click anywhere does nothing, however often.
    for _ in 0..3 {
        assert_eq!(r.click(), Click::None);
    }
    assert_eq!(r.show.slide, 0);
}

#[test]
fn background_click_advances_when_the_slide_allows_it() {
    let mut r = two_slides(true);
    r.click(); // the picture disappears
    r.click(); // the button disappears and links to slide 2
    r.click(); // the button there links back
    assert_eq!(r.show.slide, 0);
    assert_eq!(r.target(), None);
    assert_eq!(r.click(), Click::Show(ShowAction::Slide(1)));
}

#[test]
fn advance_on_click_off_still_plays_animation_steps() {
    let main = Animation { shape: ShapeId(1), class: AnimClass::Entrance, effect: "fade".into(), duration_ms: 500, ..Default::default() };
    let s1 = slide(1000, vec![shape(1)], vec![main], false);
    let s2 = slide(1001, vec![], vec![], false);
    let pres = Presentation { slides: vec![s1, s2], ..Default::default() };
    let mut r = Run::new(pres, vec![vec![hit(1, None)], vec![]]);
    // The first click plays the step, the next finishes it; then the slide stays.
    assert!(matches!(r.click(), Click::Show(ShowAction::PlayStep(0))));
    assert!(matches!(r.click(), Click::Show(ShowAction::FinishStep(0))));
    assert_eq!(r.click(), Click::None);
    assert_eq!(r.show.slide, 0);
    // The arrow keys still move on.
    assert_eq!(r.show.next(&r.pres), ShowAction::Slide(1));
}

#[test]
fn a_shape_without_an_action_blocks_the_one_beneath_it() {
    let s = slide(1000, vec![shape(1), shape(2)], vec![], true);
    let pres = Presentation { slides: vec![s, slide(1001, vec![], vec![], true)], ..Default::default() };
    let mut r = Run::new(pres, vec![vec![hit(1, to_slide(1001)), hit(2, None)], vec![]]);
    assert_eq!(r.target(), None, "the plain shape on top blocks the button");
    // So the click is a background click, not the button's link.
    assert_eq!(r.click(), Click::Show(ShowAction::Slide(1)));
    // Reversed, the button is on top and is the target.
    let s = slide(1000, vec![shape(2), shape(1)], vec![], true);
    let pres = Presentation { slides: vec![s, slide(1001, vec![], vec![], true)], ..Default::default() };
    let r = Run::new(pres, vec![vec![hit(2, None), hit(1, to_slide(1001))], vec![]]);
    assert_eq!(r.target().map(|t| t.shape), Some(ShapeId(1)));
}

#[test]
fn groups_answer_for_their_children() {
    let s = slide(1000, vec![shape(1)], vec![], true);
    let pres = Presentation { slides: vec![s, slide(1001, vec![], vec![], true)], ..Default::default() };
    let group = |link| HitShape { id: ShapeId(10), parent: None, is_group: true, link, under_pointer: true };
    let child = HitShape { id: ShapeId(11), parent: Some(1), is_group: false, link: None, under_pointer: true };
    // A child of a group with an action acts as the group.
    let r = Run::new(pres.clone(), vec![vec![hit(5, None), group(to_slide(1001)), child.clone()], vec![]]);
    assert_eq!(r.target().map(|t| t.shape), Some(ShapeId(10)));
    // Empty space inside the box of a group without an action blocks nothing.
    let behind = hit(5, to_slide(1001));
    let r = Run::new(pres, vec![vec![behind, group(None)], vec![]]);
    assert_eq!(r.target().map(|t| t.shape), Some(ShapeId(5)));
}

#[test]
fn pointer_outside_a_shape_is_not_on_it() {
    let s = slide(1000, vec![shape(1)], vec![], true);
    let pres = Presentation { slides: vec![s], ..Default::default() };
    let mut h = hit(1, to_slide(1000));
    h.under_pointer = false;
    let r = Run::new(pres, vec![vec![h]]);
    assert_eq!(r.target(), None);
}

#[test]
fn each_click_plays_one_step_of_a_trigger_and_a_click_completes_a_playing_step() {
    // Clicking shape 1 fades shape 2 out over 5 s, then shape 3 over 5 s.
    let s = slide(1000, vec![shape(1), shape(2), shape(3)], vec![exit_on_click(1, 2, 5000), exit_on_click(1, 3, 5000)], true);
    let pres = Presentation { slides: vec![s], ..Default::default() };
    let mut r = Run::new(pres, vec![vec![hit(2, None), hit(3, None), hit(1, None)]]);
    assert_eq!(r.target().map(|t| t.shape), Some(ShapeId(1)));
    assert_eq!(r.click(), Click::Animated);
    r.wait(2.0);
    assert!(r.show.trigger_playing());
    assert!(r.opacity(2) < 1.0 && r.opacity(2) > 0.0, "part way through the fade");
    assert!(r.visible(ShapeId(3)));
    // A click on the shape while its step plays completes the step at once...
    assert_eq!(r.click(), Click::Animated);
    assert!(!r.visible(ShapeId(2)) && !r.show.trigger_playing());
    // ...and the next click plays the next step.
    assert_eq!(r.click(), Click::Animated);
    r.wait(5.2);
    assert!(!r.visible(ShapeId(3)));
    // All steps have played, and the shape still answers: its sequence starts over.
    assert_eq!(r.target().map(|t| t.shape), Some(ShapeId(1)));
    assert_eq!(r.click(), Click::Animated);
    assert!(r.visible(ShapeId(2)) && r.visible(ShapeId(3)), "the first lap's effects are undone");
    r.wait(5.2);
    assert!(!r.visible(ShapeId(2)) && r.visible(ShapeId(3)));
}

#[test]
fn a_ring_of_triggers_goes_round_and_round() {
    // Soccer's score: clicking shape k hides it and shows shape k+1; the last shows the first again.
    let ring = 4u32;
    let anims: Vec<Animation> = (1..=ring)
        .flat_map(|k| {
            let next = k % ring + 1;
            let show = Animation {
                shape: ShapeId(next),
                class: AnimClass::Entrance,
                effect: "appear".into(),
                start: AnimStart::WithPrevious,
                duration_ms: 0,
                trigger: Some(ShapeId(k)),
                ..Default::default()
            };
            [exit_on_click(k, k, 0), show]
        })
        .collect();
    let shapes: Vec<Shape> = (1..=ring).map(shape).collect();
    let s = slide(1000, shapes, anims, true);
    let pres = Presentation { slides: vec![s], ..Default::default() };
    let hits: Vec<HitShape> = (1..=ring).map(|k| hit(k, None)).collect();
    let mut r = Run::new(pres, vec![hits]);
    let showing = |r: &Run| -> Vec<u32> { (1..=ring).filter(|k| r.visible(ShapeId(*k))).collect() };
    assert_eq!(showing(&r), vec![1]);
    for lap in 0..3 {
        for k in 1..=ring {
            assert_eq!(showing(&r), vec![k], "lap {lap}, before click {k}");
            assert_eq!(r.target().map(|t| t.shape), Some(ShapeId(k)));
            assert_eq!(r.click(), Click::Animated);
        }
    }
    assert_eq!(showing(&r), vec![1]);
}

#[test]
fn a_link_is_followed_at_once_and_the_animation_goes_on_where_it_stopped() {
    // Shape 1 fades shape 4 over 5 s; shape 2 links to slide 2.
    let s1 = slide(1000, vec![shape(1), shape(2), shape(4)], vec![exit_on_click(1, 4, 5000)], false);
    let s2 = slide(1001, vec![shape(5)], vec![], false);
    let pres = Presentation { slides: vec![s1, s2], ..Default::default() };
    let mut r = Run::new(pres, vec![vec![hit(4, None), hit(2, to_slide(1001)), hit(1, None)], vec![hit(5, to_slide(1000))]]);
    // Click the animated shape, wait 2 s, click the link.
    r.click();
    r.wait(2.0);
    assert_eq!(r.show.trigger_plays()[0].t.round(), 2.0);
    // The second shape is not over the first one, so click the link directly.
    let tl = r.tl();
    let hits = vec![hit(2, to_slide(1001))];
    assert_eq!(r.show.click(&r.pres, &tl, &hits, &|_| true, r.now, None), Click::Show(ShowAction::Slide(1)));
    // However long the show stays away, the animation does not run.
    r.wait(60.0);
    assert_eq!(r.click(), Click::Show(ShowAction::Slide(0)));
    // Back on the slide it has 3 s left.
    assert!((r.show.trigger_plays()[0].t - 2.0).abs() < 0.1);
    assert!(r.show.trigger_playing());
    r.wait(2.5);
    assert!(r.show.trigger_playing() && r.opacity(4) > 0.0);
    r.wait(1.0);
    assert!(!r.show.trigger_playing() && !r.visible(ShapeId(4)));
}

#[test]
fn a_shape_with_a_link_and_an_animation_follows_the_link_without_waiting() {
    let s1 = slide(1000, vec![shape(2)], vec![exit_on_click(2, 2, 5000)], false);
    let s2 = slide(1001, vec![shape(5)], vec![], false);
    let pres = Presentation { slides: vec![s1, s2], ..Default::default() };
    let mut r = Run::new(pres, vec![vec![hit(2, to_slide(1001))], vec![hit(5, to_slide(1000))]]);
    assert_eq!(r.click(), Click::Show(ShowAction::Slide(1)));
    r.wait(30.0);
    assert_eq!(r.click(), Click::Show(ShowAction::Slide(0)));
    // The whole animation is still ahead.
    assert!(r.visible(ShapeId(2)) && r.opacity(2) > 0.95);
    assert!(r.show.trigger_playing());
    r.wait(5.5);
    assert!(!r.visible(ShapeId(2)));
}

#[test]
fn a_slide_comes_back_the_way_it_was_left() {
    let steps: Vec<Animation> = (1..=3)
        .map(|i| Animation { shape: ShapeId(i), class: AnimClass::Entrance, effect: "appear".into(), duration_ms: 0, ..Default::default() })
        .collect();
    let s1 = slide(1000, vec![shape(1), shape(2), shape(3)], steps, true);
    let s2 = slide(1001, vec![], vec![], true);
    let s3 = slide(1002, vec![], vec![], true);
    let pres = Presentation { slides: vec![s1, s2, s3], ..Default::default() };
    let mut s = ShowState::new(&pres);
    s.next(&pres);
    s.step_done(&pres);
    assert_eq!(s.step, 1);
    // A link away and back: still after step 1.
    assert_eq!(s.follow(&pres, &Action::Slide { slide: SlideId(1002) }, None), LinkJump::Show(ShowAction::Slide(2)));
    assert_eq!(s.follow(&pres, &Action::Slide { slide: SlideId(1000) }, None), LinkJump::Show(ShowAction::Slide(0)));
    assert_eq!((s.step, s.playing), (1, false));
    // So does the back arrow.
    s.goto(&pres, 1);
    assert_eq!(s.prev(&pres), ShowAction::Slide(0));
    assert_eq!(s.step, 1);
    // A slide the show has not been to is entered at its start; going back to one shows it fully
    // built.
    let mut s = ShowState::new(&pres);
    s.goto(&pres, 2);
    assert_eq!(s.prev(&pres), ShowAction::Slide(1));
    assert_eq!(s.step, ShowState::steps(&pres, 1));
}

#[test]
fn a_slide_entered_going_forward_again_starts_over() {
    // #2090: the show advances to slide 2 a second time; its triggered animation is undone too.
    let s1 = slide(1000, vec![], vec![], true);
    let s2 = slide(1001, vec![shape(4)], vec![exit_on_click(4, 4, 0)], true);
    let pres = Presentation { slides: vec![s1, s2], ..Default::default() };
    let mut r = Run::new(pres, vec![vec![], vec![hit(4, None)]]);
    assert_eq!(r.show.next(&r.pres), ShowAction::Slide(1));
    assert_eq!(r.click(), Click::Animated);
    assert!(!r.visible(ShapeId(4)));
    assert_eq!(r.show.prev(&r.pres), ShowAction::Slide(0));
    assert_eq!(r.show.prev(&r.pres), ShowAction::None);
    assert_eq!(r.show.next(&r.pres), ShowAction::Slide(1));
    assert!(r.visible(ShapeId(4)), "the shape is back");
    assert!(r.show.trigger_plays().is_empty());
}

#[test]
fn a_playing_main_step_is_finished_when_the_show_leaves() {
    let a = Animation { shape: ShapeId(1), class: AnimClass::Entrance, effect: "fade".into(), duration_ms: 500, ..Default::default() };
    let s1 = slide(1000, vec![shape(1)], vec![a], true);
    let s2 = slide(1001, vec![], vec![], true);
    let pres = Presentation { slides: vec![s1, s2], ..Default::default() };
    let mut s = ShowState::new(&pres);
    assert_eq!(s.next(&pres), ShowAction::PlayStep(0));
    s.goto(&pres, 1);
    s.goto(&pres, 0);
    assert_eq!((s.step, s.playing), (1, false));
}

#[test]
fn show_without_animation_ignores_triggers() {
    let s = slide(1000, vec![shape(1), shape(2)], vec![exit_on_click(1, 2, 0)], true);
    let mut pres = Presentation { slides: vec![s], ..Default::default() };
    pres.show.without_animation = true;
    let r = Run::new(pres, vec![vec![hit(2, None), hit(1, None)]]);
    assert_eq!(r.target(), None);
}

#[test]
fn clocks_and_pointers_with_hostile_numbers_are_harmless() {
    let mut r = two_slides(true);
    let tl = r.tl();
    for now in [f64::NAN, f64::INFINITY, -1.0e300, 1.0e300, 0.0] {
        r.show.tick(&tl, now);
        let _ = r.show.click(&r.pres, &tl, &[], &|_| true, now, None);
    }
    // A parent index that points nowhere, or in a circle, is not followed forever.
    let bad = vec![
        HitShape { id: ShapeId(1), parent: Some(7), is_group: true, link: None, under_pointer: true },
        HitShape { id: ShapeId(2), parent: Some(2), is_group: false, link: None, under_pointer: true },
    ];
    assert_eq!(r.show.target(&r.pres, &tl, &bad, &|_| true), None);
}
