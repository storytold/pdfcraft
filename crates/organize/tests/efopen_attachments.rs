//! A document that encrypts only its embedded files (`/AuthEvent /EFOpen`, ISO 32000-2 §7.6.6,
//! Table 25), opened without its password, holds them encrypted: combining it into another
//! document must not copy that data as if it were the files.

use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Document, NewEncryption, SaveOptions, write_full};
use pdfcraft_organize::{OrganizeError, combine};

/// The password values (`/O`, `/U`, `/OE`, `/UE`, `/Perms`) of an AES-256 (R6) protection with a
/// user password, as a save writes them.
fn r6_password_values() -> String {
    let plain = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF".to_vec();
    let mut doc = Document::open(Arc::new(plain)).unwrap();
    let params = NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: "user pw",
        owner_password: "owner pw",
        permissions: -4,
        encrypt_metadata: true,
        seed: [5; 32],
    };
    doc.set_encryption(&params).unwrap();
    let saved = Document::open_with_password(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap()), Some("user pw")).unwrap();
    let encrypt = saved.resolve(saved.trailer().get(b"Encrypt").unwrap());
    let encrypt = encrypt.as_dict().unwrap();
    ["O", "U", "OE", "UE", "Perms"]
        .iter()
        .map(|k| {
            let value = saved.resolve(encrypt.get(k.as_bytes()).unwrap());
            let hex: String = value.as_string().unwrap().bytes.iter().map(|b| format!("{b:02X}")).collect();
            format!("/{k} <{hex}>")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One page in the clear and one attached file encrypted with `/StdCF`, authorised by `/EFOpen`.
fn efopen_pdf() -> Vec<u8> {
    let encrypt = format!(
        "<< /Filter /Standard /V 5 /R 6 {} /P -4 /CF << /StdCF << /AuthEvent /EFOpen /CFM /AESV3 /Length 32 >> >> /StmF /Identity /StrF /Identity /EFF /StdCF >>",
        r6_password_values()
    );
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(note.txt) 4 0 R] >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".to_string(),
        "<< /Type /Filespec /F (note.txt) /EF << /F 5 0 R >> >>".to_string(),
        "<< /Type /EmbeddedFile /Length 32 >>\nstream\n0123456789abcdef0123456789abcdef\nendstream".to_string(),
        encrypt,
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R /Encrypt 6 0 R /ID [<00> <00>] >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes(),
    );
    out
}

#[test]
fn combining_does_not_copy_attachments_held_encrypted() {
    let src = Document::open(Arc::new(efopen_pdf())).unwrap();
    assert!(src.embedded_files_locked());
    match combine(&[("efopen.pdf", &src)]) {
        Err(OrganizeError::Invalid(why)) => assert!(why.contains("password"), "{why}"),
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("an encrypted attachment was copied as the file"),
    }
}
