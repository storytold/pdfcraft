//! Link tools: list, add, edit and delete links; create links from URLs in the text; remove
//! all links. Rectangles are points from the top-left of the displayed page.

use pdfcraft_engine::{
    Edit, LinkAction, LinkHighlight, LinkStyle, read_remote_document, remote_document_path_is_absolute, resolve_remote_document_path,
};
use pdfcraft_render::{DestView, LinkTarget, RemoteDestination};
use serde_json::{Value, json};
use std::path::Path;

use crate::comments::parse_color;
use crate::{Args, Automation, Result, ToolError, failed};

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

impl Automation {
    fn view_rect(&self, a: &Args, page: usize) -> Result<Option<[f64; 4]>> {
        let Some(v) = a.get("rect") else { return Ok(None) };
        let r: Vec<f64> = v.as_array().map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let r = <[f64; 4]>::try_from(r).map_err(|_| bad("rect must be 4 numbers"))?;
        let p = &self.doc(a)?.info.pages[page];
        let (u0, u1) = (p.view_to_user(r[0] as f32, r[1] as f32), p.view_to_user(r[2] as f32, r[3] as f32));
        Ok(Some([u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64]))
    }

    fn link_action(&self, a: &Args) -> Result<Option<LinkAction>> {
        match (a.opt_str("url")?, a.opt_int("to_page")?) {
            (Some(_), Some(_)) => Err(bad("pass url or to_page, not both")),
            (Some(u), None) => Ok(Some(LinkAction::Uri(u.to_string()))),
            (None, Some(p)) => {
                let n = self.doc(a)?.info.pages.len() as i64;
                if p < 1 || p > n {
                    return Err(bad(format!("to_page must be 1–{n}")));
                }
                Ok(Some(LinkAction::Page(p as usize - 1)))
            }
            (None, None) => Ok(None),
        }
    }

    fn link_style(&self, a: &Args, base: LinkStyle) -> Result<LinkStyle> {
        let mut s = base;
        if let Some(v) = a.opt_bool("visible")? {
            s.visible = v;
        }
        if let Some(c) = a.opt_str("color")? {
            s.color = parse_color(c)?;
        }
        if let Some(w) = a.opt_num("width")? {
            s.width = w.clamp(1.0, 3.0);
        }
        if let Some(h) = a.opt_str("highlight")? {
            s.highlight = match h {
                "none" => LinkHighlight::None,
                "invert" => LinkHighlight::Invert,
                "outline" => LinkHighlight::Outline,
                "inset" => LinkHighlight::Inset,
                o => return Err(bad(format!("unknown highlight {o:?}"))),
            };
        }
        Ok(s)
    }

    pub(crate) fn link_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let targets: std::collections::HashMap<(usize, usize), &LinkTarget> = doc.info.links.iter().map(|t| ((t.page, t.index), &t.target)).collect();
        let items: Vec<Value> = doc
            .links()
            .iter()
            .map(|l| {
                // The link's page may be one the viewer could not read (#2660): rect unchanged then.
                let rect = crate::view_rect(&doc.info, l.page, l.rect.map(|x| x as f32));
                let mut v = json!({ "page": l.page + 1, "index": l.index + 1, "rect": rect, "visible": l.style.visible });
                // Where an in-document link goes, as the viewer resolves it: named destinations
                // included, with the position on the page.
                let target = targets.get(&(l.page, l.index)).and_then(|t| match t {
                    LinkTarget::Page(page, view) => Some((*page, view)),
                    _ => None,
                });
                match (&l.action, target) {
                    (LinkAction::Uri(u), _) => v["url"] = json!(u),
                    (action, Some((page, view))) => {
                        v["to_page"] = json!(page + 1);
                        if let Some(d) = doc.info.pages.get(page).and_then(|p| crate::destination(view, p)) {
                            v["destination"] = d;
                        }
                        if matches!(action, LinkAction::Other(_)) {
                            v["named"] = json!(true);
                        }
                    }
                    (LinkAction::Page(p), None) => v["to_page"] = json!(p + 1),
                    (LinkAction::Other(o), None) => v["other"] = json!(o),
                }
                v
            })
            .collect();
        Ok(json!({ "count": items.len(), "links": items }))
    }

    pub(crate) fn link_open(&mut self, a: &Args) -> Result<Value> {
        let page_number = a.int("page")?;
        let index_number = a.int("index")?;
        let page = usize::try_from(page_number.checked_sub(1).filter(|n| *n >= 0).ok_or_else(|| bad("page must be 1 or more"))?)
            .map_err(|_| bad("page is out of range"))?;
        let index = usize::try_from(index_number.checked_sub(1).filter(|n| *n >= 0).ok_or_else(|| bad("index must be 1 or more"))?)
            .map_err(|_| bad("index is out of range"))?;
        let (source_path, file, destination, new_window) = {
            let source = self.doc(a)?;
            if page >= source.info.pages.len() {
                return Err(bad(format!("page {page_number} is out of range: the document has {} pages", source.info.pages.len())));
            }
            let link = source
                .info
                .links
                .iter()
                .find(|link| link.page == page && link.index == index)
                .ok_or_else(|| failed(format!("page {page_number} has no link {index_number} (see link_list)")))?;
            let LinkTarget::RemoteGoTo { file, destination, new_window } = &link.target else {
                return Err(bad("the selected link is not a supported local PDF GoToR link"));
            };
            (source.path.clone(), file.clone(), destination.clone(), *new_window)
        };
        let path = resolve_remote_document_path(source_path.as_deref().map(Path::new), &file).map_err(failed)?;
        let allow_absolute = a.opt_bool("allow_absolute")?.unwrap_or(false);
        if remote_document_path_is_absolute(&file) && !allow_absolute {
            return Err(failed(
                "this GoToR link names an absolute local path; review the target with link_list and pass allow_absolute: true to open it",
            ));
        }
        let path = self.resolve(&path.to_string_lossy(), false)?;
        let bytes = read_remote_document(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let id =
            self.session.open(name, Some(path.to_string_lossy().into_owned()), std::sync::Arc::new(bytes), a.opt_str("password")?).map_err(failed)?;
        let opened = self.session.get(id).ok_or_else(|| failed("the linked document vanished"))?;
        let (destination_applied, destination_value, warning) = match destination {
            RemoteDestination::Default => (true, json!({ "kind": "default" }), None),
            RemoteDestination::Named(name) => {
                (false, json!({ "kind": "named", "name": name }), Some("The PDF opened, but the named destination was not applied."))
            }
            RemoteDestination::Page { page, view } if page < opened.info.pages.len() => {
                (true, json!({ "kind": "page", "page": page.saturating_add(1), "view": dest_view_value(view) }), None)
            }
            RemoteDestination::Page { page, view } => (
                false,
                json!({ "kind": "page", "page": page.saturating_add(1), "view": dest_view_value(view) }),
                Some("The PDF opened, but the link's page destination is out of range."),
            ),
        };
        let mut display_path = path.to_string_lossy().into_owned();
        if let Some(root) = &self.root
            && let Ok(relative) = path.strip_prefix(root)
        {
            display_path = relative.to_string_lossy().into_owned();
        }
        Ok(json!({
            "document": crate::summary(opened),
            "path": display_path,
            "destination_applied": destination_applied,
            "destination": destination_value,
            "warning": warning,
            "new_window_hint": new_window,
        }))
    }

    pub(crate) fn link_add(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let rect = self.view_rect(a, page)?.ok_or_else(|| bad("link_add needs rect"))?;
        let action = self.link_action(a)?.ok_or_else(|| bad("pass url or to_page"))?;
        let style = self.link_style(a, LinkStyle::default())?;
        self.apply(a, Edit::AddLink { page, rect, action, style })
    }

    fn link_index(&self, a: &Args) -> Result<(usize, usize, LinkStyle)> {
        let page = self.page(a)?;
        let index = a.int("index")?;
        let doc = self.doc(a)?;
        let l = doc
            .links()
            .iter()
            .find(|l| l.page == page && l.index as i64 + 1 == index)
            .ok_or_else(|| failed(format!("page {} has no link {index} (see link_list)", page + 1)))?;
        Ok((page, l.index, l.style))
    }

    pub(crate) fn link_edit(&mut self, a: &Args) -> Result<Value> {
        let (page, index, style) = self.link_index(a)?;
        let rect = self.view_rect(a, page)?;
        let action = self.link_action(a)?;
        let new_style = self.link_style(a, style)?;
        self.apply(a, Edit::SetLink { page, index, rect, action, style: (new_style != style).then_some(new_style) })
    }

    pub(crate) fn link_delete(&mut self, a: &Args) -> Result<Value> {
        let (page, index, _) = self.link_index(a)?;
        self.apply(a, Edit::DeleteLink { page, index })
    }

    pub(crate) fn links_from_urls(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let found = self.session.find_urls(id);
        if found.is_empty() {
            return Err(failed("no unlinked web addresses were found in the text"));
        }
        let urls: Vec<String> = found.iter().map(|f| f.2.clone()).collect();
        let mut out = self.apply(a, Edit::AddLinks { links: found, style: LinkStyle::default() })?;
        out["created"] = json!(urls);
        Ok(out)
    }

    pub(crate) fn links_remove(&mut self, a: &Args) -> Result<Value> {
        let pages = match a.opt_ints("pages")? {
            Some(_) => Some(self.pages(a, "pages")?),
            None => None,
        };
        let before = match &pages {
            Some(ps) => self.doc(a)?.links().iter().filter(|l| ps.contains(&l.page)).count(),
            None => self.doc(a)?.links().len(),
        };
        let mut out = self.apply(a, Edit::RemoveLinks { pages })?;
        out["removed"] = json!(before);
        Ok(out)
    }
}

fn dest_view_value(view: DestView) -> Value {
    match view {
        DestView::Top => json!({ "kind": "top" }),
        DestView::Xyz { left, top, zoom } => json!({ "kind": "xyz", "left": left, "top": top, "zoom": zoom }),
        DestView::Fit => json!({ "kind": "fit" }),
        DestView::FitH { top } => json!({ "kind": "fit_h", "top": top }),
        DestView::FitV { left } => json!({ "kind": "fit_v", "left": left }),
        DestView::FitR { rect } => json!({ "kind": "fit_r", "rect": rect }),
    }
}
