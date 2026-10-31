use adw::{glib, gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_automation::Automation;
use pdfcraft_engine::{DocId, DocInfo, RenderRequest};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::{Rc, Weak},
};

mod annotations;

const POINTS_TO_PIXELS: f64 = 96.0 / 72.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PageLayout {
    Single,
    TwoUp,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    None,
    Page,
    Width,
}

#[derive(Clone, Copy, PartialEq)]
struct ViewState {
    page: usize,
    zoom: f64,
    fit: Fit,
    layout: PageLayout,
    continuous: bool,
    gaps: bool,
    cover: bool,
}

pub struct DocumentView {
    pub id: DocId,
    pub name: RefCell<String>,
    pub path: RefCell<Option<PathBuf>>,
    pub(crate) automation: Rc<RefCell<Automation>>,
    pub info: RefCell<DocInfo>,
    pub root: adw::ToolbarView,
    pub scroll: gtk::ScrolledWindow,
    pub page_label: gtk::Label,
    pub zoom_label: gtk::Label,
    pub prev_button: gtk::Button,
    pub next_button: gtk::Button,
    pub zoom_out_button: gtk::Button,
    pub zoom_in_button: gtk::Button,
    pub fit_button: gtk::Button,
    pub actual_button: gtk::Button,
    pub spinner: gtk::Spinner,
    pub bottom_bar: gtk::Box,
    page_box: gtk::Box,
    marquee: Cell<bool>,
    drag: RefCell<Option<(f64, f64, f64, f64)>>,
    pictures: RefCell<Vec<(usize, gtk::Picture)>>,
    rendered: RefCell<HashMap<usize, u64>>,
    current: Cell<usize>,
    zoom: Cell<f64>,
    fit: Cell<Fit>,
    layout: Cell<PageLayout>,
    continuous: Cell<bool>,
    gaps: Cell<bool>,
    cover: Cell<bool>,
    tag: Cell<u64>,
    history: RefCell<Vec<ViewState>>,
    history_index: Cell<usize>,
    weak_self: RefCell<Weak<DocumentView>>,
    annotation_tool: RefCell<Option<String>>,
    annotation_stroke: RefCell<Vec<[f64; 2]>>,
    attachment: RefCell<Option<(String, Vec<u8>)>>,
}

impl DocumentView {
    pub fn new(id: DocId, name: String, path: Option<PathBuf>, info: DocInfo, automation: Rc<RefCell<Automation>>) -> Rc<Self> {
        let root = adw::ToolbarView::new();
        let page_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page_box.set_margin_top(24);
        page_box.set_margin_bottom(24);
        page_box.set_margin_start(24);
        page_box.set_margin_end(24);
        page_box.set_halign(gtk::Align::Center);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroll.set_child(Some(&page_box));
        root.set_content(Some(&scroll));

        let prev_button = icon_button("go-previous-symbolic", "Página anterior");
        let next_button = icon_button("go-next-symbolic", "Página siguiente");
        let zoom_out_button = icon_button("zoom-out-symbolic", "Reducir");
        let zoom_in_button = icon_button("zoom-in-symbolic", "Ampliar");
        let page_label = gtk::Label::new(None);
        page_label.set_width_chars(14);
        let zoom_label = gtk::Label::new(Some("100 %"));
        zoom_label.set_width_chars(7);
        let fit_button = gtk::Button::with_label("Ajustar");
        fit_button.set_tooltip_text(Some("Ajustar la página al área visible"));
        let actual_button = gtk::Button::with_label("100 %");
        actual_button.set_tooltip_text(Some("Tamaño real"));
        let spinner = gtk::Spinner::new();
        spinner.set_spinning(true);
        spinner.set_visible(false);

        let navigation = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        navigation.set_halign(gtk::Align::Center);
        navigation.set_margin_top(8);
        navigation.set_margin_bottom(8);
        navigation.append(&prev_button);
        navigation.append(&page_label);
        navigation.append(&next_button);
        navigation.append(&spinner);
        navigation.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        navigation.append(&zoom_out_button);
        navigation.append(&zoom_label);
        navigation.append(&zoom_in_button);
        navigation.append(&fit_button);
        navigation.append(&actual_button);
        let bottom_bar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bottom_bar.add_css_class("toolbar");
        bottom_bar.append(&navigation);
        root.add_bottom_bar(&bottom_bar);

        let view = Rc::new(Self {
            id,
            name: RefCell::new(name),
            path: RefCell::new(path),
            automation,
            info: RefCell::new(info),
            root,
            scroll,
            page_label,
            zoom_label,
            prev_button,
            next_button,
            zoom_out_button,
            zoom_in_button,
            fit_button,
            actual_button,
            spinner,
            bottom_bar,
            page_box,
            marquee: Cell::new(false),
            drag: RefCell::new(None),
            pictures: RefCell::new(Vec::new()),
            rendered: RefCell::new(HashMap::new()),
            current: Cell::new(0),
            zoom: Cell::new(1.0),
            fit: Cell::new(Fit::Page),
            layout: Cell::new(PageLayout::Single),
            continuous: Cell::new(false),
            gaps: Cell::new(true),
            cover: Cell::new(false),
            tag: Cell::new(0),
            history: RefCell::new(Vec::new()),
            history_index: Cell::new(0),
            weak_self: RefCell::new(Weak::new()),
            annotation_tool: RefCell::new(None),
            annotation_stroke: RefCell::new(Vec::new()),
            attachment: RefCell::new(None),
        });
        *view.weak_self.borrow_mut() = Rc::downgrade(&view);
        view.update_navigation();
        view.rebuild_pages();
        view.push_history();
        view.install_marquee();
        view
    }

    fn install_marquee(self: &Rc<Self>) {
        let gesture = gtk::GestureDrag::new();
        let view = self.clone();
        gesture.connect_drag_begin(move |_, x, y| {
            if view.marquee.get() {
                *view.drag.borrow_mut() = Some((x, y, 0.0, 0.0));
            }
        });
        let view = self.clone();
        gesture.connect_drag_update(move |_, x, y| {
            if let Some((start_x, start_y, _, _)) = *view.drag.borrow() {
                *view.drag.borrow_mut() = Some((start_x, start_y, x, y));
            }
        });
        let view = self.clone();
        gesture.connect_drag_end(move |_, _, _| {
            let Some((_x, _y, width, height)) = view.drag.borrow_mut().take() else { return };
            if !view.marquee.get() || width.abs() < 20.0 || height.abs() < 20.0 {
                return;
            }
            let available_width = f64::from(view.scroll.width()).max(1.0);
            let available_height = f64::from(view.scroll.height()).max(1.0);
            let factor = (available_width / width.abs()).min(available_height / height.abs()).clamp(0.1, 8.0);
            view.fit.set(Fit::None);
            view.zoom.set((view.zoom.get() * factor).clamp(0.1, 8.0));
            view.marquee.set(false);
            view.page_box.set_cursor_from_name(None);
            view.push_history();
            view.request_render(&view.automation);
        });
        self.page_box.add_controller(gesture);
    }

    pub fn set_marquee(&self, enabled: bool) {
        self.marquee.set(enabled);
        self.page_box.set_cursor_from_name(if enabled { Some("crosshair") } else { None });
    }

    pub fn current(&self) -> usize {
        self.current.get()
    }

    pub fn name(&self) -> String {
        self.name.borrow().clone()
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.path.borrow().clone()
    }

    pub fn set_location(&self, name: String, path: Option<PathBuf>) {
        *self.name.borrow_mut() = name;
        *self.path.borrow_mut() = path;
    }

    pub fn refresh(&self, automation: &Rc<RefCell<Automation>>) {
        let info = automation.borrow().session().get(self.id).map(|document| document.info.clone());
        let Some(info) = info else { return };
        let count = info.pages.len();
        *self.info.borrow_mut() = info;
        self.current.set(self.current.get().min(count.saturating_sub(1)));
        self.update_navigation();
        self.rebuild_pages();
        self.request_render(automation);
    }

    pub fn page_count(&self) -> usize {
        self.info.borrow().pages.len()
    }

    pub fn has_page(&self) -> bool {
        let tag = self.tag.get();
        tag != 0 && self.rendered.borrow().get(&self.current.get()) == Some(&tag)
    }

    pub fn go_to(&self, page: usize, automation: &Rc<RefCell<Automation>>) {
        self.go_to_inner(page, automation, true);
    }

    fn go_to_inner(&self, page: usize, automation: &Rc<RefCell<Automation>>, record: bool) {
        let count = self.info.borrow().pages.len();
        if count == 0 {
            return;
        }
        let page = page.min(count - 1);
        let changed = self.current.get() != page;
        self.current.set(page);
        self.update_navigation();
        if changed {
            self.rebuild_pages();
        }
        self.request_render(automation);
        if record {
            self.push_history();
        }
    }

    pub fn next(&self, automation: &Rc<RefCell<Automation>>) {
        self.go_to(self.current.get().saturating_add(1), automation);
    }

    pub fn previous(&self, automation: &Rc<RefCell<Automation>>) {
        self.go_to(self.current.get().saturating_sub(1), automation);
    }

    pub fn zoom_in(&self, automation: &Rc<RefCell<Automation>>) {
        self.set_zoom(self.zoom.get() * 1.25, automation);
    }

    pub fn zoom_out(&self, automation: &Rc<RefCell<Automation>>) {
        self.set_zoom(self.zoom.get() / 1.25, automation);
    }

    pub fn actual_size(&self, automation: &Rc<RefCell<Automation>>) {
        self.set_zoom(1.0, automation);
    }

    pub fn fit_page(&self, automation: &Rc<RefCell<Automation>>) {
        self.fit.set(Fit::Page);
        self.zoom_label.set_text("Auto");
        self.request_render(automation);
        self.push_history();
    }

    fn set_zoom(&self, zoom: f64, automation: &Rc<RefCell<Automation>>) {
        self.fit.set(Fit::None);
        self.zoom.set(zoom.clamp(0.1, 8.0));
        self.zoom_label.set_text(&format!("{:.0} %", self.zoom.get() * 100.0));
        self.request_render(automation);
        self.push_history();
    }

    pub fn apply_view_command(&self, id: &str, automation: &Rc<RefCell<Automation>>) -> bool {
        match id {
            "view.layout.single" => self.layout.set(PageLayout::Single),
            "view.layout.two_up" => self.layout.set(PageLayout::TwoUp),
            "view.continuous" => {
                self.continuous.set(!self.continuous.get());
                self.update_navigation();
                self.rebuild_pages();
                self.request_render(automation);
                self.push_history();
                return true;
            }
            "view.page_gaps" => {
                self.gaps.set(!self.gaps.get());
                self.rebuild_pages();
                self.request_render(automation);
                self.push_history();
                return true;
            }
            "view.layout.cover" => {
                self.cover.set(!self.cover.get());
                self.rebuild_pages();
                self.request_render(automation);
                self.push_history();
                return true;
            }
            "view.fit_width_scrolling" => {
                self.layout.set(PageLayout::Single);
                self.continuous.set(true);
                self.fit.set(Fit::Width);
                self.zoom_label.set_text("Anchura");
            }
            "view.fit_one_page" | "view.fit_visible" => {
                self.layout.set(PageLayout::Single);
                self.continuous.set(false);
                self.fit.set(Fit::Page);
                self.zoom_label.set_text("Auto");
            }
            _ => return false,
        }
        self.update_navigation();
        self.rebuild_pages();
        self.request_render(automation);
        self.push_history();
        true
    }

    pub fn previous_view(&self, automation: &Rc<RefCell<Automation>>) {
        let index = self.history_index.get();
        if index > 0 {
            self.restore_history(index - 1, automation);
        }
    }

    pub fn next_view(&self, automation: &Rc<RefCell<Automation>>) {
        let index = self.history_index.get();
        if index + 1 < self.history.borrow().len() {
            self.restore_history(index + 1, automation);
        }
    }

    pub fn set_read_mode(&self, enabled: bool) {
        self.bottom_bar.set_visible(!enabled);
    }

    fn state(&self) -> ViewState {
        ViewState {
            page: self.current.get(),
            zoom: self.zoom.get(),
            fit: self.fit.get(),
            layout: self.layout.get(),
            continuous: self.continuous.get(),
            gaps: self.gaps.get(),
            cover: self.cover.get(),
        }
    }

    fn push_history(&self) {
        let state = self.state();
        let mut history = self.history.borrow_mut();
        let index = self.history_index.get();
        if history.get(index) == Some(&state) {
            return;
        }
        history.truncate(index + 1);
        history.push(state);
        self.history_index.set(history.len() - 1);
    }

    fn restore_history(&self, index: usize, automation: &Rc<RefCell<Automation>>) {
        let Some(state) = self.history.borrow().get(index).copied() else { return };
        self.history_index.set(index);
        self.current.set(state.page.min(self.page_count().saturating_sub(1)));
        self.zoom.set(state.zoom);
        self.fit.set(state.fit);
        self.layout.set(state.layout);
        self.continuous.set(state.continuous);
        self.gaps.set(state.gaps);
        self.cover.set(state.cover);
        let zoom_text = match state.fit {
            Fit::None => format!("{:.0} %", state.zoom * 100.0),
            Fit::Page => "Auto".to_string(),
            Fit::Width => "Anchura".to_string(),
        };
        self.zoom_label.set_text(&zoom_text);
        self.update_navigation();
        self.rebuild_pages();
        self.request_render(automation);
    }

    fn update_navigation(&self) {
        let count = self.info.borrow().pages.len();
        let current = self.current.get();
        self.page_label.set_text(&format!("Página {} de {}", current + 1, count));
        self.prev_button.set_sensitive(current > 0 || self.history_index.get() > 0);
        self.next_button.set_sensitive(current + 1 < count);
    }

    fn scale(&self, page: usize) -> f32 {
        let info = self.info.borrow();
        let Some(page) = info.pages.get(page) else { return 1.0 };
        match self.fit.get() {
            Fit::None => (self.zoom.get() * POINTS_TO_PIXELS) as f32,
            Fit::Page => {
                let available_w = (self.scroll.width() - 56).max(160) as f64;
                let available_h = (self.scroll.height() - 56).max(160) as f64;
                (available_w / f64::from(page.width.max(1.0))).min(available_h / f64::from(page.height.max(1.0))).clamp(0.1, 8.0) as f32
            }
            Fit::Width => {
                let available_w = (self.scroll.width() - 56).max(160) as f64;
                (available_w / f64::from(page.width.max(1.0))).clamp(0.1, 8.0) as f32
            }
        }
    }

    fn rebuild_pages(&self) {
        while let Some(child) = self.page_box.first_child() {
            self.page_box.remove(&child);
        }
        let mut pictures = self.pictures.borrow_mut();
        pictures.clear();
        self.rendered.borrow_mut().clear();
        self.page_box.set_spacing(if self.gaps.get() { 18 } else { 0 });
        let count = self.page_count();
        if count == 0 {
            return;
        }
        let two_up = self.layout.get() == PageLayout::TwoUp;
        let pages: Vec<usize> = if self.continuous.get() {
            (0..count).collect()
        } else if two_up && self.current.get() + 1 < count {
            vec![self.current.get(), self.current.get() + 1]
        } else {
            vec![self.current.get()]
        };
        let mut index = 0;
        while index < pages.len() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, if self.gaps.get() { 18 } else { 0 });
            row.set_halign(gtk::Align::Center);
            let first = pages[index];
            row.append(&self.make_picture(first, &mut pictures));
            index += 1;
            let cover_single = self.cover.get() && first == 0;
            if two_up && index < pages.len() && !cover_single {
                row.append(&self.make_picture(pages[index], &mut pictures));
                index += 1;
            }
            self.page_box.append(&row);
            if !self.continuous.get() {
                break;
            }
        }
    }

    fn make_picture(&self, page: usize, pictures: &mut Vec<(usize, gtk::Picture)>) -> gtk::Picture {
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        picture.add_css_class("pdf-page");
        picture.set_halign(gtk::Align::Center);
        picture.set_valign(gtk::Align::Start);
        self.install_annotation_gestures(&picture, page);
        pictures.push((page, picture.clone()));
        picture
    }

    pub fn request_render(&self, automation: &Rc<RefCell<Automation>>) {
        if self.info.borrow().pages.is_empty() {
            return;
        }
        let tag = self.tag.get().wrapping_add(1);
        self.tag.set(tag);
        self.rendered.borrow_mut().clear();
        self.spinner.set_visible(true);
        let requests: Vec<RenderRequest> = self
            .pictures
            .borrow()
            .iter()
            .map(|(page, _)| RenderRequest { page: *page, scale: self.scale(*page), tag, ..Default::default() })
            .collect();
        let automation = automation.borrow();
        if let Some(document) = automation.session().get(self.id) {
            document.renderer.set_queue(requests);
        }
    }

    pub fn poll(&self, automation: &Rc<RefCell<Automation>>) -> bool {
        let mut updated = false;
        for _ in 0..32 {
            let result = {
                let automation = automation.borrow();
                automation.session().get(self.id).and_then(|document| document.renderer.try_recv())
            };
            let Some(output) = result else { break };
            if output.request.tag != self.tag.get() {
                continue;
            }
            let picture = self.pictures.borrow().iter().find(|(page, _)| *page == output.request.page).map(|(_, picture)| picture.clone());
            let Some(picture) = picture else { continue };
            if let Some(error) = output.error {
                eprintln!("GTK render error: {error}");
                continue;
            }
            let Ok(width) = i32::try_from(output.width) else { continue };
            let Ok(height) = i32::try_from(output.height) else { continue };
            if width <= 0 || height <= 0 {
                continue;
            }
            let bytes = glib::Bytes::from(out_bytes(&output));
            let texture = gtk::gdk::MemoryTexture::new(width, height, gtk::gdk::MemoryFormat::R8g8b8a8, &bytes, output.width as usize * 4);
            picture.set_paintable(Some(&texture));
            picture.set_size_request(width, height);
            self.rendered.borrow_mut().insert(output.request.page, output.request.tag);
            let complete = self.pictures.borrow().iter().all(|(page, _)| self.rendered.borrow().get(page) == Some(&self.tag.get()));
            self.spinner.set_visible(!complete);
            updated = true;
        }
        updated
    }

    pub fn current_texture(&self) -> Option<gtk::gdk::Texture> {
        let current = self.current.get();
        self.pictures
            .borrow()
            .iter()
            .find(|(page, _)| *page == current)
            .and_then(|(_, picture)| picture.paintable())
            .and_then(|paintable| paintable.downcast::<gtk::gdk::Texture>().ok())
    }
}

fn out_bytes(output: &pdfcraft_engine::RenderedPage) -> &[u8] {
    output.rgba.as_ref()
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button
}
