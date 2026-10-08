//! Print how a file was opened: revisions, repair notes, trailer, security and the catalog.
//! `cargo run -p pdfcraft-cos --example cosinfo f.pdf [password] [object-number…]`
fn show(o: &pdfcraft_cos::Object) -> String {
    let mut t = Vec::new();
    pdfcraft_cos::serialize(o, &mut t);
    let s = String::from_utf8_lossy(&t).into_owned();
    s.chars().take(400).collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: cosinfo <file.pdf> [password] [obj…]");
    let password = args.next().filter(|p| !p.is_empty());
    let objs: Vec<u32> = args.filter_map(|a| a.parse().ok()).collect();
    let bytes = std::sync::Arc::new(std::fs::read(&path).expect("readable"));
    match pdfcraft_cos::Document::open_with_password(bytes, password.as_deref()) {
        Ok(doc) => {
            println!("version {}  revisions {:?}", doc.version(), doc.revisions());
            for r in doc.repair_log() {
                println!("repair: {r}");
            }
            println!("trailer {}", show(&pdfcraft_cos::Object::Dict(doc.trailer().clone())));
            if let Some(s) = doc.security() {
                println!("security: auth {:?}, key {} bytes, {:?}", s.auth(), s.file_key().len(), s.permissions());
            }
            if let Some(r) = doc.root() {
                println!("catalog {}", show(&doc.get(r)));
            }
            for n in objs {
                match doc.try_get(n) {
                    Ok(o) => println!("{n} 0 obj {}", show(&o)),
                    Err(e) => println!("{n} 0 obj error: {e}"),
                }
            }
        }
        Err(e) => println!("error: {e}"),
    }
}
