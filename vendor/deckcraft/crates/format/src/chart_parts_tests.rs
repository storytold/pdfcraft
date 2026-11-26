use std::io::{Cursor, Write};
use std::sync::Arc;

use super::{Budget, Presentation, load, load_checked, load_within, read_entry, read_entry_max, save};
use deckcraft_model::chart::{Chart, ChartPart, ChartRel, ChartType};
use deckcraft_model::{Master, Shape, ShapeId, ShapeKind};
use serde_json::{Value, json};

const PART: &str = "ppt/embeddings/owned.xlsx";

fn fixture(payload: Vec<u8>) -> Presentation {
    // Construct through the old manifest representation: this also exercises compatibility.
    let mut part: ChartPart = serde_json::from_value(json!({
        "name": PART, "contentType": "application/zip", "bytes": []
    }))
    .unwrap();
    part.bytes = payload;
    let mut chart = Chart::sample(ChartType::Pie);
    chart.kept.rels.push(ChartRel {
        id: "rWorkbook".into(),
        rel_type: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/package".into(),
        target: "../embeddings/owned.xlsx".into(),
        part: PART.into(),
        external: false,
    });
    chart.kept.parts.push(part);
    let mut p = Presentation::default();
    let child = Shape { id: ShapeId(p.alloc_id().unwrap()), kind: ShapeKind::Chart(Box::new(chart)), ..Default::default() };
    let group = Shape {
        id: ShapeId(p.alloc_id().unwrap()),
        kind: ShapeKind::Group { children: vec![child], child: Default::default() },
        ..Default::default()
    };
    Arc::make_mut(&mut p.slides[0]).shapes = vec![group];
    p
}

fn part_json(value: &mut Value) -> Option<&mut Value> {
    if value.get("name").and_then(Value::as_str) == Some(PART) && value.get("contentType").is_some() {
        return Some(value);
    }
    match value {
        Value::Array(values) => values.iter_mut().find_map(part_json),
        Value::Object(values) => values.values_mut().find_map(part_json),
        _ => None,
    }
}

fn rewrite_manifest(bytes: &[u8], edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut json: Value = serde_json::from_slice(&read_entry(&mut z, "presentation.json").unwrap()).unwrap();
    edit(&mut json);
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let f = z.by_index(i).unwrap();
        if f.name() == "presentation.json" {
            out.start_file("presentation.json", zip::write::SimpleFileOptions::default()).unwrap();
            out.write_all(&serde_json::to_vec(&json).unwrap()).unwrap();
        } else {
            out.raw_copy_file(f).unwrap();
        }
    }
    out.finish().unwrap().into_inner()
}

#[test]
fn large_chart_payload_is_a_referenced_zip_entry() {
    let payload = vec![200; 3 * 1024 * 1024];
    let p = fixture(payload.clone());
    let bytes = save(&p).unwrap();
    let mut z = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert!(z.by_name("presentation.json").unwrap().size() < 128 * 1024, "binary chart bytes must not become JSON numbers");
    let mut manifest: Value = serde_json::from_slice(&read_entry(&mut z, "presentation.json").unwrap()).unwrap();
    let part = part_json(&mut manifest).unwrap();
    assert!(part.get("bytes").is_none(), "new manifests keep only metadata and a reference");
    let entry = part["nativeEntry"].as_str().unwrap();
    assert!(entry.starts_with("chart-parts/"));
    assert_eq!(read_entry(&mut z, entry).unwrap(), payload);
    assert!(read_entry_max(&mut z, entry, payload.len() as u64 - 1).is_err());
    assert_eq!(load(&bytes).unwrap(), p);
}

#[test]
fn chart_parts_share_media_byte_and_item_budgets() {
    let mut p = fixture(vec![7; 600]);
    p.add_media("a.bin", "application/octet-stream", vec![8; 600]).unwrap();
    let bytes = save(&p).unwrap();
    assert!(load_within(&bytes, Budget::media(1200, 2)).is_ok());
    assert!(load_within(&bytes, Budget::media(1199, 2)).is_err(), "chart payloads share the cumulative media budget");
    assert!(load_within(&bytes, Budget::media(1200, 1)).is_err(), "chart parts share the media item budget");
    let duplicated = rewrite_manifest(&bytes, |manifest| {
        let media = manifest["presentation"]["media"].as_array_mut().unwrap();
        media.push(media[0].clone());
    });
    assert!(load_within(&duplicated, Budget::media(1200, 2)).is_err(), "duplicate media IDs still count as listed items");
}

#[test]
fn legacy_inline_chart_bytes_are_budgeted_and_preserved() {
    let mut p = fixture(vec![9; 600]);
    p.add_media("a.bin", "application/octet-stream", vec![8; 600]).unwrap();
    let legacy = rewrite_manifest(&save(&p).unwrap(), |manifest| {
        let part = part_json(manifest).unwrap().as_object_mut().unwrap();
        part.remove("nativeEntry");
        part.insert("bytes".into(), json!(vec![9_u8; 600]));
    });
    assert_eq!(load_within(&legacy, Budget::media(1200, 2)).unwrap().0, p);
    assert!(load_within(&legacy, Budget::media(1199, 2)).is_err());
    assert!(load_within(&legacy, Budget::media(1200, 1)).is_err());
}

#[test]
fn chart_payloads_in_every_shape_owner_get_distinct_entries() {
    let mut p = fixture(vec![42; 4096]);
    let shapes = p.slides[0].shapes.clone();
    let master = Arc::make_mut(&mut p.masters[0]);
    master.shapes = shapes.clone();
    master.layouts[0].shapes = shapes.clone();
    p.notes_master = Some(Arc::new(Master { shapes: shapes.clone(), ..Default::default() }));
    p.handout_master = Some(Arc::new(Master { shapes, ..Default::default() }));
    let bytes = save(&p).unwrap();
    let z = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(z.file_names().filter(|name| name.starts_with("chart-parts/")).count(), 5);
    assert_eq!(load(&bytes).unwrap(), p);
}

#[test]
fn missing_or_corrupt_chart_payload_is_reported() {
    let p = fixture(b"SYNTHETIC-CHART-PAYLOAD-0123456789".to_vec());
    let bytes = save(&p).unwrap();
    let mut z = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    let mut json: Value = serde_json::from_slice(&read_entry(&mut z, "presentation.json").unwrap()).unwrap();
    let name = part_json(&mut json).unwrap()["nativeEntry"].as_str().unwrap().to_owned();
    let start = z.by_name(&name).unwrap().data_start() as usize;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let f = z.by_index(i).unwrap();
        if f.name() != name {
            out.raw_copy_file(f).unwrap();
        }
    }
    let missing = out.finish().unwrap().into_inner();
    let (q, damaged) = load_checked(&missing).unwrap();
    assert_eq!(damaged.len(), 1, "{damaged:?}");
    let ShapeKind::Group { children, .. } = &q.slides[0].shapes[0].kind else { panic!("group") };
    let ShapeKind::Chart(chart) = &children[0].kind else { panic!("chart") };
    assert!(chart.kept.parts[0].bytes.is_empty());
    let mut corrupt = bytes.clone();
    corrupt[start] ^= 0xff;
    assert!(load_checked(&corrupt).is_err());
}

#[test]
fn chart_references_cannot_alias_other_members_or_mix_inline_bytes() {
    let p = fixture(vec![3; 10]);
    let bytes = save(&p).unwrap();
    for entry in ["media/1", "chart-parts/../0", "chart-parts/00", "chart-parts/-1"] {
        let changed = rewrite_manifest(&bytes, |manifest| {
            part_json(manifest).unwrap()["nativeEntry"] = json!(entry);
        });
        assert!(load(&changed).is_err(), "accepted {entry}");
    }
    let ambiguous = rewrite_manifest(&bytes, |manifest| {
        part_json(manifest).unwrap()["bytes"] = json!([3]);
    });
    assert!(load(&ambiguous).is_err());

    let mut p = p;
    let ShapeKind::Group { children, .. } = &mut Arc::make_mut(&mut p.slides[0]).shapes[0].kind else { panic!("group") };
    let ShapeKind::Chart(chart) = &mut children[0].kind else { panic!("chart") };
    chart.kept.parts.push(chart.kept.parts[0].clone());
    let aliased = rewrite_manifest(&save(&p).unwrap(), |manifest| {
        fn alias(value: &mut Value) {
            if let Some(entry) = value.get_mut("nativeEntry") {
                *entry = json!("chart-parts/0");
            }
            match value {
                Value::Array(values) => values.iter_mut().for_each(alias),
                Value::Object(values) => values.values_mut().for_each(alias),
                _ => {}
            }
        }
        alias(manifest);
    });
    assert!(load(&aliased).is_err(), "one ZIP member must not be inflated repeatedly");
}

fn with_two_parts() -> Presentation {
    let mut p = fixture(vec![3; 10]);
    let ShapeKind::Group { children, .. } = &mut Arc::make_mut(&mut p.slides[0]).shapes[0].kind else { panic!("group") };
    let ShapeKind::Chart(chart) = &mut children[0].kind else { panic!("chart") };
    let mut second = chart.kept.parts[0].clone();
    second.bytes = vec![4; 10];
    chart.kept.parts.push(second);
    p
}

#[test]
fn corrupt_chart_members_stop_before_reusing_the_inflation_budget() {
    let mut bytes = save(&with_two_parts()).unwrap();
    let offsets: Vec<_> = {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        (0..2).map(|i| z.by_name(&format!("chart-parts/{i}")).unwrap().data_start() as usize).collect()
    };
    for offset in offsets {
        bytes[offset] ^= 0xff;
    }
    // A failed CRC may already have inflated bytes. Continuing with the same remaining
    // budget would let each corrupt member consume that work budget again.
    let result = load_within(&bytes, Budget::media(10, 2));
    assert!(result.is_err(), "two corrupt 10-byte members must not reuse a 10-byte budget");
    assert!(result.unwrap_err().to_string().contains("chart-parts/0"));
}

#[test]
fn duplicate_physical_zip_names_are_indexed_and_inflated_once() {
    let mut bytes = save(&with_two_parts()).unwrap();
    // ZipWriter refuses duplicate names. Forge equal-length local/central-directory names
    // instead, to exercise ZipArchive's duplicate-name indexing on an untrusted archive.
    let original = b"chart-parts/1";
    let mut replaced = 0;
    for offset in 0..=bytes.len() - original.len() {
        if &bytes[offset..offset + original.len()] == original {
            bytes[offset + original.len() - 1] = b'0';
            replaced += 1;
        }
    }
    assert_eq!(replaced, 2);
    let z = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(z.file_names().filter(|n| n.starts_with("chart-parts/")).count(), 1);
    let (q, damaged) = load_within(&bytes, Budget::media(10, 2)).unwrap();
    assert_eq!(damaged.len(), 1, "the second reference is missing, not inflated again");
    let ShapeKind::Group { children, .. } = &q.slides[0].shapes[0].kind else { panic!("group") };
    let ShapeKind::Chart(chart) = &children[0].kind else { panic!("chart") };
    assert_eq!(chart.kept.parts[0].bytes, vec![4; 10]);
    assert!(chart.kept.parts[1].bytes.is_empty());
}
