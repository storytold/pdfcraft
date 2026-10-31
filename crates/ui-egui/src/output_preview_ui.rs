//! Print production ▸ Output preview: while the panel is open, the pages on screen show what
//! their inks print (each page separated into plates on a worker thread, then composed back),
//! the panel lists the inks with a switch each, reads every ink's coverage under the pointer,
//! and warns where the total area coverage passes a limit. It names what the process plates
//! simulate: the output intent's CMYK profile, or the device formulas.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, mpsc};

use egui::{Color32, Rect, Sense, vec2};
use pdfcraft_render::{Coverage, Separations, Simulation};

use crate::canvas::PageXform;
use crate::theme::{self, Tokens};

/// A page's long side, in device pixels, at which it is separated. Separations don't follow the
/// zoom (each zoom step would separate the page again); this keeps a page sharp at about
/// 180 dpi on A4, within `MAX_PLATE_PIXELS`.
const LONG_SIDE: f32 = 2000.0;
/// Pages kept separated at once (the current one and its neighbours on screen).
const KEEP: usize = 4;
/// Where the total area coverage is above the limit, the composite shows this colour.
const MARK: [u8; 3] = [0, 220, 0];

/// What a page was separated from: the document as displayed and the separation's options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Source {
    display: usize,
    layers: usize,
    hide_comments: bool,
    overprint: bool,
}

/// How the composite was drawn.
#[derive(Clone, Debug, PartialEq)]
struct Look {
    hidden: BTreeSet<String>,
    warn: Option<u32>,
}

struct Entry {
    source: Source,
    result: Result<Arc<Separations>, String>,
    texture: Option<(Look, egui::TextureHandle)>,
}

struct Job {
    page: usize,
    source: Source,
    receiver: mpsc::Receiver<Result<Separations, String>>,
}

/// Output preview state of one document view.
pub(crate) struct OutputPreview {
    /// Honour overprinting (`OP`, `op`, `OPM`) as a separating device does.
    pub(crate) overprint: bool,
    /// Inks switched off, by name.
    pub(crate) hidden: BTreeSet<String>,
    /// Show where the total area coverage is above `limit`.
    pub(crate) warn: bool,
    /// The total area coverage limit, in percent.
    pub(crate) limit: f32,
    pages: HashMap<usize, Entry>,
    job: Option<Job>,
    /// The ink coverage under the pointer this frame: (page, coverage).
    hover: Option<(usize, Coverage)>,
    open: bool,
}

impl Default for OutputPreview {
    fn default() -> Self {
        Self { overprint: true, hidden: BTreeSet::new(), warn: false, limit: 300.0, pages: HashMap::new(), job: None, hover: None, open: false }
    }
}

impl OutputPreview {
    /// A page is being separated for the open panel.
    pub(crate) fn pending(&self) -> bool {
        self.open && self.job.is_some()
    }

    /// Drop the separations and textures, keeping the panel's settings (hidden tabs).
    pub(crate) fn release(&mut self) {
        self.pages.clear();
        self.job = None;
        self.hover = None;
    }

    fn enabled(&self, s: &Separations) -> Vec<bool> {
        s.inks.iter().map(|i| !self.hidden.contains(i.name())).collect()
    }

    fn look(&self) -> Look {
        Look { hidden: self.hidden.clone(), warn: self.warn.then(|| self.limit.round().max(0.0) as u32) }
    }

    /// Once per frame: separate the pages on screen (`current` first) on a worker thread and
    /// collect what finished. `open` is whether the panel shows; closing it lets everything go.
    pub(crate) fn prepare(
        &mut self,
        ctx: &egui::Context,
        doc: &pdfcraft_engine::Document,
        open: bool,
        current: usize,
        on_screen: &[usize],
        inline: bool,
    ) {
        self.open = open;
        self.hover = None;
        if !open {
            self.release();
            return;
        }
        let config = doc.separation_config();
        let source = Source {
            display: Arc::as_ptr(&doc.display) as usize,
            layers: Arc::as_ptr(&config.layers) as usize,
            hide_comments: config.hide_comments,
            overprint: self.overprint,
        };
        if let Some(job) = &self.job {
            match job.receiver.try_recv() {
                Ok(result) => {
                    let (page, made_from) = (job.page, job.source);
                    self.job = None;
                    self.pages.insert(page, Entry { source: made_from, result: result.map(Arc::new), texture: None });
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    let page = job.page;
                    self.job = None;
                    self.pages.insert(page, Entry { source, result: Err("the separation stopped".into()), texture: None });
                }
            }
        }
        let mut wanted: Vec<usize> = std::iter::once(current).chain(on_screen.iter().copied().filter(|p| *p != current)).collect();
        wanted.truncate(KEEP);
        self.pages.retain(|page, entry| wanted.contains(page) && entry.source == source);
        if self.job.is_some() {
            return;
        }
        let Some(page) = wanted.into_iter().find(|p| !self.pages.contains_key(p)) else { return };
        let Some(info) = doc.info.pages.get(page) else { return };
        let scale = LONG_SIDE / info.width.max(info.height).max(1.0);
        let (bytes, overprint) = (doc.display.clone(), self.overprint);
        let work = move || pdfcraft_render::PageRenderer::new(bytes, config).separate(page, scale, overprint);
        if inline || cfg!(target_arch = "wasm32") {
            self.pages.insert(page, Entry { source, result: work().map(Arc::new), texture: None });
            ctx.request_repaint();
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new().name("output-preview".into()).spawn(move || {
            let _ = sender.send(work());
            repaint.request_repaint();
        });
        match spawned {
            Ok(_) => self.job = Some(Job { page, source, receiver }),
            Err(e) => {
                self.pages.insert(page, Entry { source, result: Err(e.to_string()), texture: None });
            }
        }
    }

    /// Draw page `page`'s composite over its raster, and read the coverage under `pointer`.
    pub(crate) fn paint(&mut self, ctx: &egui::Context, painter: &egui::Painter, xf: &PageXform, page: usize, pointer: Option<egui::Pos2>) {
        if !self.open {
            return;
        }
        let look = self.look();
        let warn = look.warn.map(|limit| (limit as f32, MARK));
        let Some(entry) = self.pages.get_mut(&page) else { return };
        let Ok(s) = &entry.result else { return };
        let enabled: Vec<bool> = s.inks.iter().map(|i| !look.hidden.contains(i.name())).collect();
        if entry.texture.as_ref().is_none_or(|(l, _)| *l != look) {
            let image = crate::canvas::texture_image([s.width as usize, s.height as usize], s.composite(&enabled, warn));
            entry.texture = Some((look.clone(), ctx.load_texture(format!("output-preview-{page}"), image, egui::TextureOptions::LINEAR)));
        }
        if let Some((_, tex)) = &entry.texture {
            let painter = painter.with_clip_rect(painter.clip_rect().intersect(xf.rect));
            xf.paint_image(&painter, tex.id(), 0.0, 0.0, 1.0, 1.0);
        }
        if let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) {
            let (u, v) = xf.screen_to_norm(p);
            if let Some(c) = s.pixel_at(u, v).and_then(|(x, y)| s.coverage_at(x, y, &enabled)) {
                self.hover = Some((page, c));
            }
        }
    }
}

fn color([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

fn percent(v: f32) -> String {
    format!("{}%", v.round() as i64)
}

/// A row: a label, and a value at the right.
fn row(ui: &mut egui::Ui, t: &Tokens, label: &str, value: Option<String>, value_color: Color32) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).font(theme::regular(13.0)).color(t.text));
        if let Some(v) = value {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(v).font(theme::medium(13.0)).color(value_color));
            });
        }
    });
}

/// What the process plates simulate: the output intent's profile, or the device formulas.
fn simulation_row(ui: &mut egui::Ui, t: &Tokens, simulation: &Simulation) {
    let text = match simulation {
        Simulation::OutputIntent { profile, .. } => crate::i18n::fmt(tl!("Simulation: {profile}"), &[("profile", profile)]),
        Simulation::DeviceFormulas { .. } => tl!("Simulation: device formulas").to_string(),
    };
    let label = ui.label(egui::RichText::new(text).font(theme::regular(12.5)).color(t.text_muted));
    if matches!(simulation, Simulation::DeviceFormulas { .. }) {
        label.on_hover_text(tl!("No usable CMYK output intent: RGB and CIE-based colours separate by the ISO 32000 device formulas."));
    }
    ui.add_space(6.0);
}

/// The panel body. `current` is the page shown as current.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, op: &mut OutputPreview, current: usize) {
    let warning = crate::combine_ui::WARNING;
    ui.add(crate::widgets::checkbox(&mut op.overprint, tl!("Simulate overprinting")));
    ui.add_space(8.0);
    // The page under the pointer, once separated; otherwise the current page.
    let shown = op.hover.as_ref().map(|(page, _)| *page).filter(|page| op.pages.contains_key(page)).unwrap_or(current);
    let hover = op.hover.as_ref().filter(|(page, _)| *page == shown).map(|(_, c)| c.clone());
    let entry = op.pages.get(&shown).map(|e| e.result.clone());
    let Some(result) = entry else {
        ui.horizontal(|ui| {
            ui.spinner();
            let n = (shown + 1).to_string();
            ui.label(crate::i18n::fmt(tl!("Separating page {n}…"), &[("n", &n)]));
        });
        return;
    };
    let s = match result {
        Ok(s) => s,
        Err(e) => {
            ui.label(
                egui::RichText::new(crate::i18n::fmt(tl!("The page could not be separated: {error}"), &[("error", &e)]))
                    .color(crate::combine_ui::ERROR),
            );
            return;
        }
    };
    simulation_row(ui, t, &s.simulation);
    ui.label(egui::RichText::new(tl!("Inks")).font(theme::semibold(13.0)).color(t.text_muted));
    for (i, ink) in s.inks.iter().enumerate() {
        let name = ink.name().to_string();
        let mut on = !op.hidden.contains(&name);
        ui.horizontal(|ui| {
            // The switch carries the ink's name; the name beside it is painted, not a second
            // label for the same ink.
            let switch = ui.add(crate::widgets::checkbox_without_text(&mut on));
            switch.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, &name));
            if switch.changed() {
                if on {
                    op.hidden.remove(&name);
                } else {
                    op.hidden.insert(name.clone());
                }
            }
            let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            ui.painter().rect_filled(rect, 2.0, color(ink.swatch));
            ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            let width = (ui.available_width() - 48.0).max(0.0);
            let (rect, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
            let fg = if on { t.text } else { t.text_faint };
            crate::widgets::paint_left(ui, rect.left_center(), &name, theme::regular(13.0), fg, width);
            if let Some(v) = hover.as_ref().and_then(|c| c.inks.get(i)) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(percent(*v)).font(theme::medium(13.0)).color(fg));
                });
            }
        });
    }
    if !s.unseparated.is_empty() {
        let names = s.unseparated.join(", ");
        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Too many spot inks to separate: {names}"), &[("names", &names)])).color(t.text_faint));
    }
    ui.add_space(6.0);
    ui.separator();
    let enabled = op.enabled(&s);
    let over = |v: f32| if v > op.limit { warning } else { t.text };
    let total = hover.as_ref().map(|c| c.total);
    row(ui, t, tl!("Total area coverage"), total.map(percent), total.map_or(t.text, over));
    if let Some((max, _, _)) = s.max_total(&enabled) {
        let n = (max.round() as i64).to_string();
        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Page maximum: {n}%"), &[("n", &n)])).font(theme::regular(12.5)).color(over(max)));
    }
    if hover.is_none() {
        ui.label(egui::RichText::new(tl!("Point at the page to read its ink coverage.")).font(theme::regular(12.0)).color(t.text_faint));
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add(crate::widgets::checkbox(&mut op.warn, tl!("Highlight coverage above")));
        ui.add(egui::DragValue::new(&mut op.limit).range(0.0..=400.0).speed(1.0).suffix("%").max_decimals(0));
        let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        ui.painter().rect_filled(Rect::from_center_size(rect.center(), vec2(12.0, 12.0)), 2.0, color(MARK));
    });
}
