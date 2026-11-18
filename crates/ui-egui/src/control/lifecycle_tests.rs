use super::*;
use egui::Plugin as _;
use egui_kittest::Harness;

/// A request is answered after accepted frames only: a discarded layout pass is not a frame
/// the agent can see, so it must not advance the count.
#[test]
fn discarded_passes_are_not_counted_as_accepted_frames() {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let mut plugin = ControlPlugin { shared: shared.clone() };
    let ctx = egui::Context::default();
    let mut accepted = egui::FullOutput::default();
    plugin.output_hook(&ctx, &mut accepted);
    let mut discarded = egui::FullOutput::default();
    discarded.platform_output.request_discard_reasons.push(egui::RepaintCause::new_reason("control test discard"));
    plugin.output_hook(&ctx, &mut discarded);
    assert_eq!(shared.lock().unwrap().passes, 1, "only the presented pass counts");
}

/// A frame that requests discard on every pass and hits `max_passes` still presents its
/// last pass: that presented pass must count, or `Pending::Frames` waits for a frame that
/// never arrives. This runs the real multi-pass loop rather than a hand-built FullOutput.
#[test]
fn a_final_capped_pass_is_counted_as_presented() {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let ctx = egui::Context::default();
    ctx.add_plugin(ControlPlugin { shared: shared.clone() });
    ctx.options_mut(|o| o.max_passes = 2.try_into().expect("two is non-zero"));
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| ui.ctx().request_discard("control test must discard"));
    output.textures_delta.clear();
    assert_eq!(output.platform_output.num_completed_passes, 2, "the cap was reached");
    assert!(!output.platform_output.request_discard_reasons.is_empty(), "the last pass still requested a discard");
    assert_eq!(shared.lock().unwrap().passes, 1, "the presented final pass counts once");
}

/// `max_passes` above two still counts a frame that settles after earlier discarded passes:
/// egui clears the previous pass's discard reasons at the start of each pass, so the merged
/// output of a presented final pass is not mistaken for a discarded one.
#[test]
fn a_frame_that_settles_before_a_higher_pass_cap_is_counted_once() {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let ctx = egui::Context::default();
    ctx.add_plugin(ControlPlugin { shared: shared.clone() });
    ctx.options_mut(|o| o.max_passes = 3.try_into().expect("three is non-zero"));
    let mut pass = 0;
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        pass += 1;
        if pass < 3 {
            ui.ctx().request_discard("control test must discard the first two passes");
        }
    });
    output.textures_delta.clear();
    assert_eq!(output.platform_output.num_completed_passes, 3, "the third pass settled at the higher cap");
    assert!(output.platform_output.request_discard_reasons.is_empty(), "earlier discard reasons do not survive the settling pass");
    assert_eq!(shared.lock().unwrap().passes, 1, "the one presented frame counts once");
}

/// `ui.key` delivers held modifiers through `ModifiersChanged`, because egui does not derive
/// `InputState.modifiers` from a key event's own field. Shift+Tab and ⌘Enter handlers read it.
#[test]
fn key_commands_deliver_modifier_state() {
    let held = egui::Modifiers { command: true, ctrl: true, ..egui::Modifiers::NONE };
    let batches = key_batches(egui::Key::Enter, held);
    let key = |pressed| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: held };
    assert_eq!(
        batches,
        vec![vec![egui::Event::ModifiersChanged(held), key(true)], vec![key(false), egui::Event::ModifiersChanged(egui::Modifiers::NONE)]]
    );
    let ctx = egui::Context::default();
    let mut press_seen = None;
    ctx.run_ui(egui::RawInput { events: batches[0].clone(), ..Default::default() }, |ui| press_seen = Some(ui.input(|i| i.modifiers)))
        .textures_delta
        .clear();
    let mut release_seen = None;
    ctx.run_ui(egui::RawInput { events: batches[1].clone(), ..Default::default() }, |ui| release_seen = Some(ui.input(|i| i.modifiers)))
        .textures_delta
        .clear();
    assert_eq!(press_seen, Some(held), "the press sees the modifiers it was sent with");
    assert_eq!(release_seen, Some(egui::Modifiers::NONE), "the modifier release follows the key-up");
}

/// A batch admitted for one document/focus generation must not apply after that generation
/// changes; the request is reported as stale instead of replaying the input.
#[test]
fn a_stale_synthetic_batch_is_dropped_and_reported() {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let mut plugin = ControlPlugin { shared: shared.clone() };
    {
        let mut s = shared.lock().unwrap();
        s.epoch = 2;
        s.inject.push_back(RequestBatches {
            request: 41,
            epoch: 1,
            started: false,
            since: web_time::Instant::now(),
            release: Vec::new(),
            batches: VecDeque::from([vec![egui::Event::PointerMoved(egui::pos2(1.0, 1.0))]]),
        });
    }
    let mut raw = egui::RawInput::default();
    plugin.input_hook(&egui::Context::default(), &mut raw);
    assert!(raw.events.is_empty(), "stale input must not reach the new document");
    let s = shared.lock().unwrap();
    assert_eq!(s.dropped, vec![41]);
    assert!(s.inject.is_empty());
}

/// A stale request is dropped alone. A request admitted later for the current generation
/// behind it still runs: one agent's outdated input must not fail another's.
#[test]
fn a_stale_request_does_not_drop_a_later_current_one() {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let mut plugin = ControlPlugin { shared: shared.clone() };
    {
        let mut s = shared.lock().unwrap();
        s.epoch = 2;
        s.inject.push_back(RequestBatches {
            request: 41,
            epoch: 1,
            started: false,
            since: web_time::Instant::now(),
            release: Vec::new(),
            batches: VecDeque::from([vec![egui::Event::PointerMoved(egui::pos2(1.0, 1.0))]]),
        });
        s.inject.push_back(RequestBatches {
            request: 42,
            epoch: 2,
            started: false,
            since: web_time::Instant::now(),
            release: Vec::new(),
            batches: VecDeque::from([vec![egui::Event::PointerMoved(egui::pos2(2.0, 2.0))]]),
        });
    }
    let ctx = egui::Context::default();
    let mut first = egui::RawInput::default();
    plugin.input_hook(&ctx, &mut first);
    assert!(first.events.is_empty(), "the stale request is not replayed");
    let mut second = egui::RawInput::default();
    plugin.input_hook(&ctx, &mut second);
    assert_eq!(second.events, [egui::Event::PointerMoved(egui::pos2(2.0, 2.0))], "the current request still applies");
    let s = shared.lock().unwrap();
    assert_eq!(s.dropped, vec![41]);
    assert!(s.inject.is_empty());
}

/// Once a request has started, the focus/dialog change caused by its own first batch must not
/// drop its release: a key press without its release leaves `keys_down` stuck.
#[test]
fn a_started_request_finishes_its_batches_after_focus_moves() {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let mut plugin = ControlPlugin { shared: shared.clone() };
    {
        let mut s = shared.lock().unwrap();
        s.epoch = 7;
        s.inject.push_back(RequestBatches {
            request: 9,
            epoch: 7,
            started: false,
            since: web_time::Instant::now(),
            release: Vec::new(),
            batches: VecDeque::from([
                vec![egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
                vec![egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE }],
            ]),
        });
    }
    let ctx = egui::Context::default();
    let mut press = egui::RawInput::default();
    plugin.input_hook(&ctx, &mut press);
    assert!(matches!(press.events.as_slice(), [egui::Event::Key { pressed: true, .. }]));
    // The press moved focus: the next pass sees another input-target generation.
    shared.lock().unwrap().epoch = 8;
    let mut release = egui::RawInput::default();
    plugin.input_hook(&ctx, &mut release);
    assert!(matches!(release.events.as_slice(), [egui::Event::Key { pressed: false, .. }]), "the release still arrives");
    let s = shared.lock().unwrap();
    assert!(s.dropped.is_empty(), "a started request is not reported as stale");
    assert!(s.inject.is_empty());
}

/// A host for timeout tests: the control layer must not need a real app.
struct NoopHost;

impl Host for NoopHost {
    fn state(&self) -> Value {
        json!({})
    }
    fn command(&mut self, _id: &str) -> Reply {
        Err("no commands in this test".into())
    }
    fn panel(&mut self, _reset: bool, _params: &Value) -> Reply {
        Err("no side panels in this test".into())
    }
    fn commands(&self) -> Value {
        json!([])
    }
    fn set(&mut self, _key: &str, _value: &str) -> Reply {
        Err("no settings in this test".into())
    }
    fn open(&mut self, _path: &str) -> Reply {
        Err("no files in this test".into())
    }
    fn quit(&mut self, _ctx: &egui::Context) -> bool {
        false
    }
}

/// A host that starts unready (fonts not installed) and counts dispatched state reads.
/// `state` takes `&self`, so the count sits in a `Cell`.
struct ReadinessHost {
    ready: bool,
    states: std::cell::Cell<usize>,
    /// Methods the host was asked to run, in dispatch order.
    calls: std::cell::RefCell<Vec<&'static str>>,
}

impl ReadinessHost {
    fn new(ready: bool) -> Self {
        Self { ready, states: std::cell::Cell::new(0), calls: std::cell::RefCell::new(Vec::new()) }
    }
}

impl Host for ReadinessHost {
    fn state(&self) -> Value {
        self.states.set(self.states.get() + 1);
        self.calls.borrow_mut().push("ui.state");
        json!({ "ready": self.ready })
    }
    fn command(&mut self, _id: &str) -> Reply {
        Err("no commands in this test".into())
    }
    fn panel(&mut self, _reset: bool, _params: &Value) -> Reply {
        Err("no side panels in this test".into())
    }
    fn commands(&self) -> Value {
        self.calls.borrow_mut().push("ui.commands");
        json!([])
    }
    fn set(&mut self, _key: &str, _value: &str) -> Reply {
        Err("no settings in this test".into())
    }
    fn open(&mut self, _path: &str) -> Reply {
        Err("no files in this test".into())
    }
    fn quit(&mut self, _ctx: &egui::Context) -> bool {
        false
    }
    fn accepts_input(&self) -> bool {
        self.ready
    }
}

/// The admission timestamp of the one pending request, whichever kind it is.
fn pending_since(control: &Control) -> Option<web_time::Instant> {
    let first = control.pending.first()?;
    match first {
        Pending::Frames { since, .. } | Pending::Screenshot { since, .. } => Some(*since),
    }
}

#[test]
fn a_request_expired_in_the_channel_is_never_dispatched_or_deferred() {
    for ready in [false, true] {
        let ctx = egui::Context::default();
        let (mut control, client) = attach(&ctx);
        let mut host = ReadinessHost::new(ready);
        let (reply, waiting) = channel();
        let since = web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap();
        client.tx.send(ControlRequest { since, method: "ui.state".into(), params: json!({}), reply }).unwrap();
        control.tick(&ctx, &mut host);
        let error = waiting.try_recv().expect("expired channel request replies on the first tick").expect_err("expired request cannot dispatch");
        assert!(error.contains(if ready { "finished starting" } else { "still starting" }), "{error}");
        assert_eq!(host.states.get(), 0);
        assert!(control.deferred.is_empty() && control.pending.is_empty());
    }
}

/// A request held during startup keeps its arrival time. Once the app accepts input, one
/// older than the control deadline fails instead of being dispatched into a later UI.
#[test]
fn a_deferred_request_expires_instead_of_dispatching_after_the_deadline() {
    let ctx = egui::Context::default();
    let (mut control, client) = attach(&ctx);
    let mut host = ReadinessHost::new(false);
    let waiting = client.send("ui.state", json!({}));
    control.tick(&ctx, &mut host);
    assert_eq!(control.deferred.len(), 1, "unready host defers the request");
    assert!(waiting.try_recv().is_err(), "it is not answered while starting");
    let past = web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap();
    for deferred in &mut control.deferred {
        deferred.since = past;
    }
    // It expires even while the app is still not ready, so the client is not left waiting.
    control.tick(&ctx, &mut host);
    let error = waiting.try_recv().expect("expired deferred request receives a reply").expect_err("it must not dispatch");
    assert!(error.contains("still starting"), "{error}");
    assert!(control.deferred.is_empty());
    // And readiness arriving later cannot revive it.
    host.ready = true;
    control.tick(&ctx, &mut host);
    assert_eq!(host.states.get(), 0, "an expired request is never dispatched");
}

/// A request deferred only briefly still runs once fonts are ready.
#[test]
fn a_fresh_deferred_request_still_dispatches_when_the_app_becomes_ready() {
    let ctx = egui::Context::default();
    let (mut control, client) = attach(&ctx);
    let mut host = ReadinessHost::new(false);
    let waiting = client.send("ui.state", json!({}));
    control.tick(&ctx, &mut host);
    assert!(waiting.try_recv().is_err());
    host.ready = true;
    control.tick(&ctx, &mut host);
    let reply = waiting.try_recv().expect("fresh deferred request replies").expect("it dispatches");
    assert_eq!(reply["ready"], true);
    assert_eq!(host.states.get(), 1, "the fresh deferred request is dispatched once");
}

/// A request deferred during startup keeps its one arrival timestamp. Dispatching it into
/// synthetic input, a deferred frame reply or a screenshot reservation must not start a fresh
/// 5 s in that phase: every later phase measures the same deadline from arrival.
#[test]
fn a_startup_wait_counts_against_one_admission_deadline_in_every_phase() {
    let params = [("ui.type", json!({ "text": "x" })), ("ui.resize", json!({ "width": 800, "height": 600 })), ("ui.screenshot", json!({}))];
    for (method, params) in params {
        // `ui.resize` needs a native window; the other phases are checked on every target.
        if method == "ui.resize" && cfg!(target_arch = "wasm32") {
            continue;
        }
        let ctx = egui::Context::default();
        // A real window size, so an admitted screenshot has capturable geometry.
        ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0))), ..Default::default() },
            |_| {},
        )
        .drop_without_applying_deltas();
        let (mut control, client) = attach(&ctx);
        let mut host = ReadinessHost::new(false);
        let waiting = client.send(method, params);
        control.tick(&ctx, &mut host);
        assert_eq!(control.deferred.len(), 1, "{method}: deferred while starting");
        // Three of the five seconds were spent starting: the request is still inside its
        // deadline, but only just.
        let arrived = web_time::Instant::now().checked_sub(std::time::Duration::from_secs(3)).unwrap();
        for deferred in &mut control.deferred {
            deferred.since = arrived;
        }
        host.ready = true;
        control.tick(&ctx, &mut host);
        assert!(control.deferred.is_empty(), "{method}: dispatched once the app accepts input");
        assert!(waiting.try_recv().is_err(), "{method}: a preserved deadline does not answer early");
        assert_eq!(pending_since(&control), Some(arrived), "{method}: the pending reply keeps the arrival deadline");
        if method == "ui.type" {
            let shared = control.shared.lock().unwrap();
            assert_eq!(shared.inject.front().map(|request| request.since), Some(arrived), "{method}: queued input keeps the arrival deadline");
        }
        // Move that same shared timestamp past its own deadline (five seconds after arrival).
        // Expiry measures the one admission instant, so nothing waits for a fresh five.
        let past = arrived.checked_sub(timeout()).expect("a representable instant");
        for pending in &mut control.pending {
            match pending {
                Pending::Frames { since, .. } | Pending::Screenshot { since, .. } => *since = past,
            }
        }
        if let Ok(mut shared) = control.shared.lock() {
            for request in &mut shared.inject {
                request.since = past;
            }
        }
        control.finish_pending(&ctx, &mut host);
        let error = waiting.try_recv().expect("the expired request replies").expect_err("it must not succeed");
        assert!(error.contains("not being drawn"), "{method}: {error}");
        assert!(control.pending.is_empty(), "{method}: the expired request is not retried");
        assert_eq!(control.reserved_bytes(), 0, "{method}: any reservation is released");
    }
}

/// One client's requests keep their arrival order: a request deferred during startup runs
/// before a newer request that was still waiting in the channel when the app became ready.
#[test]
fn a_deferred_request_runs_before_a_newer_queued_one() {
    let ctx = egui::Context::default();
    let (mut control, client) = attach(&ctx);
    let mut host = ReadinessHost::new(false);
    let older = client.send("ui.state", json!({}));
    control.tick(&ctx, &mut host);
    assert_eq!(control.deferred.len(), 1, "the older request is deferred while starting");
    let newer = client.send("ui.commands", json!({}));
    host.ready = true;
    control.tick(&ctx, &mut host);
    assert_eq!(*host.calls.borrow(), ["ui.state", "ui.commands"], "the older deferred request is dispatched first");
    assert_eq!(older.try_recv().expect("the older request replies").expect("it dispatches")["ready"], true);
    assert_eq!(newer.try_recv().expect("the newer request replies").expect("it dispatches"), json!([]));
}

/// A request whose startup wait used up the deadline is refused once the app is ready, and the
/// message says the app finished starting instead of claiming it is still starting.
#[test]
fn a_request_that_outlasted_startup_reports_that_the_app_finished_starting() {
    let ctx = egui::Context::default();
    let (mut control, client) = attach(&ctx);
    let mut host = ReadinessHost::new(false);
    let waiting = client.send("ui.state", json!({}));
    control.tick(&ctx, &mut host);
    assert_eq!(control.deferred.len(), 1);
    let past = web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap();
    for deferred in &mut control.deferred {
        deferred.since = past;
    }
    host.ready = true;
    control.tick(&ctx, &mut host);
    let error = waiting.try_recv().expect("the expired request replies").expect_err("it is never dispatched");
    assert!(error.contains("finished starting"), "{error}");
    assert!(!error.contains("was still starting"), "the app is ready now: {error}");
    assert_eq!(host.states.get(), 0, "an expired request is never dispatched");
    assert!(control.deferred.is_empty() && control.pending.is_empty());
}

/// Keep key/modifier cleanup, but a canceled pointer must never produce a release event.
#[test]
fn a_timed_out_started_request_cancels_pointer_and_releases_keys() {
    let ctx = egui::Context::default();
    let (mut control, _client) = attach(&ctx);
    let held = egui::Modifiers { command: true, ctrl: true, ..egui::Modifiers::NONE };
    let pos = egui::pos2(24.0, 32.0);
    let key = |pressed| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: held };
    let ids = control
        .inject(
            vec![
                vec![
                    egui::Event::ModifiersChanged(held),
                    key(true),
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: held },
                ],
                vec![egui::Event::PointerMoved(egui::pos2(40.0, 48.0))],
                vec![key(false), egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: held }],
            ],
            web_time::Instant::now(),
        )
        .unwrap();
    let (reply, rx) = channel();
    control.pending.push(Pending::Frames { until_pass: u64::MAX, batches: ids, since: web_time::Instant::now(), reply, value: json!({}) });
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    assert!(ctx.input(|i| i.keys_down.contains(&egui::Key::Enter) && i.pointer.primary_down()));
    assert_eq!(ctx.input(|i| i.modifiers), held);
    age_input(&mut control);
    let next = egui::pos2(80.0, 90.0);
    control.inject(vec![vec![egui::Event::PointerMoved(next)]], web_time::Instant::now()).unwrap();
    control.finish_pending(&ctx, &mut NoopHost);
    assert!(rx.try_recv().unwrap().unwrap_err().contains("not being drawn"));
    assert!(control.pending.is_empty());
    ctx.run_ui(egui::RawInput::default(), |ui| {
        assert!(!ui.input(|i| i.keys_down.contains(&egui::Key::Enter)));
        assert_eq!(ui.input(|i| i.modifiers), egui::Modifiers::NONE);
        assert_eq!(ui.input(|i| i.pointer.hover_pos()), Some(next), "another live request survives");
        assert!(!ui.input(|i| i.pointer.any_down() || i.pointer.any_released()));
        assert!(ui.ctx().dragged_id().is_none() && ui.ctx().drag_stopped_id().is_none());
    })
    .drop_without_applying_deltas();
    assert!(control.shared.lock().unwrap().inject.is_empty());
    assert!(rx.try_recv().is_err(), "only one timeout reply");
}

#[test]
fn a_deadline_crossing_inside_an_admitted_pass_cancels_before_the_next_interaction() {
    let ctx = egui::Context::default();
    let (mut control, _client) = attach(&ctx);
    let pos = egui::pos2(24.0, 32.0);
    let ids = control
        .inject(
            vec![
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
                ],
                vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }],
            ],
            web_time::Instant::now(),
        )
        .unwrap();
    let (reply, rx) = channel();
    control.pending.push(Pending::Frames { until_pass: u64::MAX, batches: ids, since: web_time::Instant::now(), reply, value: json!({}) });
    ctx.run_ui(egui::RawInput::default(), |ui| {
        assert!(ui.input(|i| i.pointer.primary_down()), "the press was admitted before expiry");
        age_input(&mut control);
        control.finish_pending(ui.ctx(), &mut NoopHost);
        assert!(rx.try_recv().is_err(), "do not cancel an already-calculated interaction mid-pass");
    })
    .drop_without_applying_deltas();
    ctx.run_ui(egui::RawInput::default(), |ui| {
        control.finish_pending(ui.ctx(), &mut NoopHost);
        assert!(!ui.input(|i| i.pointer.any_down() || i.pointer.any_released()));
        assert!(ui.ctx().drag_stopped_id().is_none());
    })
    .drop_without_applying_deltas();
    assert!(rx.try_recv().unwrap().unwrap_err().contains("not being drawn"));
    assert!(control.shared.lock().unwrap().inject.is_empty());
}

/// Deterministic elapsed-time input: age the same admission in both halves, without sleeping
/// or fabricating any egui pointer/widget state.
fn age_input(control: &mut Control) {
    let past = web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap();
    for request in &mut control.shared.lock().unwrap().inject {
        request.since = past;
    }
    for pending in &mut control.pending {
        if let Pending::Frames { since, .. } = pending {
            *since = past;
        }
    }
}

/// The public gestures this test drives through real UI passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gesture {
    AtomicClick,
    HeldClick,
    Marquee,
}

fn gesture_request(gesture: Gesture, from: egui::Pos2, to: egui::Pos2) -> (&'static str, Value) {
    match gesture {
        Gesture::AtomicClick => ("ui.click", json!({"x": from.x, "y": from.y})),
        Gesture::HeldClick => ("ui.drag", json!({"from": [from.x, from.y], "to": [from.x, from.y], "steps": 6})),
        Gesture::Marquee => ("ui.drag", json!({"from": [from.x, from.y], "to": [to.x, to.y], "steps": 6})),
    }
}

fn marquee_harness() -> (Harness<'static, crate::PdfCraftApp>, ControlClient) {
    let client_slot: Arc<Mutex<Option<ControlClient>>> = Arc::new(Mutex::new(None));
    let slot = client_slot.clone();
    let h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).with_step_dt(0.01).build_eframe(move |cc| {
        let mut app = crate::PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("synthetic-form.pdf", None, include_bytes!("../../tests/data/form.pdf").to_vec()).unwrap();
        app.set_option("quick", "marquee-zoom").unwrap();
        *slot.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        app
    });
    let client = client_slot.lock().unwrap().take().unwrap();
    (h, client)
}

fn marquee_step(h: &mut Harness<'static, crate::PdfCraftApp>) {
    let ctx = h.ctx.clone();
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().prepare_raw_input(&ctx, &mut raw);
    *h.input_mut() = raw;
    h.step();
}

/// The actual laid-out page rect and a short point gesture inside it.
fn gesture_target(h: &Harness<'static, crate::PdfCraftApp>) -> (egui::Pos2, egui::Pos2) {
    let page = h.state().views[0].page_screen_rect(0).expect("the actual PDF page was laid out").intersect(h.state().views[0].viewport_rect());
    assert!(page.width() > 100.0 && page.height() > 100.0, "a visible hit target: {page:?}");
    let from = page.center();
    let to = from + egui::vec2(40.0, 50.0);
    assert!(page.contains(to));
    (from, to)
}

/// Step until the public request has really reached egui and can still be interrupted.
fn await_gesture_admission(h: &mut Harness<'static, crate::PdfCraftApp>, gesture: Gesture) -> bool {
    let mut admitted = false;
    for _ in 0..8 {
        marquee_step(h);
        admitted = if gesture == Gesture::AtomicClick {
            let shared = h.state().control.as_ref().unwrap().shared.lock().unwrap();
            shared.inject.front().is_some_and(|request| request.started && request.batches.len() == 1)
        } else {
            h.ctx.input(|i| i.pointer.primary_down()) && (gesture != Gesture::Marquee || h.state().views[0].marquee.is_some())
        };
        if admitted {
            break;
        }
    }
    admitted
}

/// The request is queued exactly once, with the expected terminal batch not yet applied.
fn assert_interruptible_admission(h: &Harness<'static, crate::PdfCraftApp>, gesture: Gesture, from: egui::Pos2) {
    {
        let shared = h.state().control.as_ref().unwrap().shared.lock().unwrap();
        assert_eq!(shared.inject.len(), 1, "only the actual public request is queued");
        let pending = shared.inject.front().unwrap();
        assert!(pending.started);
        let terminal = pending.batches.back().expect("the terminal batch is still pending");
        if gesture == Gesture::AtomicClick {
            assert_eq!(pending.batches.len(), 1);
            assert_eq!(terminal.len(), 2);
            assert!(
                matches!(terminal[0], egui::Event::PointerButton { pressed: true, button: egui::PointerButton::Primary, pos, .. } if pos == from)
            );
            assert!(
                matches!(terminal[1], egui::Event::PointerButton { pressed: false, button: egui::PointerButton::Primary, pos, .. } if pos == from)
            );
            assert!(!h.ctx.input(|i| i.pointer.primary_down()), "atomic click has only hovered so far");
        } else {
            assert!(h.ctx.input(|i| i.pointer.primary_down()), "a real pass consumed the press");
            assert_eq!(terminal.len(), 1);
            assert!(matches!(terminal[0], egui::Event::PointerButton { pressed: false, button: egui::PointerButton::Primary, .. }));
        }
    }
    assert!(!h.ctx.input(|i| i.pointer.any_released()), "the terminal release has not run");
    assert!(h.ctx.drag_stopped_id().is_none());
    if gesture == Gesture::Marquee {
        assert!(h.ctx.dragged_id().is_some() && h.state().views[0].marquee.is_some(), "the app has a real drag preview");
    } else {
        assert!(h.state().views[0].marquee.is_none() && h.state().views[0].marquee_done.is_none());
    }
}

/// Expire the admission both through hidden logic and through the next input hook, then
/// assert that cancellation left no gesture, zoom or edit behind.
fn assert_timed_out_gesture(h: &mut Harness<'static, crate::PdfCraftApp>, reply: &Receiver<Reply>, hidden_logic: bool, zoom: f32, bytes: &[u8]) {
    age_input(h.state_mut().control.as_mut().unwrap());
    if hidden_logic {
        let ctx = h.ctx.clone();
        let app = h.state_mut();
        let mut control = app.control.take().unwrap();
        control.tick(&ctx, app);
        app.control = Some(control);
        assert!(reply.try_recv().unwrap().unwrap_err().contains("not being drawn"));
    }
    marquee_step(h);
    if !hidden_logic {
        assert!(reply.try_recv().unwrap().unwrap_err().contains("not being drawn"));
    }
    for _ in 0..2 {
        marquee_step(h);
    }
    assert!(!h.ctx.input(|i| i.pointer.any_down() || i.pointer.any_released()));
    assert!(h.ctx.dragged_id().is_none() && h.ctx.drag_stopped_id().is_none());
    assert!(h.state().views[0].marquee.is_none() && h.state().views[0].marquee_done.is_none());
    assert_eq!(h.state().views[0].zoom, zoom, "canceling must not turn a drag into a zoom click");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes.as_slice(), bytes, "no abandoned edit");
    assert!(h.state().control.as_ref().unwrap().shared.lock().unwrap().inject.is_empty());
    assert!(h.state().control.as_ref().unwrap().pending.is_empty());
    assert!(reply.try_recv().is_err(), "the cancellation replies once");
}

/// Complete one public gesture and check its real event trace and one-step zoom.
fn complete_positive_gesture(
    h: &mut Harness<'static, crate::PdfCraftApp>,
    client: &ControlClient,
    gesture: Gesture,
    positive: Gesture,
    bytes: &[u8],
) {
    let (from, to) = gesture_target(h);
    let zoom = h.state().views[0].zoom;
    let (method, params) = gesture_request(positive, from, to);
    let reply = client.send(method, params);
    let mut value = None;
    let mut buttons = Vec::new();
    let mut positions = Vec::new();
    for _ in 0..20 {
        marquee_step(h);
        h.ctx.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed, pos, .. } => {
                        buttons.push((*pressed, i.time));
                        positions.push(*pos);
                    }
                    egui::Event::PointerMoved(pos) => positions.push(*pos),
                    _ => {}
                }
            }
        });
        if let Ok(result) = reply.try_recv() {
            value = Some(result.unwrap());
            break;
        }
    }
    assert!(value.is_some(), "a fresh completed request succeeds: {positive:?}, after {gesture:?}");
    assert_eq!(buttons.iter().map(|(pressed, _)| *pressed).collect::<Vec<_>>(), [true, false]);
    assert!(!h.ctx.input(|i| i.pointer.any_down()));
    assert!(h.ctx.dragged_id().is_none());
    if positive == Gesture::Marquee {
        assert!(h.state().views[0].zoom > zoom, "the normal marquee rectangle zooms");
    } else {
        let options = h.ctx.options(|o| o.input_options);
        assert!(buttons[1].1 - buttons[0].1 < options.max_click_duration, "real press/release remain a short click");
        assert!(positions.iter().all(|pos| pos.distance(from) < options.max_click_dist), "real input remains stationary");
        assert!((h.state().views[0].zoom - zoom * 1.5).abs() < 0.001, "a normal click zooms one step");
    }
    assert!(h.state().views[0].marquee.is_none() && h.state().views[0].marquee_done.is_none());
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes.as_slice(), bytes, "completed zooms do not edit the document");
    assert!(reply.try_recv().is_err(), "successful request replies once");
}

/// Real page hit-testing, real control batches and the production marquee completion path.
/// Exercise both hidden-logic expiry and expiry first observed by the next input hook.
#[test]
fn a_timed_out_marquee_never_zooms_and_completed_clicks_and_drags_still_work() {
    for hidden_logic in [false, true] {
        for gesture in [Gesture::AtomicClick, Gesture::HeldClick, Gesture::Marquee] {
            let (mut h, client) = marquee_harness();
            for _ in 0..5 {
                marquee_step(&mut h);
            }
            let (from, to) = gesture_target(&h);
            let zoom = h.state().views[0].zoom;
            let bytes = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
            let (method, params) = gesture_request(gesture, from, to);
            let reply = client.send(method, params);
            assert!(await_gesture_admission(&mut h, gesture), "a real interruptible admission: {gesture:?}, hidden={hidden_logic}");
            assert_interruptible_admission(&h, gesture, from);
            assert!(reply.try_recv().is_err(), "the terminal batch has not run");
            assert_eq!(h.state().views[0].zoom, zoom);
            assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes, bytes);
            assert_timed_out_gesture(&mut h, &reply, hidden_logic, zoom, &bytes);

            // Complete the same public gesture, then an atomic click after held input.
            // Real event traces prove the stationary drag is a valid short point click.
            let positives = [gesture, Gesture::AtomicClick];
            let count = if gesture == Gesture::AtomicClick { 1 } else { 2 };
            for positive in positives.into_iter().take(count) {
                complete_positive_gesture(&mut h, &client, gesture, positive, &bytes);
            }
        }
    }
}

/// A screenshot that times out cancels its slot and gives its reservation back, with a
/// defined error instead of a silent success.
#[test]
fn a_timed_out_screenshot_releases_its_slot_and_budget() {
    struct FakeHost;
    impl Host for FakeHost {
        fn state(&self) -> Value {
            json!({})
        }
        fn command(&mut self, _id: &str) -> Reply {
            Err("no commands in this test".into())
        }
        fn panel(&mut self, _reset: bool, _params: &Value) -> Reply {
            Err("no side panels in this test".into())
        }
        fn commands(&self) -> Value {
            json!([])
        }
        fn set(&mut self, _key: &str, _value: &str) -> Reply {
            Err("no settings in this test".into())
        }
        fn open(&mut self, _path: &str) -> Reply {
            Err("no files in this test".into())
        }
        fn quit(&mut self, _ctx: &egui::Context) -> bool {
            false
        }
    }
    let ctx = egui::Context::default();
    let (mut control, _client) = attach(&ctx);
    let (slot, callback) = craft_ui::capture::CaptureSlot::new(&ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
    let (reply, rx) = channel();
    control.pending.push(Pending::Screenshot {
        slot,
        callback: Some(callback),
        region: None,
        reply,
        since: web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap_or_else(web_time::Instant::now),
        geometry: Geometry { size: egui::vec2(800.0, 600.0), pixels_per_point: 1.0, epoch: 0 },
        reserved: 800 * 600 * 4,
        attempts: 0,
    });
    control.finish_pending(&ctx, &mut FakeHost);
    let error = rx.try_recv().expect("the request is answered").expect_err("a timeout is an error");
    assert!(error.contains("not being drawn"), "{error}");
    assert_eq!(control.reserved_bytes(), 0, "the reservation is released");
    assert!(control.pending.is_empty(), "the cancelled capture is not retried");
}

/// A real successful callback must not bypass expiry, including when resize would otherwise
/// retry it. A non-expired completion still produces a real encoded PNG reply.
#[test]
fn completed_screenshots_obey_the_admission_deadline_before_success_or_retry() {
    for (expired, resized) in [(false, false), (true, false), (true, true)] {
        let ctx = egui::Context::default();
        ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 32.0))), ..Default::default() },
            |_| {},
        )
        .drop_without_applying_deltas();
        let (mut control, _client) = attach(&ctx);
        let geometry = Geometry::current(&ctx, 0);
        let (slot, callback) = craft_ui::capture::CaptureSlot::new(&ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
        let image = Arc::new(egui::ColorImage::filled([64, 32], egui::Color32::RED));
        let weak_image = Arc::downgrade(&image);
        callback.complete(image);
        assert!(weak_image.upgrade().is_some(), "the completed slot really retains pixels");
        let (reply, rx) = channel();
        control.pending.push(Pending::Screenshot {
            slot,
            callback: None,
            region: None,
            reply,
            since: if expired { web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap() } else { web_time::Instant::now() },
            geometry,
            reserved: geometry.image_bytes().unwrap(),
            attempts: 0,
        });
        if resized {
            ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(80.0, 40.0))), ..Default::default() },
                |_| {},
            )
            .drop_without_applying_deltas();
            assert_ne!(Geometry::current(&ctx, 0).size, geometry.size);
        }
        control.finish_pending(&ctx, &mut NoopHost);
        let result = rx.try_recv().expect("one settled reply");
        if expired {
            assert!(result.unwrap_err().contains("not being drawn"), "expiry takes priority over success or resize");
        } else {
            let image = result.expect("the live completed capture still succeeds");
            assert_eq!(image["width"], 64);
            assert_eq!(image["height"], 32);
            use base64::Engine as _;
            let png = base64::engine::general_purpose::STANDARD.decode(image["png_base64"].as_str().expect("actual PNG reply")).unwrap();
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
        assert!(control.pending.is_empty(), "an expired capture is never retried");
        assert_eq!(control.reserved_bytes(), 0);
        assert!(weak_image.upgrade().is_none(), "the slot releases its pixels");
        assert!(rx.try_recv().is_err(), "exactly one reply");
    }
}

#[test]
fn expired_unissued_screenshots_are_not_issued_and_late_callbacks_are_harmless() {
    let ctx = egui::Context::default();
    let (mut control, _client) = attach(&ctx);
    let (slot, callback) = craft_ui::capture::CaptureSlot::new(&ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
    let late_callback = callback.clone();
    let geometry = Geometry { size: egui::vec2(64.0, 32.0), pixels_per_point: 1.0, epoch: 0 };
    let (reply, rx) = channel();
    control.pending.push(Pending::Screenshot {
        slot,
        callback: Some(callback),
        region: None,
        reply,
        since: web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap(),
        geometry,
        reserved: 64 * 32 * 4,
        attempts: 0,
    });
    let output = ctx.run_ui(egui::RawInput::default(), |ui| control.finish_frame(ui.ctx(), &mut NoopHost));
    assert!(
        output
            .viewport_output
            .values()
            .all(|viewport| !viewport.commands.iter().any(|command| matches!(command, egui::ViewportCommand::Screenshot(_))))
    );
    output.drop_without_applying_deltas();
    assert!(rx.try_recv().unwrap().unwrap_err().contains("not being drawn"));
    assert!(control.pending.is_empty());
    assert_eq!(control.reserved_bytes(), 0);
    let image = Arc::new(egui::ColorImage::filled([64, 32], egui::Color32::RED));
    let weak_image = Arc::downgrade(&image);
    late_callback.complete(image);
    assert!(weak_image.upgrade().is_none(), "a late completion cannot resurrect a canceled request");
    control.finish_pending(&ctx, &mut NoopHost);
    assert!(rx.try_recv().is_err(), "no second reply");
}

/// One queued batch is consumed by exactly one UI pass.
#[test]
fn one_batch_is_consumed_by_one_pass() {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let mut plugin = ControlPlugin { shared: shared.clone() };
    {
        let mut s = shared.lock().unwrap();
        s.epoch = 3;
        s.inject.push_back(RequestBatches {
            request: 1,
            epoch: 3,
            started: false,
            since: web_time::Instant::now(),
            release: Vec::new(),
            batches: VecDeque::from([vec![egui::Event::PointerMoved(egui::pos2(2.0, 3.0))]]),
        });
        s.inject.push_back(RequestBatches {
            request: 2,
            epoch: 3,
            started: false,
            since: web_time::Instant::now(),
            release: Vec::new(),
            batches: VecDeque::from([vec![egui::Event::PointerMoved(egui::pos2(4.0, 5.0))]]),
        });
    }
    let mut first = egui::RawInput::default();
    plugin.input_hook(&egui::Context::default(), &mut first);
    assert_eq!(first.events.len(), 1);
    let mut second = egui::RawInput::default();
    plugin.input_hook(&egui::Context::default(), &mut second);
    assert_eq!(second.events.len(), 1);
    assert_eq!(second.events[0], egui::Event::PointerMoved(egui::pos2(4.0, 5.0)));
    let mut third = egui::RawInput::default();
    plugin.input_hook(&egui::Context::default(), &mut third);
    assert!(third.events.is_empty(), "a consumed batch is never repeated");
}
