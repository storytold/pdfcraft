//! Encrypt a PDF: `cargo run -p printcraft-cos --example protect in.pdf out.pdf rc4-40|rc4-128|aes-128|aes-256 USER OWNER`
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, alg, user, owner] = &a[..] else { panic!("usage: protect in out alg user owner") };
    let algorithm = match alg.as_str() {
        "rc4-40" => printcraft_cos::Algorithm::Rc4_40,
        "rc4-128" => printcraft_cos::Algorithm::Rc4_128,
        "aes-128" => printcraft_cos::Algorithm::Aes128,
        _ => printcraft_cos::Algorithm::Aes256,
    };
    let mut doc = printcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(input).unwrap())).unwrap();
    doc.set_encryption(&printcraft_cos::NewEncryption {
        algorithm,
        user_password: user,
        owner_password: owner,
        permissions: -1,
        encrypt_metadata: true,
        seed: [1; 32],
    })
    .unwrap();
    std::fs::write(output, printcraft_cos::write_full(&doc, &printcraft_cos::SaveOptions::default()).unwrap()).unwrap();
}
