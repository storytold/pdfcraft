#![allow(clippy::unwrap_used)]

use egui::{Context, RawInput, Rect, pos2};
use pdfcraft_ui_egui::ask_agent::ui;

fn anchor() -> Rect {
    Rect::from_min_max(pos2(20.0, 20.0), pos2(100.0, 40.0))
}

#[test]
fn long_accepted_replies_keep_review_and_send_actions_reachable_by_wheel() {
    use pdfcraft_ui_egui::ask_agent::hub::{AgentHub, Reply, Request, Target};
    use serde_json::json;
    for failed in [false, true] {
        let hub = AgentHub::default();
        let target = Target { document: "synthetic:scroll".into(), revision: 1 };
        let id = hub
            .ask(Request {
                id: 0,
                instruction: "Review this synthetic selection".into(),
                history: vec![],
                selection: json!({"pages":[1]}),
                document: json!({}),
                target: target.clone(),
                agent: None,
            })
            .unwrap();
        hub.listen(1, None, std::time::Duration::ZERO, || true).unwrap().unwrap();
        let progress = "Progress: checking this synthetic selection.\n".repeat(80);
        for _ in 0..2 {
            hub.reply(1, id, Reply { text: &progress, done: false, ..Default::default() }, Some(&target)).unwrap();
        }
        let answer = "Review result: preserve the person's document.\n".repeat(60);
        let proposal = "Unapplied proposal: inspect this synthetic selection.\n".repeat(100);
        let reply = if failed {
            Reply { error: Some(&answer), proposal: Some(&proposal), ..Default::default() }
        } else {
            Reply { text: &answer, proposal: Some(&proposal), ..Default::default() }
        };
        hub.reply(1, id, reply, Some(&target)).unwrap();
        let snapshot = hub.snapshot(id).unwrap();
        assert_eq!(snapshot.notes, [progress.clone(), progress]);
        assert_eq!(snapshot.proposal.as_deref(), Some(proposal.as_str()));
        assert_eq!(if failed { snapshot.error.as_deref() } else { snapshot.answer.as_deref() }, Some(answer.as_str()));
        for (label, expected) in [("Keep", ui::Action::Keep), ("Undo", ui::Action::Undo), ("Send", ui::Action::Send("Follow up".into()))] {
            let ctx = Context::default();
            let screen = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(640.0, 480.0));
            let view = ui::View { snapshot: Some(&snapshot), agents: &[], actions: &[], history: &[], instruction: "" };
            let mut draft = "Follow up".to_owned();
            let mut picked = None;
            let mut tick = 0_u32;
            let mut frame = |events| {
                tick += 1;
                let input = RawInput { screen_rect: Some(screen), time: Some(f64::from(tick) / 30.0), events, ..Default::default() };
                let mut action = None;
                let output = ctx.run_pass(input, |ctx| {
                    action = ui::bubble(ctx, anchor(), &view, &mut draft, &mut picked);
                });
                let button = output.shapes.iter().find_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape
                        && text.galley.job.text == label
                    {
                        let rect = Rect::from_min_size(text.pos, text.galley.size());
                        if shape.clip_rect.intersect(screen).contains_rect(rect) {
                            return Some(rect.center());
                        }
                    }
                    None
                });
                assert!(output.platform_output.commands.is_empty(), "review text must not execute commands");
                output.drop_without_applying_deltas();
                (action, button)
            };
            for _ in 0..4 {
                frame(vec![]);
            }
            let mut button = None;
            for _ in 0..160 {
                // Native wheel input near the body's right edge also avoids the nested proposal text.
                let (_, visible) = frame(vec![
                    egui::Event::PointerMoved(pos2(390.0, 240.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -240.0),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                if visible.is_some() {
                    button = visible;
                    break;
                }
            }
            let position = button.unwrap_or_else(|| panic!("{label} is outside the screen/clip after native wheel scrolling (failed={failed})"));
            let mut actual = None;
            for pressed in [true, false] {
                let (action, _) = frame(vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton { pos: position, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
                ]);
                actual = action.or(actual);
            }
            assert_eq!(actual, Some(expected), "{label} must remain clickable after scrolling (failed={failed})");
        }
    }
}
