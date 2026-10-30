use pdfcraft_ffi::{Engine, FfiRenderRequest, FfiTile};
use std::thread;

fn synthetic_pdf(pages: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    let mut objs: Vec<String> = Vec::new();
    objs.push("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string());
    let mut kids = String::new();
    for i in 0..pages {
        kids.push_str(&format!("{} 0 R ", 3 + i * 2));
    }
    objs.push(format!("2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {pages} >>\nendobj\n"));
    for i in 0..pages {
        let page_obj_num = 3 + i * 2;
        let content_obj_num = page_obj_num + 1;
        objs.push(format!("{page_obj_num} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {content_obj_num} 0 R >>\nendobj\n"));
        let stream = format!("BT /F1 24 Tf 100 700 Td (Concurrent Page {}) Tj ET", i + 1);
        objs.push(format!("{content_obj_num} 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n", stream.len(), stream));
    }
    for obj in &objs {
        offsets.push(out.len());
        out.extend_from_slice(obj.as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn test_concurrent_open_render_close() {
    let engine = Engine::new();
    let num_threads = 8;
    let iterations_per_thread = 5;

    let handles: Vec<_> = (0..num_threads)
        .map(|t_idx| {
            let engine = engine.clone();
            thread::spawn(move || {
                for iter in 0..iterations_per_thread {
                    let pdf = synthetic_pdf(2);
                    let doc_id = engine.open_bytes(format!("doc_{t_idx}_{iter}.pdf"), pdf, None).expect("open in thread");

                    let req = FfiRenderRequest {
                        page: 0,
                        tile: Some(FfiTile { x: 0, y: 0, width: 128, height: 128 }),
                        scale: 1.0,
                        tag: (t_idx * 100 + iter) as u64,
                        preview: false,
                        reading_colors: None,
                        hide_comments: false,
                    };

                    let res = engine.render_tile(doc_id, req).expect("render in thread");
                    assert_eq!(res.width, 128);
                    assert_eq!(res.height, 128);

                    engine.close(doc_id).expect("close in thread");
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("thread finished cleanly");
    }

    assert_eq!(engine.document_count(), 0);
}
