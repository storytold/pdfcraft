//! Inspect model proposals on a PNG without parsing or modifying a PDF.
use pdfcraft_form_detect::{Detector, Kind};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let model = args.get(1).ok_or("usage: detect MODEL.onnx PAGE.png")?;
    let path = args.get(2).ok_or("usage: detect MODEL.onnx PAGE.png")?;
    let image = image::open(path)?.into_rgba8();
    let detector = Detector::load(std::path::Path::new(model))?;
    let proposals = detector.detect(image.as_raw(), image.width(), image.height(), 0.3)?;
    let fields: Vec<_> = proposals
        .iter()
        .map(|p| {
            serde_json::json!({
                "kind": match p.kind { Kind::Text => "text", Kind::CheckBox => "checkbox", Kind::Signature => "signature" },
                "rect": p.rect,
                "confidence": p.confidence
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&fields)?);
    Ok(())
}
