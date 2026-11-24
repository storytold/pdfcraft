//! Keep opaque chart payloads out of the JSON manifest and within the media load budget.
use std::collections::HashSet;
use std::sync::Arc;

use craft_deckcraft_format::{MAX_ENTRY, Package, chart_part_entry, is_chart_part_entry};

use deckcraft_model::chart::ChartPart;
use deckcraft_model::{Master, Presentation, Shape, ShapeKind};

use crate::{FormatError, Result};

type Payload = (String, String, Vec<u8>);

pub(super) fn separate(p: &mut Presentation) -> Result<Vec<Payload>> {
    let mut payloads = Vec::new();
    visit_parts(p, |part| {
        // The native namespace is independent of the original OPC names, which may repeat
        // when charts from several decks are combined. No user-supplied path is extracted.
        let entry = chart_part_entry(payloads.len());
        part.native_entry = Some(entry.clone());
        payloads.push((entry, part.content_type.clone(), std::mem::take(&mut part.bytes)));
        Ok(())
    })?;
    Ok(payloads)
}

pub(super) fn restore(
    p: &mut Presentation,
    z: &mut Package<'_>,
    left: &mut u64,
    mut items: usize,
    max_items: usize,
    damaged: &mut Vec<String>,
) -> Result<()> {
    let mut seen = HashSet::new();
    visit_parts(p, |part| {
        if items >= max_items {
            return Err(FormatError::Damaged(format!("media and chart parts exceed {max_items} items")));
        }
        items += 1;
        if let Some(entry) = part.native_entry.take() {
            // Accept only names produced by separate(), and inflate each member once.
            let canonical = is_chart_part_entry(&entry);
            if !canonical || !seen.insert(entry.clone()) || !part.bytes.is_empty() {
                return Err(FormatError::Damaged("invalid or ambiguous chart part reference".into()));
            }
            if !z.contains(&entry) {
                // Missing members consume no inflation work and can be reported for recovery.
                damaged.push(format!("{entry}: missing chart part"));
                return Ok(());
            }
            if z.declared_size(&entry).is_some_and(|size| size > *left) {
                return Err(FormatError::Damaged("embedded media and chart parts are larger than DeckCraft can open".into()));
            }
            // A CRC/decode error may already have inflated bytes. Stop on that first error
            // rather than letting later corrupt members reuse the unchanged byte budget.
            let bytes = z.read_max(&entry, (*left).min(MAX_ENTRY))?;
            *left -= bytes.len() as u64;
            part.bytes = bytes;
        } else {
            // Older manifests used JSON byte arrays. Charge those too, after the existing
            // bounded manifest parser has read them, so resaving migrates them losslessly.
            let size = part.bytes.len() as u64;
            if size > (*left).min(MAX_ENTRY) {
                return Err(FormatError::Damaged("inline chart part is larger than DeckCraft can open".into()));
            }
            *left -= size;
        }
        Ok(())
    })
}

fn visit_parts(p: &mut Presentation, mut visit: impl FnMut(&mut ChartPart) -> Result<()>) -> Result<()> {
    // Use an explicit stack for groups. Charts can also live on masters and their layouts,
    // including the notes and handout masters; none of these owners may leak inline bytes.
    for slide in &mut p.slides {
        shapes(&mut Arc::make_mut(slide).shapes, &mut visit)?;
    }
    for master in &mut p.masters {
        master_parts(Arc::make_mut(master), &mut visit)?;
    }
    for master in [&mut p.notes_master, &mut p.handout_master].into_iter().flatten() {
        master_parts(Arc::make_mut(master), &mut visit)?;
    }
    Ok(())
}

fn master_parts(master: &mut Master, visit: &mut impl FnMut(&mut ChartPart) -> Result<()>) -> Result<()> {
    shapes(&mut master.shapes, visit)?;
    for layout in &mut master.layouts {
        shapes(&mut layout.shapes, visit)?;
    }
    Ok(())
}

fn shapes(root: &mut [Shape], visit: &mut impl FnMut(&mut ChartPart) -> Result<()>) -> Result<()> {
    let mut pending = vec![root];
    while let Some(shapes) = pending.pop() {
        for shape in shapes {
            match &mut shape.kind {
                ShapeKind::Group { children, .. } => pending.push(children),
                ShapeKind::Chart(chart) => {
                    for part in &mut chart.kept.parts {
                        visit(part)?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
