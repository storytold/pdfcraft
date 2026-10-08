//! Link tools: list, add, edit and delete links; create links from URLs in the text; remove
//! all links. Rectangles are points from the top-left of the displayed page.

use pdfcraft_engine::{Edit, LinkAction, LinkHighlight, LinkStyle};
use serde_json::{Value, json};

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
        let items: Vec<Value> = doc
            .links
            .iter()
            .map(|l| {
                let p = &doc.info.pages[l.page];
                let (v0, v1) = (p.user_to_view(l.rect[0] as f32, l.rect[1] as f32), p.user_to_view(l.rect[2] as f32, l.rect[3] as f32));
                let rect = [v0[0].min(v1[0]), v0[1].min(v1[1]), v0[0].max(v1[0]), v0[1].max(v1[1])].map(|x| (x * 100.0).round() / 100.0);
                let mut v = json!({ "page": l.page + 1, "index": l.index + 1, "rect": rect, "visible": l.style.visible });
                match &l.action {
                    LinkAction::Page(p) => v["to_page"] = json!(p + 1),
                    LinkAction::Uri(u) => v["url"] = json!(u),
                    LinkAction::Other(o) => v["other"] = json!(o),
                }
                v
            })
            .collect();
        Ok(json!({ "count": items.len(), "links": items }))
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
            .links
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
        let before = self.doc(a)?.links.len();
        let mut out = self.apply(a, Edit::RemoveLinks { pages: None })?;
        out["removed"] = json!(before);
        Ok(out)
    }
}
