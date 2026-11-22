//! The presentation shell's touch-only way out of full screen.

use std::time::Duration;

use egui::{Align2, Context, Event, Id, Pos2, Rect, TouchDeviceId, TouchId, TouchPhase, vec2};

use crate::{theme::Tokens, widgets};

const EDGE_DEPTH: f32 = 24.0;
const SWIPE_DISTANCE: f32 = 56.0;
const REVEAL_FOR: Duration = Duration::from_secs(5);

fn edge_id() -> Id {
    Id::unique("full-screen-touch-edge")
}

struct EdgeTouch {
    device: TouchDeviceId,
    id: TouchId,
    start: Pos2,
}

#[derive(Default)]
pub(crate) struct FullScreenTouch {
    touch: Option<EdgeTouch>,
    shown_until: Option<f64>,
    button: Option<Id>,
}

impl FullScreenTouch {
    /// Mouse and synthesized pointer drags are left to the viewer. Only a native touch
    /// starting at the top edge can reveal the button.
    fn track(&mut self, events: &[Event], screen: Rect, now: f64, drag_stopped: bool) {
        for event in events {
            let Event::Touch { device_id, id, phase, pos, .. } = event else { continue };
            match phase {
                TouchPhase::Start => {
                    let at_edge = screen.contains(*pos) && pos.y <= screen.top() + EDGE_DEPTH;
                    self.touch = (self.touch.is_none() && at_edge).then_some(EdgeTouch { device: *device_id, id: *id, start: *pos });
                }
                TouchPhase::End | TouchPhase::Cancel => {
                    if self.touch.as_ref().is_some_and(|touch| touch.device == *device_id && touch.id == *id)
                        && let Some(touch) = self.touch.take()
                    {
                        let delta = *pos - touch.start;
                        if drag_stopped && *phase == TouchPhase::End && delta.y >= SWIPE_DISTANCE && delta.y > delta.x.abs() {
                            self.shown_until = Some(now + REVEAL_FOR.as_secs_f64());
                        }
                    }
                }
                TouchPhase::Move => {}
            }
        }
    }

    /// Enter and Space activate a focused button before they can turn the page.
    pub(super) fn has_focus(&self, ctx: &Context) -> bool {
        self.button.is_some_and(|id| ctx.memory(|memory| memory.has_focus(id)))
    }

    /// Register after the page so egui gives this strip its drags. Sharing the page's
    /// layer lets a tap still reach the reader's regular click handling.
    pub(super) fn register_edge(ui: &egui::Ui) {
        let screen = ui.ctx().content_rect();
        let size = vec2(screen.width(), screen.height().min(EDGE_DEPTH));
        ui.interact(Rect::from_min_size(screen.min, size), edge_id(), egui::Sense::DRAG);
    }

    /// Draw just the exit button, centred at the top so it stays clear of the page controls.
    pub(super) fn show(&mut self, ctx: &Context) -> bool {
        let screen = ctx.content_rect();
        let drag_stopped = ctx.read_response(edge_id()).is_some_and(|edge| edge.drag_stopped());
        let now = ctx.input(|input| {
            if input.multi_touch().is_some() {
                self.touch = None;
            } else {
                self.track(&input.events, screen, input.time, drag_stopped);
            }
            input.time
        });
        // Keep the exit available to someone who is using its keyboard or accessibility focus.
        let focused = self.has_focus(ctx);
        let Some(until) = self.shown_until.filter(|until| focused || now < *until) else {
            self.hide(ctx);
            return false;
        };
        if !focused && let Ok(remaining) = Duration::try_from_secs_f64(until - now) {
            ctx.request_repaint_after(remaining);
        }
        let t = Tokens::get(ctx);
        let button = egui::Area::new(Id::unique("full-screen-touch-exit"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, 10.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(t.card)
                    .stroke(egui::Stroke::new(1.0, t.border))
                    .corner_radius(t.radius)
                    .inner_margin(egui::Margin::same(4))
                    .show(ui, |ui| {
                        let button = widgets::ghost_button_sized(ui, "minimize", tl!("Exit full screen"), 36.0);
                        if button.has_focus() {
                            ui.painter().rect_stroke(button.rect, t.radius, egui::Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
                        }
                        button
                    })
                    .inner
            })
            .inner;
        self.button = Some(button.id);
        if button.clicked_elsewhere() {
            self.hide(ctx);
        }
        button.clicked()
    }

    pub(super) fn reset(&mut self, ctx: Option<&Context>) {
        if let Some(ctx) = ctx {
            self.hide(ctx);
        }
        *self = Self::default();
    }

    fn hide(&mut self, ctx: &Context) {
        if let Some(id) = self.button.take() {
            ctx.memory_mut(|memory| memory.surrender_focus(id));
            ctx.request_repaint();
        }
        self.shown_until = None;
    }
}
