//! Tags panel and Reading Order tool, headless: structural tag edits, and tagging page content
//! by unit or rectangle. Rectangles and points are top-left-origin points on the displayed page,
//! like every other tool's rectangles.

use std::collections::HashSet;

use pdfcraft_cos::ObjRef;
use pdfcraft_engine::a11y::{ContentUnit, Destination, Selection, UnitKind, UnitMark};
use pdfcraft_engine::{Document, Edit};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed};

/// A user-space box as top-left-origin points on the displayed page.
fn view_rect(doc: &Document, page: usize, b: [f64; 4]) -> Option<[f64; 4]> {
    let info = doc.info.pages.get(page)?;
    let (u, v) = (info.user_to_view(b[0] as f32, b[1] as f32), info.user_to_view(b[2] as f32, b[3] as f32));
    let r = |x: f32| (f64::from(x) * 100.0).round() / 100.0;
    Some([r(u[0].min(v[0])), r(u[1].min(v[1])), r(u[0].max(v[0])), r(u[1].max(v[1]))])
}

fn tag_json(r: ObjRef) -> Value {
    json!({ "object": r.num, "generation": r.generation })
}

fn kind_id(k: UnitKind) -> &'static str {
    match k {
        UnitKind::Text => "text",
        UnitKind::Image => "image",
        UnitKind::Graphic => "graphic",
    }
}

/// One unit of page content: its index, kind, state (with its MCID and tag when it has them),
/// rectangle and text.
fn unit_json(doc: &Document, page: usize, u: &ContentUnit) -> Value {
    let (state, mcid, tag) = match u.mark {
        UnitMark::Tagged { mcid, tag } => ("tagged", Some(mcid), Some(tag)),
        UnitMark::Orphan { mcid } => ("orphan", Some(mcid), None),
        UnitMark::Unaddressable { mcid } => ("unaddressable", Some(mcid), None),
        UnitMark::Artifact => ("artifact", None, None),
        UnitMark::Untagged => ("untagged", None, None),
        UnitMark::FormStructure => ("form_structure", None, None),
    };
    json!({
        "unit": u.index, "kind": kind_id(u.kind), "state": state, "mcid": mcid, "tag": tag.map(tag_json),
        "rect": u.bbox.and_then(|b| view_rect(doc, page, b)), "text": u.text, "nested": u.nested,
    })
}

impl Automation {
    fn tag_arg(&self, a: &Args, number: &str, generation: &str) -> Result<ObjRef> {
        let n = u32::try_from(a.int(number)?)
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| ToolError::InvalidArgs(format!("{number} must be a positive tag object number")))?;
        let g = u16::try_from(a.opt_int(generation)?.unwrap_or(0))
            .map_err(|_| ToolError::InvalidArgs(format!("{generation} must be a PDF object generation from accessibility_tags")))?;
        Ok(ObjRef::new(n, g))
    }

    /// `units` (indices from accessibility_page_content) or `rect` (view points), exactly one.
    fn selection(&self, a: &Args, page: usize) -> Result<Selection> {
        match (a.opt_ints("units")?, a.nums::<4>("rect")?) {
            (Some(units), None) => Ok(Selection::Units(
                units
                    .into_iter()
                    .map(|u| usize::try_from(u).map_err(|_| ToolError::InvalidArgs("units must be indices from accessibility_page_content".into())))
                    .collect::<Result<_>>()?,
            )),
            (None, Some(r)) => {
                let info = self.doc(a)?.info.pages.get(page).ok_or_else(|| failed("the page is not available"))?;
                let (u, v) = (info.view_to_user(r[0] as f32, r[1] as f32), info.view_to_user(r[2] as f32, r[3] as f32));
                Ok(Selection::Rect([f64::from(u[0]), f64::from(u[1]), f64::from(v[0]), f64::from(v[1])]))
            }
            _ => Err(ToolError::InvalidArgs("give either units or rect".into())),
        }
    }

    /// Where a tag goes: `placement` before, after or into the `anchor` tag, or root; `index`
    /// is the child position for into and root (default: last). `None` without a placement.
    fn destination(&self, a: &Args) -> Result<Option<Destination>> {
        let Some(placement) = a.opt_str("placement")? else { return Ok(None) };
        let index =
            a.opt_int("index")?.map(|i| usize::try_from(i).map_err(|_| ToolError::InvalidArgs("index must be 0 or more".into()))).transpose()?;
        let anchor = || self.tag_arg(a, "anchor", "anchor_generation");
        Ok(Some(match (placement, index) {
            ("before", _) => Destination::Before(anchor()?),
            ("after", _) => Destination::After(anchor()?),
            ("into", Some(i)) => Destination::At(Some(anchor()?), i),
            ("into", None) => Destination::Into(Some(anchor()?)),
            ("root", Some(i)) => Destination::At(None, i),
            ("root", None) => Destination::Into(None),
            (other, _) => return Err(ToolError::InvalidArgs(format!("placement must be before, after, into or root, not {other:?}"))),
        }))
    }

    /// Apply a tag edit and report the tags it created.
    fn apply_creating(&mut self, a: &Args, edit: Edit) -> Result<Value> {
        let before: HashSet<ObjRef> = self.doc(a)?.tags().into_iter().map(|t| t.object).collect();
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        let created: Vec<Value> = self
            .doc(a)?
            .tags()
            .into_iter()
            .filter(|t| !before.contains(&t.object))
            .map(|t| json!({ "object": t.object.num, "generation": t.object.generation, "type": t.kind, "depth": t.depth, "page": t.page.map(|p| p + 1) }))
            .collect();
        Ok(json!({ "created": created }))
    }

    pub(crate) fn accessibility_page_content(&self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let doc = self.doc(a)?;
        let listing = doc.page_tagging(page).map_err(failed)?;
        let only_untagged = a.opt_bool("only_untagged")?.unwrap_or(false);
        let units: Vec<Value> = listing.units.iter().filter(|u| !only_untagged || u.is_untagged()).map(|u| unit_json(doc, page, u)).collect();
        let annotations: Vec<Value> = listing
            .annotations
            .iter()
            .filter(|n| !only_untagged || n.tag.is_none())
            .map(|n| {
                json!({
                    "object": n.object.num, "generation": n.object.generation, "subtype": n.subtype,
                    "rect": n.rect.and_then(|b| view_rect(doc, page, b)), "tag": n.tag.map(tag_json),
                })
            })
            .collect();
        Ok(json!({ "page": page + 1, "units": units, "annotations": annotations, "read_only": listing.read_only }))
    }

    pub(crate) fn accessibility_reading_order(&self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let doc = self.doc(a)?;
        let listing = doc.page_tagging(page).map_err(failed)?;
        let regions: Vec<Value> = listing
            .regions
            .iter()
            .map(|g| {
                json!({
                    "order": g.order, "tag": tag_json(g.tag), "type": g.kind, "rect": g.bbox.and_then(|b| view_rect(doc, page, b)),
                    "units": g.units, "annotations": g.annotations.iter().copied().map(tag_json).collect::<Vec<_>>(), "text": g.text,
                })
            })
            .collect();
        let untagged = listing.units.iter().filter(|u| u.is_untagged()).count() + listing.annotations.iter().filter(|n| n.tag.is_none()).count();
        Ok(json!({ "page": page + 1, "regions": regions, "untagged": untagged }))
    }

    pub(crate) fn accessibility_create_tag(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let selection = self.selection(a, page)?;
        let edit = Edit::TagContent {
            page,
            selection,
            kind: a.str("type")?.to_owned(),
            alt: a.opt_str("alt")?.map(str::to_owned),
            destination: self.destination(a)?,
        };
        let created = self.apply_creating(a, edit)?;
        let order = self.accessibility_reading_order(a)?;
        Ok(
            json!({ "created": created.get("created").cloned().unwrap_or(Value::Null), "reading_order": order.get("regions").cloned().unwrap_or(Value::Null) }),
        )
    }

    pub(crate) fn accessibility_mark_content_artifact(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let selection = self.selection(a, page)?;
        let id = self.doc(a)?.id;
        self.session.apply(id, Edit::ContentToArtifact { page, selection }).map_err(failed)?;
        self.accessibility_reading_order(a)
    }

    pub(crate) fn accessibility_new_tag(&mut self, a: &Args) -> Result<Value> {
        let destination = match self.destination(a)? {
            Some(d) => d,
            // Into the document's one top-level tag, as the Tags panel does with nothing selected.
            None => {
                let tops: Vec<ObjRef> = self.doc(a)?.tags().into_iter().filter(|t| t.depth == 0 && t.parent.is_none()).map(|t| t.object).collect();
                Destination::Into(match tops.as_slice() {
                    [only] => Some(*only),
                    _ => None,
                })
            }
        };
        self.apply_creating(a, Edit::NewTag { kind: a.str("type")?.to_owned(), destination })
    }

    pub(crate) fn accessibility_delete_tag(&mut self, a: &Args) -> Result<Value> {
        let object = self.tag_arg(a, "object", "generation")?;
        let keep_contents = a.opt_bool("keep_contents")?.unwrap_or(false);
        let id = self.doc(a)?.id;
        self.session.apply(id, Edit::DeleteTag { object, keep_contents }).map_err(failed)?;
        self.accessibility_tags(a)
    }

    pub(crate) fn accessibility_move_tag_to(&mut self, a: &Args) -> Result<Value> {
        let object = self.tag_arg(a, "object", "generation")?;
        let destination = self.destination(a)?.ok_or_else(|| ToolError::InvalidArgs("placement is required".into()))?;
        let id = self.doc(a)?.id;
        self.session.apply(id, Edit::MoveTagTo { object, destination }).map_err(failed)?;
        self.accessibility_tags(a)
    }

    pub(crate) fn accessibility_tag_to_artifact(&mut self, a: &Args) -> Result<Value> {
        let object = self.tag_arg(a, "object", "generation")?;
        let id = self.doc(a)?.id;
        self.session.apply(id, Edit::TagToArtifact { object }).map_err(failed)?;
        self.accessibility_tags(a)
    }

    pub(crate) fn accessibility_tag_annotation(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let annotation = self.tag_arg(a, "annotation", "annotation_generation")?;
        self.apply_creating(a, Edit::TagAnnotation { page, annotation })
    }

    pub(crate) fn accessibility_tag_at(&self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let doc = self.doc(a)?;
        let (x, y) = (
            a.opt_num("x")?.ok_or_else(|| ToolError::InvalidArgs("x is required".into()))?,
            a.opt_num("y")?.ok_or_else(|| ToolError::InvalidArgs("y is required".into()))?,
        );
        let info = doc.info.pages.get(page).ok_or_else(|| failed("the page is not available"))?;
        let [ux, uy] = info.view_to_user(x as f32, y as f32);
        let tag = doc.tag_at(page, f64::from(ux), f64::from(uy));
        let tags = doc.tags();
        let info = tag.and_then(|t| tags.iter().find(|x| x.object == t));
        Ok(json!({ "tag": tag.map(tag_json), "type": info.map(|t| t.kind.clone()), "title": info.and_then(|t| t.title.clone()) }))
    }

    pub(crate) fn accessibility_tag_boxes(&self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let object = self.tag_arg(a, "object", "generation")?;
        let doc = self.doc(a)?;
        let rects: Vec<[f64; 4]> = doc.tag_boxes(object, page).into_iter().filter_map(|b| view_rect(doc, page, b)).collect();
        Ok(json!({ "page": page + 1, "rects": rects }))
    }
}
