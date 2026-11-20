//! A local speech player with cancellable, read-only text preparation off the desktop UI thread.
use crate::PdfCraftApp;
use pdfcraft_engine::{DocId, speech::Plan};
use pdfcraft_platform::speech::{Chunk, Options, Player, State};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

struct Preparation {
    receiver: mpsc::Receiver<Result<Plan, String>>,
    cancelled: Arc<AtomicBool>,
    options: Options,
}

impl Drop for Preparation {
    /// Signal cancellation when a pending text-preparation handle leaves the UI state.
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

pub struct ReadAloud {
    pub open: bool,
    pub player: Player,
    pub first: usize,
    pub last: usize,
    pub options: Options,
    pub error: Option<String>,
    pub empty_pages: Vec<usize>,
    pub(crate) source: Option<(DocId, Arc<Vec<u8>>)>,
    preparation: Option<Preparation>,
}

impl Default for ReadAloud {
    /// Create closed read-aloud controls with an idle player and a one-page default range.
    fn default() -> Self {
        Self {
            open: false,
            player: Player::new(),
            first: 1,
            last: 1,
            options: Options::default(),
            error: None,
            empty_pages: Vec::new(),
            source: None,
            preparation: None,
        }
    }
}

impl PdfCraftApp {
    /// Open controls at the active page and request local voices without starting playback.
    pub fn open_read_aloud(&mut self) {
        if let Some((i, _)) = self.active_ids() {
            let page = self.views.get(i).map_or(1, |v| v.current.saturating_add(1));
            self.read_aloud.first = page;
            self.read_aloud.last = page;
            self.read_aloud.open = true;
            if self.read_aloud.player.available()
                && let Err(e) = self.read_aloud.player.refresh_voices()
            {
                self.read_aloud.error = Some(e);
            }
        }
    }

    /// Prepare a 1-based range of at most 64 pages off the UI thread before queuing audio.
    /// Reject invalid ranges or concurrent preparation; retain the source snapshot for invalidation.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_read_aloud(&mut self, first: usize, last: usize) -> Result<(), String> {
        if self.read_aloud.preparation.is_some() {
            return Err("read-aloud preparation is still running; wait or stop it".into());
        }
        let (_, id) = self.active_ids().ok_or("open a PDF before reading aloud")?;
        let doc = self.session.get(id).ok_or("the document has closed")?;
        if first == 0 || last < first || last > doc.info.pages.len() || last.saturating_sub(first) >= pdfcraft_engine::speech::MAX_PAGES {
            return Err("choose a valid range of at most 64 pages".into());
        }
        self.read_aloud.options.validate()?;
        let source = doc.read_aloud_source(&(first - 1..last).collect::<Vec<_>>())?;
        let snapshot = (id, doc.bytes.clone());
        self.read_aloud.player.stop()?;
        self.read_aloud.error = None;
        self.read_aloud.empty_pages.clear();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let (tx, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("pdfcraft-speech-text".into())
            .spawn(move || {
                let _ = tx.send(source.extract_cancellable(&cancel));
            })
            .map_err(|e| format!("could not prepare read-aloud text: {e}"))?;
        self.read_aloud.source = Some(snapshot);
        self.read_aloud.preparation = Some(Preparation { receiver, cancelled, options: self.read_aloud.options.clone() });
        Ok(())
    }

    /// Return the unsupported-browser error without starting preparation or audio.
    #[cfg(target_arch = "wasm32")]
    pub fn start_read_aloud(&mut self, _first: usize, _last: usize) -> Result<(), String> {
        Err("Read out loud requires the desktop app on Windows, macOS or Linux".into())
    }

    /// Cancel text preparation, request playback Stop and detach the source snapshot.
    pub fn stop_read_aloud(&mut self) {
        if let Some(p) = &self.read_aloud.preparation {
            p.cancelled.store(true, Ordering::Relaxed);
        }
        if let Err(e) = self.read_aloud.player.stop() {
            self.read_aloud.error = Some(e);
        }
        self.read_aloud.source = None;
    }

    /// Stop stale-source playback, collect prepared chunks/errors and schedule bounded UI polling.
    pub(crate) fn poll_read_aloud(&mut self, ctx: &egui::Context) {
        if self.read_aloud.source.as_ref().is_some_and(|(id, bytes)| {
            self.active_ids().is_none_or(|(_, active)| active != *id) || self.session.get(*id).is_none_or(|d| !Arc::ptr_eq(bytes, &d.bytes))
        }) {
            self.stop_read_aloud();
        }
        let result = self.read_aloud.preparation.as_ref().and_then(|p| match p.receiver.try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("read-aloud text worker stopped".into())),
            Err(mpsc::TryRecvError::Empty) => None,
        });
        if let Some(result) = result
            && let Some(p) = self.read_aloud.preparation.take()
            && !p.cancelled.load(Ordering::Relaxed)
            && self.read_aloud.source.is_some()
        {
            let result = result.and_then(|plan| {
                self.read_aloud.empty_pages = plan.empty_pages;
                self.read_aloud.player.start(plan.chunks.into_iter().map(|c| Chunk { page: c.page, text: c.text }).collect(), p.options.clone())
            });
            if let Err(e) = result {
                self.read_aloud.error = Some(e);
            }
        }
        let status = self.read_aloud.player.status();
        if let Some(e) = status.error {
            self.read_aloud.error = Some(e);
        }
        if self.read_aloud.preparation.is_some() || matches!(status.state, State::Preparing | State::Speaking) {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if self.read_aloud.open {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }
}

/// Render page/voice/rate controls and asynchronous speech status; playback requires an explicit action.
pub(crate) fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.read_aloud.open {
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("Read out loud")).id(egui::Id::unique("read_aloud")).open(&mut open).default_width(440.0).resizable(false).show(
        ctx,
        |ui| {
            if !app.read_aloud.player.available() {
                ui.label(tl!("Read out loud requires Windows, macOS or Linux."));
                return;
            }
            let status = app.read_aloud.player.status();
            let max = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
            let running = app.read_aloud.preparation.is_some() || matches!(status.state, State::Preparing | State::Speaking | State::Paused);
            ui.add_enabled_ui(!running, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tl!("Pages"));
                    ui.add(egui::DragValue::new(&mut app.read_aloud.first).range(1..=max));
                    ui.label("–");
                    ui.add(egui::DragValue::new(&mut app.read_aloud.last).range(1..=max));
                    if ui.button(tl!("Current page")).clicked()
                        && let Some((i, _)) = app.active_ids()
                    {
                        app.read_aloud.first = app.views.get(i).map_or(1, |v| v.current.saturating_add(1));
                        app.read_aloud.last = app.read_aloud.first;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(tl!("Voice"));
                    let name = app
                        .read_aloud
                        .options
                        .voice
                        .as_ref()
                        .and_then(|id| status.voices.iter().find(|v| &v.id == id))
                        .map_or_else(|| tl!("Default").to_owned(), |v| format!("{} ({})", v.name, v.language));
                    craft_ui::ComboBox::from_id_salt("speech_voice").selected_text(name).show_ui(ui, |ui| {
                        ui.selectable_value(&mut app.read_aloud.options.voice, None, tl!("Default"));
                        for voice in &status.voices {
                            ui.selectable_value(
                                &mut app.read_aloud.options.voice,
                                Some(voice.id.clone()),
                                format!("{} ({})", voice.name, voice.language),
                            );
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(tl!("Speed"));
                    ui.add(egui::Slider::new(&mut app.read_aloud.options.rate, 0.5..=2.0).suffix("×"));
                });
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(!running && app.active.is_some(), egui::Button::new(tl!("Read out loud"))).clicked()
                    && let Err(e) = app.start_read_aloud(app.read_aloud.first, app.read_aloud.last)
                {
                    app.read_aloud.error = Some(e);
                }
                if ui.add_enabled(status.state == State::Speaking, egui::Button::new(tl!("Pause"))).clicked()
                    && let Err(e) = app.read_aloud.player.pause()
                {
                    app.read_aloud.error = Some(e);
                }
                if ui.add_enabled(status.state == State::Paused, egui::Button::new(tl!("Resume"))).clicked()
                    && let Err(e) = app.read_aloud.player.resume()
                {
                    app.read_aloud.error = Some(e);
                }
                if ui.add_enabled(running, egui::Button::new(tl!("Stop"))).clicked() {
                    app.stop_read_aloud();
                }
            });
            if app.read_aloud.preparation.is_some() {
                ui.label(tl!("Preparing read-aloud text…"));
            }
            if let Some(page) = status.page {
                ui.label(format!("{} {page} · {}/{}", tl!("Page"), status.chunk.saturating_add(1), status.total_chunks));
            }
            if status.state == State::Paused {
                ui.label(tl!("Paused"));
            }
            if status.state == State::Completed {
                ui.label(tl!("Completed"));
            }
            ui.label(tl!("Scanned pages need OCR. Reading order follows extracted page text."));
            if !app.read_aloud.empty_pages.is_empty() {
                ui.label(format!(
                    "{} · {}: {}",
                    tl!("Skipped"),
                    tl!("Pages"),
                    app.read_aloud.empty_pages.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")
                ));
            }
            if let Some(e) = &app.read_aloud.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        },
    );
    if !open {
        app.stop_read_aloud();
        app.read_aloud.open = false;
    }
}
