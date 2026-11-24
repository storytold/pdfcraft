//! Encrypted workbooks end to end: compound file → streams → plain package.

use crate::testing::{compound_file, encrypt_workbook, encrypt_workbook_standard};
use crate::{DecryptError, decrypt};

#[test]
fn encrypted_files_decrypt_to_the_package() {
    let package: Vec<u8> = (0..3_000_000u32).map(|i| (i * 31 % 251) as u8).collect();
    let agile = encrypt_workbook(&package, "Pass1234");
    assert!(crate::crypto::is_encrypted(&agile));
    assert_eq!(decrypt(&agile, "Pass1234").unwrap(), package);
    assert_eq!(decrypt(&agile, "wrong"), Err(DecryptError::WrongPassword));
    let standard = encrypt_workbook_standard(&package, "Pass1234");
    assert_eq!(decrypt(&standard, "Pass1234").unwrap(), package);
}

#[test]
fn a_compound_file_without_the_streams_is_malformed() {
    let file = compound_file(&[("EncryptionInfo", &[4, 0, 4, 0])]);
    assert!(matches!(decrypt(&file, "x"), Err(DecryptError::Malformed(_))));
    assert!(matches!(decrypt(b"not a compound file", "x"), Err(DecryptError::Malformed(_))));
}

#[test]
fn a_gridcraft_workbook_survives_encryption() {
    let b2 = gridcraft_core::CellRef::parse("B2").unwrap();
    let mut wb = gridcraft_model::Workbook::new();
    wb.sheet_mut(0).unwrap().set_value(b2, gridcraft_core::Value::Number(42.0));
    let xlsx = crate::write_xlsx(&wb).unwrap();
    let file = encrypt_workbook(&xlsx, "Pass1234");
    assert_eq!(crate::sniff(&file), crate::Format::Encrypted);
    let (back, _) = crate::read_xlsx(&decrypt(&file, "Pass1234").unwrap()).unwrap();
    assert_eq!(back.active().unwrap().value(b2), gridcraft_core::Value::Number(42.0));
}

#[test]
fn schemes_no_password_opens_are_named_before_asking_for_one() {
    use crate::check_encryption;
    let unsupported = |s: &str| Err(DecryptError::Unsupported(s.into()));
    assert_eq!(check_encryption(&encrypt_workbook(b"package", "pw")), Ok(()));
    assert_eq!(check_encryption(&encrypt_workbook_standard(b"package", "pw")), Ok(()));
    let file = |info: &[u8]| compound_file(&[("EncryptionInfo", info), ("EncryptedPackage", &[0; 8])]);
    assert_eq!(check_encryption(&file(&[3, 0, 2, 0, 4, 0, 0, 0])), unsupported("RC4"));
    assert_eq!(check_encryption(&file(&[4, 0, 3, 0, 0, 0, 0, 0])), unsupported("Extensible"));
    // An Agile descriptor whose only key encryptor is a certificate.
    let agile = encrypt_workbook(b"package", "pw");
    let cf = craft_cfb::CompoundFile::open(&agile).unwrap();
    let info = cf.read(cf.child(0, "EncryptionInfo").unwrap()).unwrap();
    let mut cert = info[..8].to_vec();
    cert.extend(String::from_utf8(info[8..].to_vec()).unwrap().replace("keyEncryptor/password", "keyEncryptor/certificate").into_bytes());
    assert_eq!(check_encryption(&file(&cert)), unsupported("certificate"));
    assert!(matches!(check_encryption(&file(&[4, 0])), Err(DecryptError::Malformed(_))));
    // Rights management (IRM) keeps an EncryptedPackage but no EncryptionInfo.
    let irm = compound_file(&[("EncryptedPackage", &[0; 8])]);
    assert_eq!(check_encryption(&irm), unsupported("rights-management"));
    assert_eq!(decrypt(&irm, "pw"), Err(DecryptError::Unsupported("rights-management".into())));
}
