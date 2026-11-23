//! The window and its event loop while PdfCraft draws on the processor.
//!
//! eframe owns the window for its two GPU renderers and has no place for a third. This is what its
//! integrations do for PdfCraft's one window (`eframe::native::{run, epi_integration,
//! wgpu_integration}` at the pinned egui revision), with [`Painter`] where theirs call the GPU:
//! settings and window geometry in the same `app.ron`, the same close handshake
//! (`ViewportCommand::CancelClose`), the window shown after its first frame, accessibility,
//! clipboard, IME and viewport commands through egui-winit. Compare the two when egui is updated.
//!
//! One thing is different on purpose: frames are paced here, [`Setup::frame_interval`] apart, as
//! there is no display to wait for. Over Remote Desktop that is what keeps a burst of wheel events
//! from becoming a queue of frames each to be encoded and sent.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{ViewportBuilder, ViewportCommand, ViewportEvent, ViewportId, ViewportInfo};
use egui_winit::accesskit_winit;
use egui_winit::{ActionRequested, WindowGeometry, WindowSettings};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use super::painter::{MAX_TEXTURE_SIDE, Painter};
use super::raster::PixelRect;
use super::storage::FileStore;

/// eframe's keys in the settings file.
const WINDOW_KEY: &str = "window";
const MEMORY_KEY: &str = "egui";

/// A window that isn't shown is asked for no more frames than this.
const HIDDEN_INTERVAL: Duration = Duration::from_millis(100);

/// A redraw the system was asked for and didn't deliver within this is given up on: the app's
/// logic runs without it, and the next request asks again.
const MISSED_REDRAW: Duration = Duration::from_millis(250);

/// What the window is made from.
pub(crate) struct Setup<'a> {
    pub viewport: ViewportBuilder,
    /// `app.ron`; without one nothing is saved.
    pub settings_file: Option<PathBuf>,
    pub clipboard_shortcuts: bool,
    /// The least time from one frame to the next.
    pub frame_interval: Duration,
    pub app_creator: eframe::AppCreator<'a>,
    /// Called with the window before the app is created (the taskbar icon).
    pub window_created: fn(&Window),
}

#[derive(Debug)]
enum UserEvent {
    /// egui wants a frame at `when`; `pass` is the pass that asked.
    Repaint {
        when: Instant,
        pass: u64,
    },
    Accessibility(accesskit_winit::Event),
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Accessibility(event)
    }
}

/// Opens the window and runs until it is closed. An error means the window never showed a frame.
pub(crate) fn run(setup: Setup<'_>) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().map_err(|e| format!("no event loop: {e}"))?;
    let frame_interval = setup.frame_interval;
    let mut handler = Handler {
        setup: Some(setup),
        running: None,
        proxy: event_loop.create_proxy(),
        frame_interval,
        last_frame: Instant::now(),
        next_repaint: None,
        redraw_deadline: None,
        last_paint_error: None,
        error: None,
    };
    event_loop.run_app(&mut handler).map_err(|e| format!("the event loop stopped: {e}"))?;
    handler.error.map_or(Ok(()), Err)
}

struct Handler<'a> {
    setup: Option<Setup<'a>>,
    running: Option<Running<'a>>,
    proxy: EventLoopProxy<UserEvent>,
    frame_interval: Duration,
    /// When the last frame started.
    last_frame: Instant,
    /// When egui next wants a frame.
    next_repaint: Option<Instant>,
    /// Set while the system owes a `RedrawRequested`: when to stop waiting for it.
    redraw_deadline: Option<Instant>,
    /// The last painting failure logged, so a failure every frame is one line in the log.
    last_paint_error: Option<String>,
    /// Why the window never showed a frame.
    error: Option<String>,
}

impl Handler<'_> {
    fn schedule(&mut self, when: Instant) {
        self.next_repaint = Some(self.next_repaint.map_or(when, |sooner| sooner.min(when)));
    }

    /// Saves, closes the window and ends the loop.
    fn shut_down(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(running) = self.running.take() {
            running.save_and_exit();
        }
        event_loop.exit();
    }

    /// Runs one frame now.
    fn frame(&mut self, event_loop: &ActiveEventLoop, logic_only: bool) {
        let Some(running) = self.running.as_mut() else { return };
        self.last_frame = Instant::now();
        let result = running.frame(event_loop, logic_only);
        let shown = running.shown;
        match result {
            Ok(false) => {}
            Ok(true) => self.shut_down(event_loop),
            // Before anything was shown this is a renderer that can't start: say so and stop.
            Err(e) if !shown => {
                self.error = Some(e);
                self.shut_down(event_loop);
            }
            // Later, the window keeps its last picture and the documents stay open.
            Err(e) => {
                if self.last_paint_error.as_deref() != Some(e.as_str()) {
                    log::error!("the window was not redrawn: {e}");
                    self.last_paint_error = Some(e);
                }
            }
        }
    }

    /// Asks for the frame egui wants once it is due, and sleeps until the next thing to do.
    fn pace(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.redraw_deadline.is_some_and(|deadline| deadline <= now) {
            // The system didn't deliver the redraw (the window was hidden meanwhile): keep the
            // app's own work going.
            self.redraw_deadline = None;
            self.frame(event_loop, true);
        }
        if let (Some(when), Some(window)) = (self.next_repaint, self.running.as_ref().map(|running| Arc::clone(&running.window)))
            && when.max(self.last_frame + self.frame_interval) <= now
            && self.redraw_deadline.is_none()
        {
            self.next_repaint = None;
            if hidden(&window) {
                // Windows sends a hidden window no `RedrawRequested`, so the frame runs from here:
                // commands such as "show the window" are carried out, nothing is painted.
                self.frame(event_loop, false);
                if let Some(next) = &mut self.next_repaint {
                    *next = (*next).max(Instant::now() + HIDDEN_INTERVAL);
                }
            } else {
                window.request_redraw();
                self.redraw_deadline = Some(now + MISSED_REDRAW);
            }
        }
        let wake = [self.next_repaint.map(|when| when.max(self.last_frame + self.frame_interval)), self.redraw_deadline].into_iter().flatten().min();
        event_loop.set_control_flow(wake.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

/// The pixels of an update area Windows reports.
fn update_area(rect: winsafe::RECT) -> PixelRect {
    PixelRect::new(rect.left, rect.top, rect.right, rect.bottom)
}

/// The window's own handle, to ask Windows what it wants redrawn. It is found among this thread's
/// windows by value, as winsafe makes a handle from a raw one only in `unsafe` code. `None` only if
/// it isn't there, and then every frame presents just what changed, as before.
fn own_handle(window: &Window) -> Option<winsafe::HWND> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else { return None };
    let wanted = handle.hwnd.get().cast_unsigned();
    let mut found = None;
    // The search stops at the window, which Windows reports as an error: the result says nothing.
    let _ = winsafe::EnumThreadWindows(winsafe::GetCurrentThreadId(), |hwnd| {
        if hwnd.ptr().addr() == wanted {
            found = Some(hwnd);
            false
        } else {
            true
        }
    });
    found
}

/// Whether the window is hidden or minimized, and so gets no redraws from Windows.
fn hidden(window: &Window) -> bool {
    window.is_visible() == Some(false) || window.is_minimized() == Some(true)
}

impl ApplicationHandler<UserEvent> for Handler<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(setup) = self.setup.take() else { return };
        match Running::start(setup, event_loop, &self.proxy) {
            Ok(running) => {
                self.running = Some(running);
                self.frame(event_loop, false);
                self.pace(event_loop);
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, _cause: StartCause) {
        self.pace(event_loop);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.pace(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(running) = self.running.as_mut() else { return };
        // What the event asks for: a frame at a time, or one at once.
        let (repaint_at, at_once) = match event {
            UserEvent::Repaint { when, pass } => {
                // A request from before the last frame was answered by that frame.
                let current = running.ctx.cumulative_pass_nr_for(ViewportId::ROOT);
                ((current == pass || current == pass.saturating_add(1)).then_some(when), false)
            }
            UserEvent::Accessibility(event) => match event.window_event {
                accesskit_winit::WindowEvent::InitialTreeRequested => {
                    // The screen reader shows a placeholder until the first tree arrives.
                    running.ctx.enable_accesskit();
                    (None, true)
                }
                accesskit_winit::WindowEvent::ActionRequested(request) => {
                    running.state.on_accesskit_action_request(request);
                    (Some(Instant::now()), false)
                }
                accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                    running.ctx.disable_accesskit();
                    (None, false)
                }
            },
        };
        if let Some(when) = repaint_at {
            self.schedule(when);
        }
        if at_once {
            self.frame(event_loop, false);
        }
        self.pace(event_loop);
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, _device: DeviceId, event: DeviceEvent) {
        if let (DeviceEvent::MouseMotion { delta }, Some(running)) = (event, self.running.as_mut())
            && running.state.on_mouse_motion(delta)
        {
            self.schedule(Instant::now());
            self.pace(event_loop);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window: WindowId, event: WindowEvent) {
        let Some(running) = self.running.as_mut() else { return };
        if running.window.id() != window {
            return;
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            self.redraw_deadline = None;
            // Windows' own paint requests (uncovered, restored, the session reconnected) leave
            // an update area; a frame the app asked for has none and presents only what changed.
            if let Some(rect) = running.hwnd.as_ref().and_then(|hwnd| hwnd.GetUpdateRect(false)) {
                running.painter.expose(update_area(rect));
            }
            self.frame(event_loop, false);
            self.pace(event_loop);
            return;
        }
        match running.on_window_event(&event) {
            Reaction::Close => self.shut_down(event_loop),
            // Windows wants the resized window drawn before the handler returns, or it shows the
            // old picture stretched.
            Reaction::RepaintNow => self.frame(event_loop, false),
            Reaction::Repaint => self.schedule(Instant::now()),
            Reaction::Nothing => {}
        }
        self.pace(event_loop);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(running) = self.running.take() {
            running.save_and_exit();
        }
    }
}

/// What a window event asks of the loop.
enum Reaction {
    Nothing,
    Repaint,
    RepaintNow,
    Close,
}

struct Running<'a> {
    ctx: egui::Context,
    window: Arc<Window>,
    /// The window's Win32 handle, if it could be found.
    hwnd: Option<winsafe::HWND>,
    state: egui_winit::State,
    painter: Painter,
    app: Box<dyn eframe::App + 'a>,
    /// The `Frame` the app's methods are handed; PdfCraft reads nothing from it.
    frame: eframe::Frame,
    store: Option<FileStore>,
    info: ViewportInfo,
    /// The window as egui last described it, to tell what a new description changes.
    builder: ViewportBuilder,
    actions: Vec<ActionRequested>,
    /// Texture changes of passes that weren't painted yet.
    textures: egui::TexturesDelta,
    /// Input and output of passes that only ran the app's logic, for the next full one.
    pending_input: egui::RawInput,
    pending_output: egui::FullOutput,
    beginning: Instant,
    last_save: Instant,
    /// The window settings it was restored with, for what is applied when it is first shown.
    restored: Option<WindowSettings>,
    /// The window's geometry while not maximized, as last saved.
    normal_geometry: Option<WindowGeometry>,
    /// A frame was painted and the window shown.
    shown: bool,
    /// Closing was asked for and the app didn't cancel it.
    close: bool,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        // The window is going: texture changes no frame painted have nowhere to go. egui checks
        // that they aren't dropped by oversight, so they are let go of here by name.
        self.textures.clear();
        self.pending_output.textures_delta.clear();
    }
}

/// The size, in points, of the largest monitor: a window restored larger than that doesn't appear.
fn largest_monitor(zoom: f32, event_loop: &ActiveEventLoop) -> egui::Vec2 {
    let mut largest = egui::Vec2::ZERO;
    for monitor in event_loop.available_monitors() {
        let size = monitor.size().to_logical::<f32>(f64::from(zoom) * monitor.scale_factor());
        largest = largest.max(egui::vec2(size.width, size.height));
    }
    if largest == egui::Vec2::ZERO { egui::Vec2::splat(16000.0) } else { largest }
}

impl<'a> Running<'a> {
    fn start(setup: Setup<'a>, event_loop: &ActiveEventLoop, proxy: &EventLoopProxy<UserEvent>) -> Result<Self, String> {
        let Setup { viewport, settings_file, clipboard_shortcuts, frame_interval: _, app_creator, window_created } = setup;
        let store = settings_file.map(FileStore::open);
        let stored = store.as_ref().map(|store| store as &dyn eframe::Storage);

        let ctx = egui::Context::default();
        // PdfCraft has one window; anything asking for another gets it inside this one.
        ctx.set_embed_viewports(true);
        // As eframe: a pass may ask to be run again (`Context::request_discard`).
        ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::MIN.saturating_add(1));
        if let Some(memory) = stored.and_then(|s| eframe::get_value::<egui::Memory>(s, MEMORY_KEY)) {
            ctx.memory_mut(|m| *m = memory);
        }

        let zoom = ctx.zoom_factor();
        let mut restored: Option<WindowSettings> = stored.and_then(|s| eframe::get_value(s, WINDOW_KEY));
        let largest = largest_monitor(zoom, event_loop);
        let mut builder = viewport;
        if let Some(settings) = &mut restored {
            settings.clamp_size_to_sane_values(largest);
            settings.clamp_position_to_monitors(zoom, event_loop);
            builder = settings.initialize_viewport_builder(zoom, builder);
        } else if let Some(size) = builder.inner_size {
            builder = builder.with_inner_size(size.min(largest));
        }
        // Hidden until the first frame is painted, so the window never shows up blank.
        let builder = builder.with_visible(false);
        let window = egui_winit::create_window(&ctx, event_loop, &builder).map_err(|e| format!("the window couldn't be created: {e}"))?;
        if let Some(settings) = &restored {
            settings.initialize_window(&window);
        }

        let mut info = ViewportInfo::default();
        egui_winit::update_viewport_info(&mut info, &ctx, &window, true);
        // The app is told the scale while it is created, before any pass.
        let pixels_per_point = egui_winit::pixels_per_point(&ctx, &window);
        ctx.input_mut(|i| {
            i.raw.viewports.insert(ViewportId::ROOT, info.clone());
            i.pixels_per_point = pixels_per_point;
        });

        let window = Arc::new(window);
        let painter = Painter::new(&window)?;

        // `Context::request_repaint` may come from any thread; the proxy wakes the loop.
        let waker = Mutex::new(proxy.clone());
        ctx.set_request_repaint_callback(move |request| {
            let when = Instant::now() + request.delay;
            if let Ok(waker) = waker.lock() {
                // An error means the loop is gone, and so is the window.
                let _ = waker.send_event(UserEvent::Repaint { when, pass: request.current_cumulative_pass_nr });
            }
        });

        let mut state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            event_loop.system_theme(),
            Some(MAX_TEXTURE_SIDE),
        );
        state.set_clipboard_shortcuts(clipboard_shortcuts);
        state.init_accesskit(event_loop, &window, proxy.clone());

        window_created(&window);
        let app = {
            // eframe makes this context only inside its own integrations. The constructor it
            // keeps for tests gives one with no GPU state, which is the truth here; the settings
            // are added to it.
            let mut creation = eframe::CreationContext::_new_kittest(ctx.clone());
            creation.storage = stored;
            app_creator(&creation).map_err(|e| e.to_string())?
        };

        let normal_geometry = restored
            .map(|settings| settings.normal_geometry())
            .or_else(|| (!window.is_maximized() && window.fullscreen().is_none()).then(|| WindowGeometry::from_window(zoom, &window)));
        let now = Instant::now();
        let hwnd = own_handle(&window);
        if hwnd.is_none() {
            log::debug!("the window's handle wasn't found: parts Windows asks to redraw wait for the next change");
        }
        Ok(Self {
            ctx,
            hwnd,
            window,
            state,
            painter,
            app,
            frame: eframe::Frame::_new_kittest(),
            store,
            info,
            builder,
            actions: Vec::new(),
            textures: egui::TexturesDelta::default(),
            pending_input: egui::RawInput::default(),
            pending_output: egui::FullOutput::default(),
            beginning: now,
            last_save: now,
            restored,
            normal_geometry,
            shown: false,
            close: false,
        })
    }

    fn on_window_event(&mut self, event: &WindowEvent) -> Reaction {
        let mut now = false;
        match event {
            WindowEvent::Resized(_) => {
                // The size in the event can be stale; the painter reads the window's own. A
                // minimized window reports no size and is not drawn.
                let size = self.window.inner_size();
                if size.width > 0 && size.height > 0 {
                    self.painter.invalidate();
                    now = true;
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.info.occluded = Some(*occluded);
                // Shown again (restored, or the session reconnected): the whole picture is drawn
                // and presented. Usually Windows' update area says the same; while winit is busy
                // it hands the paint on without one, and this is what still gets it drawn.
                if !*occluded {
                    self.painter.invalidate();
                    self.ctx.request_repaint_of(ViewportId::ROOT);
                }
            }
            WindowEvent::CloseRequested => {
                if self.close {
                    return Reaction::Close;
                }
                // The app hears of it in the next pass and may cancel (unsaved changes).
                self.info.events.push(ViewportEvent::Close);
                self.ctx.request_repaint_of(ViewportId::ROOT);
            }
            _ => {}
        }
        let response = self.state.on_window_event(&self.window, event);
        if self.close {
            Reaction::Close
        } else if response.repaint {
            if now { Reaction::RepaintNow } else { Reaction::Repaint }
        } else {
            Reaction::Nothing
        }
    }

    /// The input for a pass: what egui-winit collected, after what earlier logic-only passes
    /// couldn't hand to egui.
    fn take_input(&mut self) -> egui::RawInput {
        let mut fresh = self.state.take_egui_input(&self.window);
        fresh.viewports = std::iter::once((ViewportId::ROOT, self.info.clone())).collect();
        let mut input = std::mem::take(&mut self.pending_input);
        input.append(fresh);
        input.time = Some(self.beginning.elapsed().as_secs_f64());
        self.app.raw_input_hook(&self.ctx, &mut input);
        input
    }

    /// Runs one pass and paints it. `logic_only` runs the app's logic without its interface.
    /// `Ok(true)` when the window is to close; an error when it couldn't be painted.
    fn frame(&mut self, event_loop: &ActiveEventLoop, logic_only: bool) -> Result<bool, String> {
        egui_winit::update_viewport_info(&mut self.info, &self.ctx, &self.window, false);
        // A hidden window is not painted, unless someone wants its pixels (a screenshot).
        let visible = self.info.visible().unwrap_or(true) || self.actions.iter().any(ActionRequested::wants_paint);
        let input = self.take_input();
        let close_requested = input.viewport().close_requested();
        if logic_only || !visible {
            // No egui pass: the interface's state is left as it is for when it shows again.
            let (app, frame) = (&mut self.app, &mut self.frame);
            let output = self.ctx.run_logic(&input, |ctx| app.logic(ctx, frame));
            self.pending_input = input;
            let mut commands = output.viewport_commands;
            let commands = commands.remove(&ViewportId::ROOT).unwrap_or_default();
            self.note_close(close_requested, &commands);
            self.info.events.clear();
            self.state.handle_platform_output_with_event_loop(&self.window, event_loop, output.platform_output);
            self.run_commands(commands);
            self.rest_if_hidden();
            return Ok(self.close);
        }

        let (app, frame) = (&mut self.app, &mut self.frame);
        let output = self.ctx.run_ui(input, |ui| {
            app.logic(ui.ctx(), frame);
            app.ui(ui, frame);
        });
        self.pending_output.append(output);
        let egui::FullOutput { platform_output, textures_delta, shapes, pixels_per_point, mut viewport_output } =
            std::mem::take(&mut self.pending_output);
        self.textures.append(textures_delta);
        let (described, mut commands) = match viewport_output.remove(&ViewportId::ROOT) {
            Some(output) => (Some(output.builder), output.commands),
            None => (None, Vec::new()),
        };
        self.note_close(close_requested, &commands);
        self.info.events.clear();

        // IME permission first, as eframe: turning it off must come before this pass's text
        // around the caret is published.
        let (ime, rest): (Vec<_>, Vec<_>) = commands.drain(..).partition(|command| matches!(command, ViewportCommand::IMEAllowed(_)));
        self.run_commands(ime);
        self.state.handle_platform_output_with_event_loop(&self.window, event_loop, platform_output);

        let painted = self.paint(shapes, pixels_per_point);

        // What egui now says the window is (its title, mostly), then what the app asked of it.
        let mut commands = described.map_or_else(Vec::new, |described| self.builder.patch(described).0);
        commands.extend(rest);
        self.run_commands(commands);

        if self.last_save.elapsed() > self.app.auto_save_interval() {
            self.save();
        }
        self.rest_if_hidden();
        painted.map(|()| self.close)
    }

    /// Closing was asked for in this pass: it goes ahead unless the app cancelled it.
    fn note_close(&mut self, requested: bool, commands: &[ViewportCommand]) {
        if requested && !commands.contains(&ViewportCommand::CancelClose) {
            self.close = true;
        }
    }

    fn run_commands(&mut self, commands: Vec<ViewportCommand>) {
        if !commands.is_empty() {
            egui_winit::process_viewport_commands(&self.ctx, &mut self.info, commands, &self.window, &mut self.state, &mut self.actions);
        }
    }

    fn paint(&mut self, shapes: Vec<egui::epaint::ClippedShape>, pixels_per_point: f32) -> Result<(), String> {
        let primitives = self.ctx.tessellate(shapes, pixels_per_point);
        let mut screenshots = Vec::new();
        self.actions.retain(|action| match action {
            ActionRequested::Screenshot(callback) => {
                screenshots.push(callback.clone());
                false
            }
            _ => true,
        });
        let [r, g, b, _] = self.app.clear_color(&self.ctx.global_style().visuals);
        let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let clear = egui::Color32::from_rgb(channel(r), channel(g), channel(b));
        let started = Instant::now();
        let (painted, image) = self.painter.paint(&self.window, pixels_per_point, clear, &primitives, &mut self.textures, !screenshots.is_empty())?;
        log::trace!("painted {} pixels of {} meshes in {:?}", painted.pixels, primitives.len(), started.elapsed());
        if let Some(image) = image.map(Arc::new) {
            for callback in screenshots {
                callback.complete(Arc::clone(&image));
            }
        }
        for action in self.actions.drain(..) {
            match action {
                // Screenshots were taken above, and painting is all `PaintWhileHidden` asks for.
                ActionRequested::Screenshot(_) | ActionRequested::PaintWhileHidden => {}
                ActionRequested::Cut => self.state.request_cut(&self.window),
                ActionRequested::Copy => self.state.request_copy(&self.window),
                ActionRequested::Paste => self.state.request_paste(&self.window),
            }
        }
        if !std::mem::replace(&mut self.shown, true) {
            if let Some(settings) = &self.restored {
                settings.before_first_show(&self.window);
            }
            self.window.set_visible(true);
            if let Some(settings) = &self.restored {
                settings.after_first_show(&self.window);
            }
        }
        Ok(())
    }

    /// A minimized or hidden window that keeps asking for frames would use a whole processor.
    fn rest_if_hidden(&self) {
        if hidden(&self.window) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn save(&mut self) {
        self.last_save = Instant::now();
        let Some(store) = self.store.as_mut() else { return };
        let settings = WindowSettings::from_window(self.ctx.zoom_factor(), &self.window, self.normal_geometry);
        self.normal_geometry = Some(settings.normal_geometry());
        eframe::set_value(store, WINDOW_KEY, &settings);
        if self.app.persist_egui_memory() {
            self.ctx.memory(|memory| eframe::set_value(store, MEMORY_KEY, memory));
        }
        self.app.save(store);
        eframe::Storage::flush(store);
    }

    fn save_and_exit(mut self) {
        self.ctx.on_exit();
        self.save();
        self.app.on_exit(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_update_area_windows_reports_is_the_same_pixels() {
        let rect = winsafe::RECT { left: 5, top: 7, right: 640, bottom: 480 };
        assert_eq!(update_area(rect), PixelRect::new(5, 7, 640, 480));
    }
}
