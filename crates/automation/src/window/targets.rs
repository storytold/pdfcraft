//! Mutation proofs for Ask Agent, resolved on the same owner and connection as execution.
use super::WindowTools;
use pdfcraft_engine::{BlockKey, DocId, Session};
use serde_json::{Value, json};

impl WindowTools {
    /// Resolve trusted document identity and actual PDF-user-space ink, without executing a
    /// command. Unknown mutations fail closed only when the app requests an Ask Agent proof.
    pub fn resolve_target(&mut self, connection: u64, session: &Session, name: &str, args: &Value) -> Result<Value, String> {
        self.resolve_target_at(connection, session, name, args, 0)
    }

    fn resolve_target_at(&mut self, connection: u64, session: &Session, name: &str, args: &Value, depth: usize) -> Result<Value, String> {
        let unsupported = || format!("UNPROVEN_TARGET: {name} cannot prove its exact Ask Agent target; keep the current work before using this tool");
        if depth > 8 {
            return Err(unsupported());
        }
        if name == "command_run" {
            let tool = args["id"].as_str().and_then(crate::tools::tool_for_command).ok_or_else(unsupported)?;
            return self.resolve_target_at(connection, session, tool, args.get("params").unwrap_or(&Value::Null), depth + 1);
        }
        // Batches are authorized as a whole before any step executes. A region edit is kept
        // single-step so later paragraph numbers cannot change meaning within the batch.
        if name == "command_batch" {
            let steps = args["steps"].as_array().filter(|s| !s.is_empty() && s.len() <= 128).ok_or_else(unsupported)?;
            let mut target: Option<Value> = None;
            for step in steps {
                let proof = self.resolve_target_at(connection, session, "command_run", step, depth + 1)?;
                if proof["rects"].as_array().is_none_or(|regions| !regions.is_empty()) {
                    return Err(unsupported());
                }
                if let Some(target) = &mut target {
                    if target["doc"] != proof["doc"] {
                        return Err("OUTSIDE_SELECTION: a batch must edit one captured document".into());
                    }
                    let pages = target["pages"].as_array_mut().ok_or_else(unsupported)?;
                    for page in proof["pages"].as_array().ok_or_else(unsupported)? {
                        if !pages.contains(page) {
                            pages.push(page.clone());
                        }
                    }
                } else {
                    target = Some(proof);
                }
            }
            let mut target = target.ok_or_else(unsupported)?;
            // The first step proves scope, not the transaction's history behavior. Even a
            // one-step batch must not inherit the standalone Save exception.
            target["tool"] = json!("command_batch");
            return Ok(target);
        }
        let id = DocId(args["doc"].as_u64().ok_or_else(unsupported)?);
        let document = session.get(id).ok_or("STALE_TARGET: the document was closed")?;
        let page = || {
            args["page"]
                .as_u64()
                .and_then(|p| usize::try_from(p).ok())
                .and_then(|p| p.checked_sub(1))
                .filter(|p| *p < document.info.pages.len())
                .ok_or_else(unsupported)
        };
        let mut regions = Vec::new();
        let mut text_target = Value::Null;
        let pages: Vec<usize> = match name {
            "text_edit" => {
                let object = args.as_object().ok_or_else(unsupported)?;
                if object.keys().any(|key| !matches!(key.as_str(), "doc" | "page" | "paragraph" | "text")) {
                    return Err(unsupported());
                }
                let p = page()?;
                let k = args["paragraph"].as_u64().and_then(|k| usize::try_from(k).ok()).and_then(|k| k.checked_sub(1)).ok_or_else(unsupported)?;
                let state = self
                    .connections
                    .get_mut(&connection)
                    .map(|(_, state)| state)
                    .ok_or("UNPROVEN_TARGET: list text_paragraphs on this connection first")?;
                state.discard_stale(session);
                let keys = state.listed.get(&(id, p)).ok_or("UNPROVEN_TARGET: list text_paragraphs on this connection first")?;
                let blocks = document.text_blocks(p);
                let index = keys
                    .get(k)
                    .and_then(|key: &BlockKey| key.find(&blocks, k))
                    .ok_or("STALE_TARGET: the listed paragraph changed; list text_paragraphs again")?;
                let block = blocks.get(index).ok_or_else(unsupported)?;
                let lines = document.text_lines(p);
                let config = session.render_config(id).ok_or_else(unsupported)?;
                let mut renderer = pdfcraft_render::PageRenderer::new(document.display.clone(), config);
                let rendered =
                    renderer.render(pdfcraft_render::RenderRequest { page: p, kind: pdfcraft_render::RequestKind::Text, ..Default::default() });
                let rendered = rendered.text.ok_or_else(unsupported)?;
                let info = &document.info.pages[p];
                let mut matching_work = 2_000_000_usize;
                // The editor's word boxes include conservative font metrics; the selection
                // uses rendered glyph em quads. Match every source word by text and centre,
                // then prove its actual glyph quads rather than padding the selection.
                for line in &block.lines {
                    let line = lines.get(*line).ok_or_else(unsupported)?;
                    let words: Vec<_> = line.text.split_whitespace().collect();
                    if line.ink.len() != words.len() || words.is_empty() {
                        return Err(unsupported());
                    }
                    matching_work =
                        matching_work.checked_sub(rendered.glyphs.len().checked_mul(words.len()).ok_or_else(unsupported)?).ok_or_else(unsupported)?;
                    for (ink, word) in line.ink.iter().zip(words) {
                        let mut found = String::new();
                        let mut glyphs = Vec::new();
                        for glyph in &rendered.glyphs {
                            let text: String = glyph.text.chars().filter(|c| !c.is_whitespace()).collect();
                            if text.is_empty() {
                                continue;
                            }
                            let quad = glyph.quad.map(|p| info.view_to_user(p[0], p[1]));
                            let x = quad.iter().map(|p| p[0] as f64).sum::<f64>() / 4.0;
                            let y = quad.iter().map(|p| p[1] as f64).sum::<f64>() / 4.0;
                            if x < ink[0] || x > ink[2] || y < ink[1] || y > ink[3] {
                                continue;
                            }
                            found.push_str(&text);
                            let rect = [
                                quad.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min),
                                quad.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min),
                                quad.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max),
                                quad.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max),
                            ];
                            // QuadPoints use Z order; TextGlyph.quad uses polygon order.
                            let quad = [quad[0][0], quad[0][1], quad[1][0], quad[1][1], quad[3][0], quad[3][1], quad[2][0], quad[2][1]];
                            glyphs.push(json!({"page":p+1,"rect":rect,"quad":quad}));
                        }
                        if found != word || glyphs.is_empty() {
                            return Err(unsupported());
                        }
                        regions.extend(glyphs);
                    }
                }
                if regions.is_empty() {
                    return Err(unsupported());
                }
                text_target = json!({"page":p+1,"paragraph":k+1,"text":if document.allows_copying(){block.text.as_str()}else{""}});
                vec![p + 1]
            }
            "page_rotate" => args["pages"]
                .as_array()
                .ok_or_else(unsupported)?
                .iter()
                .map(|p| {
                    p.as_u64().and_then(|p| usize::try_from(p).ok()).filter(|p| *p > 0 && *p <= document.info.pages.len()).ok_or_else(unsupported)
                })
                .collect::<Result<_, _>>()?,
            "doc_save" => {
                if args.as_object().is_none_or(|object| object.keys().any(|key| !matches!(key.as_str(), "doc" | "path"))) {
                    return Err(unsupported());
                }
                (1..=document.info.pages.len()).collect()
            }
            // A rectangular comment's public coordinates are view-space. Normalize all four
            // corners through PageInfo so rotation and an offset CropBox cannot change scope.
            "comment_add" if matches!(args["type"].as_str(), Some("rectangle" | "square" | "ellipse" | "circle")) => {
                let p = page()?;
                let r = args["rect"].as_array().filter(|r| r.len() == 4).ok_or_else(unsupported)?;
                let r =
                    r.iter().map(Value::as_f64).collect::<Option<Vec<_>>>().filter(|r| r.iter().all(|x| x.is_finite())).ok_or_else(unsupported)?;
                let info = &document.info.pages[p];
                let points = [[r[0], r[1]], [r[0], r[3]], [r[2], r[1]], [r[2], r[3]]].map(|p| info.view_to_user(p[0] as f32, p[1] as f32));
                let rect = [
                    points.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min),
                    points.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min),
                    points.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max),
                    points.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max),
                ];
                if !rect.iter().all(|x| x.is_finite()) {
                    return Err(unsupported());
                }
                regions.push(json!({"page":p+1,"rect":rect}));
                vec![p + 1]
            }
            _ => return Err(unsupported()),
        };
        if pages.is_empty() {
            return Err(unsupported());
        }
        Ok(json!({"doc":id.0,"revision":document.edit_generation(),"pages":pages,"rects":regions,"textTarget":text_target,"tool":name}))
    }
}
