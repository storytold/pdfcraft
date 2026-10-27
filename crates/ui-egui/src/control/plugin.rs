use super::requests::Handled;
use super::*;

/// Track only presses that reached egui and have not yet been released. A pointer release is
/// retained as an ownership marker, never as a timeout event.
fn note_pressed(release: &mut Vec<egui::Event>, events: &[egui::Event]) {
    for event in events {
        match event {
            egui::Event::Key { key, pressed: true, .. }
                if !release.iter().any(|event| matches!(event, egui::Event::Key { key: seen, pressed: false, .. } if seen == key)) =>
            {
                release.push(egui::Event::Key { key: *key, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE });
            }
            egui::Event::PointerButton { pos, button, pressed: true, .. }
                if !release
                    .iter()
                    .any(|event| matches!(event, egui::Event::PointerButton { button: seen, pressed: false, .. } if seen == button)) =>
            {
                release.push(egui::Event::PointerButton { pos: *pos, button: *button, pressed: false, modifiers: egui::Modifiers::NONE });
            }
            egui::Event::ModifiersChanged(modifiers)
                if *modifiers != egui::Modifiers::NONE && !release.contains(&egui::Event::ModifiersChanged(egui::Modifiers::NONE)) =>
            {
                release.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            }
            egui::Event::Key { key, pressed: false, .. } => {
                release.retain(|event| !matches!(event, egui::Event::Key { key: seen, .. } if seen == key));
            }
            egui::Event::PointerButton { button, pressed: false, .. } => {
                release.retain(|event| !matches!(event, egui::Event::PointerButton { button: seen, .. } if seen == button));
            }
            egui::Event::ModifiersChanged(modifiers) if *modifiers == egui::Modifiers::NONE => {
                release.retain(|event| !matches!(event, egui::Event::ModifiersChanged(_)));
            }
            _ => {}
        }
    }
}

/// Cancel expired requests before egui computes interaction. Hidden logic calls this too, so
/// replies do not wait for a visible frame. Never complete an abandoned pointer gesture.
pub(super) fn expire_input(shared: &mut Shared, now: web_time::Instant) {
    shared.inject.retain_mut(|request| {
        if now.duration_since(request.since) <= timeout() {
            return true;
        }
        shared.expired.push(request.request);
        for event in request.release.drain(..) {
            if matches!(event, egui::Event::PointerButton { .. }) {
                shared.cancel_pointer = true;
                shared.cancel_gestures = true;
            } else {
                shared.cleanup.push(event);
            }
        }
        false
    });
}

/// Synthetic input is bounded like screenshots: a local authenticated client must not be able to
/// queue unbounded batches (one frame each) and hold the control channel's memory for ever.
const MAX_QUEUED_BATCHES: usize = 512;

/// The egui plugin half of the control channel.
pub struct ControlPlugin {
    pub(super) shared: Arc<Mutex<Shared>>,
}

impl egui::Plugin for ControlPlugin {
    fn debug_name(&self) -> &'static str {
        "pdfcraft-control"
    }

    /// Inject admitted control batches into the current pass, dropping stale requests, applying cancellations
    /// and scheduling repaints.
    fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        let Ok(mut s) = self.shared.lock() else { return };
        expire_input(&mut s, web_time::Instant::now());
        if std::mem::take(&mut s.cancel_pointer) {
            // input_hook precedes Context::begin_pass. stop_dragging sets the OLD snapshot's
            // drag_stopped; begin_pass replaces that snapshot without a release/click/drop.
            ctx.input_mut(|i| i.pointer = Default::default());
            ctx.stop_dragging();
            egui::DragAndDrop::clear_payload(ctx);
        }
        input.events.append(&mut s.cleanup);
        s.in_pass = true;
        let epoch = s.epoch;
        let Some(front) = s.inject.front_mut() else { return };
        if !front.started {
            if front.epoch != epoch {
                // The document, dialog or focus changed after this request was admitted, before
                // any of its input landed. Drop only this request; later requests are unrelated.
                let request = front.request;
                s.inject.pop_front();
                s.dropped.push(request);
                if !s.inject.is_empty() {
                    ctx.request_repaint();
                }
                return;
            }
            front.started = true;
        }
        // A non-expired request's own earlier batch may have changed focus or opened a dialog.
        // Let it finish normally; expiry above cancels held pointer state instead of completing
        // an abandoned click/drop.
        let events = front.batches.pop_front();
        if let Some(events) = &events {
            // Remember the inverse of what this request presses as it is applied, so a timeout
            // after this batch can still release exactly those inputs.
            note_pressed(&mut front.release, events);
        }
        let done = front.batches.is_empty();
        if done {
            s.inject.pop_front();
        }
        if let Some(events) = events {
            input.events.extend(events);
        }
        if !s.inject.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Record presented passes and the accesskit tree and focus after a pass, so control replies observe only
    /// drawn frames.
    fn output_hook(&mut self, ctx: &egui::Context, output: &mut egui::FullOutput) {
        let Ok(mut s) = self.shared.lock() else { return };
        s.in_pass = false;
        // A discarded pass is layout work, not a frame the agent can observe. Only count passes
        // whose output is actually presented, so a request cannot be answered early. The last
        // pass of a frame is presented even when its own discard request was refused because
        // `max_passes` is reached; egui still reports that request in the output.
        let max_passes = ctx.options(|o| o.max_passes.get());
        let final_pass = output.platform_output.num_completed_passes >= max_passes;
        if output.platform_output.request_discard_reasons.is_empty() || final_pass {
            s.passes += 1;
        }
        let Some(update) = &output.platform_output.accesskit_update else { return };
        s.nodes = update.nodes.iter().cloned().collect();
        s.root = update.tree.as_ref().map(|t| t.root);
        s.focus = Some(update.focus);
    }
}

impl Control {
    /// Queue one request's event batches (one per UI pass) and return its request id.
    ///
    /// The request is stamped once, at admission: all of its batches share the input-target
    /// generation and the caller's single arrival deadline (`since`), so a deferred startup wait
    /// is not restarted here. A poisoned mutex or a full queue is an error, never a silently empty
    /// queue that would let the caller answer `Ok` for input that never happened.
    pub(super) fn inject(&self, batches: Vec<Vec<egui::Event>>, since: web_time::Instant) -> Result<Vec<u64>, String> {
        let mut s = self.shared.lock().map_err(|_| "control state poisoned".to_string())?;
        let queued: usize = s.inject.iter().map(|request| request.batches.len()).sum();
        if queued.saturating_add(batches.len()) > MAX_QUEUED_BATCHES {
            return Err(format!("too many synthetic input batches are already queued (limit {MAX_QUEUED_BATCHES})"));
        }
        s.next_batch += 1;
        let id = s.next_batch;
        let epoch = s.epoch;
        s.inject.push_back(RequestBatches { request: id, epoch, started: false, since, release: Vec::new(), batches: batches.into() });
        Ok(vec![id])
    }

    /// Queue synthetic input batches and reply only after the requested number of frames has been presented.
    pub(super) fn after_input(&self, batches: Vec<Vec<egui::Event>>, value: Value, since: web_time::Instant) -> Result<Handled, String> {
        let frames = batches.len() as u32;
        let batches = self.inject(batches, since)?;
        Ok(Handled::AfterInput { batches, frames, value })
    }
}
