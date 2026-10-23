//! Edit ▸ Cut, Copy and Paste of added content (Edit a PDF ▸ Add content: text boxes and
//! images), from ⌘X / ⌘C / ⌘V and the Edit menu (#2636). A copy
//! keeps everything the item is: a text box its text, font, size, colour, alignment and box; an
//! image its picture, box, turns, flips and crop. It pastes into any open document. The copy also
//! goes on the system clipboard (the text, or the pixels) for other apps, and something newer
//! there wins over it on ⌘V: a picture copied since is pasted instead, and anything else copied
//! since leaves ⌘V alone.

use std::sync::Arc;

use pdfcraft_engine::{AddedContent, Edit};

use crate::create_ui::Clip;
use crate::{PdfCraftApp, QuickTool};

/// Pixels compared to tell whether the system clipboard still holds a copied picture.
const PICTURE_SAMPLES: u64 = 64;

/// How far a compared pixel's channel may drift through the system clipboard (some platforms
/// store the picture in a format of their own and hand back slightly different values).
const PICTURE_TOLERANCE: u8 = 8;

/// A copied added item, ready to paste into any open document.
#[derive(Clone, Debug)]
pub struct ContentClip {
    pub content: AddedContent,
    /// A copied image's picture as a file (name, bytes): what pastes it into another document.
    file: Option<(String, Arc<Vec<u8>>)>,
    /// What the copy put on the system clipboard.
    mark: Mark,
}

/// What a copy put on the system clipboard, to tell on paste whether it is still there.
#[derive(Clone, Debug, PartialEq)]
enum Mark {
    Text(String),
    /// The picture's size and some of its pixels.
    Picture {
        width: usize,
        height: usize,
        samples: Vec<[u8; 4]>,
    },
    /// Nothing could be put there; it held this (`None`: nothing PdfCraft can paste). The copy
    /// stands until something else is copied.
    Unwritten(Option<Box<Mark>>),
}

impl Mark {
    /// A picture of `width` × `height` RGBA pixels, by its size and a sample of them.
    fn picture(width: usize, height: usize, rgba: &[u8]) -> Mark {
        Mark::Picture { width, height, samples: samples(width, height, rgba) }
    }

    /// What the system clipboard holds now, to compare a later paste against.
    fn of(clip: &Clip) -> Mark {
        match clip {
            Clip::Text(t) => Mark::Text(t.clone()),
            Clip::Image { width, height, rgba } => Mark::picture(*width, *height, rgba),
        }
    }

    /// Whether the system clipboard still holds this copy: `system` is what it holds now (`None`
    /// when nothing PdfCraft can paste is there), and `asked` whether there is one to ask (the
    /// web build and tests without `clipboard_override` have none, and the copy stands).
    fn still_there(&self, system: Option<&Clip>, asked: bool) -> bool {
        if !asked {
            return true;
        }
        match (self, system) {
            (Mark::Unwritten(before), _) => match before {
                Some(before) => before.still_there(system, asked),
                None => system.is_none(),
            },
            (Mark::Text(copied), Some(Clip::Text(now))) => same_text(copied, now),
            (Mark::Picture { width, height, samples: copied }, Some(Clip::Image { width: w, height: h, rgba })) => {
                width == w && height == h && same_samples(copied, &samples(*w, *h, rgba))
            }
            _ => false,
        }
    }
}

/// Text as the system clipboard hands it back: line ends and trailing whitespace may change.
fn same_text(a: &str, b: &str) -> bool {
    let plain = |s: &str| s.replace("\r\n", "\n").trim_end().to_string();
    plain(a) == plain(b)
}

/// [`PICTURE_SAMPLES`] pixels spread over the picture (fewer for a tiny one); a picture whose
/// pixels don't match its size has none.
fn samples(width: usize, height: usize, rgba: &[u8]) -> Vec<[u8; 4]> {
    let pixels = (width as u64).saturating_mul(height as u64);
    if pixels == 0 || rgba.len() as u64 != pixels.saturating_mul(4) {
        return Vec::new();
    }
    let n = PICTURE_SAMPLES.min(pixels);
    (0..n)
        .filter_map(|k| {
            // k < n <= pixels: the index stays below pixels, whose bytes the length check bounds.
            let at = usize::try_from(k.saturating_mul(pixels) / n).ok()?.checked_mul(4)?;
            rgba.get(at..at.checked_add(4)?)?.try_into().ok()
        })
        .collect()
}

/// The same pixels, within the drift a system clipboard adds. The colour of a pixel that is not
/// opaque is not compared: some platforms premultiply it by its alpha on the way.
fn same_samples(a: &[[u8; 4]], b: &[[u8; 4]]) -> bool {
    let near = |x: u8, y: u8| x.abs_diff(y) <= PICTURE_TOLERANCE;
    !a.is_empty()
        && a.len() == b.len()
        && a.iter().zip(b).all(|(p, q)| near(p[3], q[3]) && (p[3] < 250 || q[3] < 250 || (0..3).all(|c| near(p[c], q[c]))))
}

/// What ⌘V or Paste takes from the clipboards.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    /// The copied item.
    Copied,
    /// The system clipboard's picture (copied since, or in another app).
    Picture,
    /// Nothing: text copied since, which only a text field takes.
    Nothing,
}

impl PdfCraftApp {
    /// Edit a PDF: added text and images can be selected, moved, edited, copied and pasted (the
    /// Edit panel is open, or a tool that edits page content is picked).
    pub(crate) fn editing_content(&self) -> bool {
        (self.left_open && self.left == crate::LeftPanel::Tool("edit")) || matches!(self.quick_tool, QuickTool::AddText | QuickTool::EditText)
    }

    /// The added item Cut and Copy act on: the selected one, unless its text is being typed or
    /// page text was selected since (selecting an item drops the text selection).
    pub(crate) fn selected_content(&self) -> Option<(usize, usize)> {
        let view = self.active.and_then(|i| self.views.get(i))?;
        if !self.editing_content() || view.organize || view.content.draft.is_some() || view.selected_text().is_some_and(|t| !t.is_empty()) {
            return None;
        }
        view.content.selected
    }

    /// Whether Paste has a copied item to paste into the active document.
    pub(crate) fn can_paste_content(&self) -> bool {
        self.content_clipboard.is_some() && self.active.and_then(|i| self.views.get(i)).is_some_and(|v| !v.organize)
    }

    /// Edit ▸ Copy (and Cut, with `cut`) of added item `index` on `page` of the active document:
    /// keep it for Paste and put its text or pixels on the system clipboard. Cut then takes it off
    /// the page, as one undo step, only once the copy worked.
    pub(crate) fn copy_content(&mut self, ctx: &egui::Context, page: usize, index: usize, cut: bool) -> bool {
        let Some((i, id)) = self.active_ids() else { return false };
        if !self.may_copy(id) {
            return false;
        }
        let Some(doc) = self.session.get(id) else { return false };
        let Some(content) = doc.added().iter().filter(|a| a.page == page).nth(index).map(|a| a.content.clone()) else { return false };
        let file = match &content {
            AddedContent::Text(_) => None,
            AddedContent::Image(_) => match doc.added_image_file(page, index) {
                Ok((ext, bytes)) => Some((format!("Pasted image.{ext}"), Arc::new(bytes))),
                Err(e) => {
                    self.notify_fmt("Couldn't copy the image: {e}", &[("e", &e)]);
                    return false;
                }
            },
        };
        self.keep_copy(ctx, content, file);
        if !cut {
            return true;
        }
        if let Some(view) = self.views.get_mut(i) {
            view.content.selected = None;
        }
        self.apply_edit(Edit::DeleteContent { page, index })
    }

    /// Keep `content` (with an image's picture `file`) for Paste, and put its text or pixels on
    /// the system clipboard, marking what went there.
    fn keep_copy(&mut self, ctx: &egui::Context, content: AddedContent, file: Option<(String, Arc<Vec<u8>>)>) {
        let mark = match (&content, &file) {
            (AddedContent::Text(t), _) => {
                ctx.copy_text(t.text.clone());
                Mark::Text(t.text.clone())
            }
            (AddedContent::Image(_), Some((_, bytes))) => self.put_picture(bytes),
            (AddedContent::Image(_), None) => self.unwritten(),
        };
        self.content_clipboard = Some(ContentClip { content, file, mark });
    }

    /// A copied picture's pixels on the system clipboard, for other apps. A picture too large for
    /// it is kept for pasting in PdfCraft only.
    fn put_picture(&mut self, bytes: &[u8]) -> Mark {
        // No system clipboard to write or read back (the web, tests): the pixels aren't needed.
        if !self.asks_system_clipboard() {
            return Mark::Unwritten(None);
        }
        let Ok((w, h, rgba)) = pdfcraft_engine::image_rgba(bytes) else { return self.unwritten() };
        let (w, h) = (w as usize, h as usize);
        #[cfg(not(target_arch = "wasm32"))]
        if self.system_clipboard {
            let img = arboard::ImageData { width: w, height: h, bytes: std::borrow::Cow::Borrowed(&rgba) };
            if arboard::Clipboard::new().and_then(|mut c| c.set_image(img)).is_err() {
                return self.unwritten();
            }
        }
        Mark::picture(w, h, &rgba)
    }

    /// The mark of a copy that could not be put on the system clipboard: what it holds now, so
    /// that anything copied later still wins.
    fn unwritten(&self) -> Mark {
        let before = if self.asks_system_clipboard() { self.system_clip() } else { None };
        Mark::Unwritten(before.as_ref().map(|c| Box::new(Mark::of(c))))
    }

    /// Whether there is a system clipboard to read (tests set `clipboard_override` instead).
    fn asks_system_clipboard(&self) -> bool {
        self.clipboard_override.is_some() || cfg!(not(target_arch = "wasm32")) && self.system_clipboard
    }

    /// What the system clipboard holds (tests set `clipboard_override` instead).
    pub(crate) fn system_clip(&self) -> Option<Clip> {
        match self.clipboard_override.clone() {
            Some(c) => Some(c),
            #[cfg(not(target_arch = "wasm32"))]
            None if self.system_clipboard => crate::create_ui::read_clipboard(),
            None => None,
        }
    }

    /// Which clipboard ⌘V takes from, given what the system clipboard holds now.
    fn paste_source(&self, system: Option<&Clip>) -> Source {
        let asked = self.asks_system_clipboard();
        match (&self.content_clipboard, system) {
            (Some(clip), _) if clip.mark.still_there(system, asked) => Source::Copied,
            (_, Some(Clip::Image { .. })) => Source::Picture,
            _ => Source::Nothing,
        }
    }

    /// ⌘V: the copied item onto the current page while the system clipboard still holds it; else
    /// `false`, and a picture there is pasted as one.
    pub(crate) fn paste_copied_key(&mut self, system: Option<&Clip>) -> bool {
        let Some(page) = self.active.and_then(|i| self.views.get(i)).map(|v| v.current) else { return false };
        if self.paste_source(system) != Source::Copied {
            return false;
        }
        self.paste_content(page, None);
        true
    }

    /// Edit ▸ Paste onto `page` (at `at`, display space, when given): a picture copied since on
    /// the system clipboard, else the copied item.
    /// Unlike ⌘V, text copied since doesn't stop it: Paste is only offered with an item copied.
    pub(crate) fn paste_from_menu(&mut self, page: usize, at: Option<[f64; 2]>) -> bool {
        let system = self.system_clip();
        match (self.paste_source(system.as_ref()), system) {
            (Source::Picture, Some(Clip::Image { width, height, rgba })) => self.paste_picture(page, at, width, height, &rgba),
            _ => self.paste_content(page, at),
        }
    }

    /// A system clipboard picture onto `page`: at its own size (shrunk to fit), with its top-left
    /// corner at `at`, else in the middle of the page. Selected, as Add Image leaves it.
    fn paste_picture(&mut self, page: usize, at: Option<[f64; 2]>, width: usize, height: usize, rgba: &[u8]) -> bool {
        let png = match crate::create_ui::png_from_rgba(width, height, rgba) {
            Ok(png) => png,
            Err(e) => {
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", tl!("Clipboard")), ("e", &e)]);
                return false;
            }
        };
        let Some((_, id)) = self.active_ids() else { return false };
        let size = self.session.get(id).and_then(|d| d.info.pages.get(page)).map(|p| [f64::from(p.width), f64::from(p.height)]);
        // A picture from the clipboard has no resolution: a pixel is a point (as Add Image places it).
        let rect = at.zip(size).map(|(at, size)| {
            let (w, h) = (width as f64, height as f64);
            let k = (size[0] * 0.8 / w).min(size[1] * 0.8 / h).min(1.0);
            pdfcraft_engine::added_paste_rect([0.0, 0.0, w * k, h * k], size, 0, Some(at))
        });
        self.add_image_on(page, rect, "Pasted image.png".into(), png)
    }

    /// The copied item onto `page` of the active document, selected: with its top-left corner at
    /// `at` (display space), else where it was on its page, stepped down and to the right past
    /// copies of it already there. One undo step.
    pub(crate) fn paste_content(&mut self, page: usize, at: Option<[f64; 2]>) -> bool {
        let Some((i, id)) = self.active_ids() else { return false };
        let (Some(clip), Some(doc)) = (self.content_clipboard.as_ref(), self.session.get(id)) else { return false };
        let Some(size) = doc.info.pages.get(page).map(|p| [f64::from(p.width), f64::from(p.height)]) else { return false };
        let on_page = || doc.added().iter().filter(|a| a.page == page);
        let rect = match at {
            Some(at) => pdfcraft_engine::added_paste_rect(clip.content.rect(), size, 0, Some(at)),
            None => pdfcraft_engine::added_paste_place(on_page().map(|a| &a.content), &clip.content, size),
        };
        let before = on_page().count();
        let edit = Edit::PasteContent { page, content: clip.content.with_rect(rect), file: clip.file.clone() };
        if !self.apply_edit(edit) {
            return false;
        }
        if let Some(view) = self.views.get_mut(i) {
            view.content.selected = None;
            view.content.select_added = Some((page, before));
        }
        self.left = crate::LeftPanel::Tool("edit");
        self.left_open = true;
        true
    }

    /// ⌘C / ⌘X on the selected added item, as Edit ▸ Copy and Cut (which say why when the
    /// document doesn't allow it). `true` when the keys were its.
    pub(crate) fn content_clip_keys(&mut self, i: usize, ctx: &egui::Context) -> bool {
        let (copy, cut) =
            ctx.input(|inp| (inp.events.iter().any(|e| matches!(e, egui::Event::Copy)), inp.events.iter().any(|e| matches!(e, egui::Event::Cut))));
        let free = self.dialog.is_none() && !self.palette_open && !self.is_preparing() && self.active == Some(i) && !ctx.egui_wants_keyboard_input();
        if !(copy || cut) || !free || self.views.get(i).is_none_or(|v| v.forms.focus.is_some()) || self.selected_content().is_none() {
            return false;
        }
        self.execute(if cut { "edit.cut" } else { "edit.copy" });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clipboard_still_holds_a_copy_until_something_else_is_copied() {
        let there = |m: &Mark, c: Option<Clip>| m.still_there(c.as_ref(), true);
        let image = |w, h, rgba: &[u8]| Some(Clip::Image { width: w, height: h, rgba: rgba.to_vec() });
        let text = Mark::Text("Line one\nLine two".into());
        assert!(text.still_there(None, false), "no system clipboard to ask: the copy stands");
        assert!(!there(&text, None), "something PdfCraft can't paste (a file) was copied since");
        assert!(there(&text, Some(Clip::Text("Line one\r\nLine two\n".into()))), "line ends changed on the way");
        assert!(!there(&text, Some(Clip::Text("Something else".into()))));
        let rgba: Vec<u8> = (0..30 * 20).flat_map(|i| [(i % 251) as u8, (i % 13) as u8, 90, 255]).collect();
        let picture = Mark::picture(30, 20, &rgba);
        assert!(!there(&text, image(30, 20, &rgba)), "a picture copied since");
        assert!(there(&picture, image(30, 20, &rgba)));
        let drifted: Vec<u8> = rgba.iter().map(|v| v.saturating_add(3)).collect();
        assert!(there(&picture, image(30, 20, &drifted)), "small drift through the clipboard");
        let other: Vec<u8> = rgba.iter().map(|v| 255 - v).collect();
        assert!(!there(&picture, image(30, 20, &other)));
        assert!(!there(&picture, image(20, 30, &rgba)), "another size");
        assert!(!there(&picture, Some(Clip::Text("hello".into()))));
        assert!(!there(&picture, image(30, 20, &[0; 7])), "malformed pixels match nothing");
        // Transparent pixels compare by alpha alone: their colour may come back premultiplied.
        let clear = [200u8, 100, 50, 128].repeat(4);
        let premultiplied = [100u8, 50, 25, 128].repeat(4);
        assert!(there(&Mark::picture(2, 2, &clear), image(2, 2, &premultiplied)));
        // A copy that couldn't be put there stands while the clipboard holds what it held then.
        let before = Mark::Unwritten(Some(Box::new(Mark::Text("older".into()))));
        assert!(there(&before, Some(Clip::Text("older".into()))));
        assert!(!there(&before, image(30, 20, &rgba)), "a screenshot taken since wins");
        let empty = Mark::Unwritten(None);
        assert!(there(&empty, None));
        assert!(!there(&empty, Some(Clip::Text("copied since".into()))));
    }

    #[test]
    fn samples_never_read_past_the_pixels() {
        assert!(samples(0, 5, &[]).is_empty());
        assert!(samples(3, 3, &[0; 35]).is_empty(), "a short buffer has no samples");
        assert_eq!(samples(1, 1, &[1, 2, 3, 4]), vec![[1, 2, 3, 4]]);
        assert_eq!(samples(1000, 1000, &vec![7; 4_000_000]).len(), PICTURE_SAMPLES as usize);
        assert!(samples(usize::MAX, 2, &[0; 8]).is_empty(), "a size that overflows matches no buffer");
    }
}
