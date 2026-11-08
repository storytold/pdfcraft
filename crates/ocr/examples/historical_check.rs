//! Local verification against line images; no fixture or model is bundled.
use std::{collections::BTreeMap, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).ok_or("expected model directory")?);
    let lines = PathBuf::from(args.get(2).ok_or("expected line directory")?);
    let out = PathBuf::from(args.get(3).ok_or("expected output JSON path")?);
    let model = pdfcraft_ocr::historical::Historical::load(&dir)?;
    let mut results = BTreeMap::new();
    for entry in std::fs::read_dir(lines)? {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "png") {
            continue;
        }
        let image = image::open(&path)?.to_rgb8();
        let words = model.read_line(&image, [0.0, 0.0, image.width() as f32, image.height() as f32])?;
        let text = words.iter().map(|word| word.text.as_str()).collect::<Vec<_>>().join(" ");
        let name = path.file_stem().ok_or("missing image name")?.to_string_lossy().into_owned();
        println!("{name}: {text}");
        results.insert(name, text);
    }
    std::fs::write(out, serde_json::to_vec_pretty(&results)?)?;
    Ok(())
}
