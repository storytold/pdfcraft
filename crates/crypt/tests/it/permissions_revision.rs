//! The permission accessors of `pdfcraft_crypt::Permissions`, checked against the flag table of
//! ISO 32000-1:2008 (Table 22). Expected values come from that table only.
//!
//! Bit n has the mask `1 << (n - 1)`. Readers use bits 3 to 6 and 9 to 12 and ignore the rest.

use pdfcraft_crypt::{Algorithm, Auth, NewEncryption, Permissions, SecurityHandler, create};

const ID: &[u8] = b"0123456789abcdef";
const T: bool = true;
const F: bool = false;

/// The eight accessors in this order: print, high-quality print, modify, copy, accessibility,
/// annotate, fill forms, assemble.
fn cells(p: &Permissions) -> [bool; 8] {
    [p.print(), p.print_high_quality(), p.modify(), p.copy(), p.extract_for_accessibility(), p.annotate(), p.fill_forms(), p.assemble()]
}

/// Creates a file with `flags` at revision `revision`, opens it with the user password, and
/// returns the user's permissions.
fn user_view(alg: Algorithm, revision: i64, flags: i32) -> Permissions {
    let made = create(
        &NewEncryption { algorithm: alg, user_password: "user", owner_password: "owner", permissions: flags, encrypt_metadata: true, seed: [42; 32] },
        ID,
    )
    .expect("the handler is created");
    assert_eq!(made.dict().r, revision, "the algorithm writes revision {revision}");
    let opened = SecurityHandler::open(made.dict().clone(), ID, Some("user")).expect("the user password opens the file");
    assert_eq!(opened.auth(), Auth::User, "the user password gives user access");
    opened.permissions()
}

/// Revision 3 and later: each row is a flag word and the expected eight cells.
const REVISION_3_ROWS: &[(&str, i32, [bool; 8])] = &[
    ("every meaningful flag set, ignored flags clear", -196, [T, T, T, T, T, T, T, T]),
    ("every flag set, ignored flags set too", -1, [T, T, T, T, T, T, T, T]),
    ("reserved flags only", -4096, [F, F, F, F, F, F, F, F]),
    ("ignored flags set, meaningful flags clear", -3901, [F, F, F, F, F, F, F, F]),
    ("modify flag clear", -9, [T, T, F, T, T, T, T, T]),
    ("copy flag clear", -17, [T, T, T, F, T, T, T, T]),
    ("annotate flag clear", -33, [T, T, T, T, T, F, T, T]),
    ("annotate and fill-existing flags clear", -289, [T, T, T, T, T, F, F, T]),
    ("annotate flag set, fill-existing flag clear", -257, [T, T, T, T, T, T, T, T]),
    ("accessibility flag clear", -513, [T, T, T, T, F, T, T, T]),
    ("high-quality flag clear, print flag set", -2049, [T, F, T, T, T, T, T, T]),
    ("bits 3 to 12 set, reserved bits clear", 4092, [T, T, T, T, T, T, T, T]),
    ("positive flag word, bit 32 clear", 2_147_483_647, [T, T, T, T, T, T, T, T]),
    ("revision 2 example read at revision 3", -44, [T, T, F, T, T, F, T, T]),
];

#[test]
fn revision_three_rows_hold_through_a_created_and_reopened_file() {
    for (name, bits, want) in REVISION_3_ROWS {
        assert_eq!(cells(&user_view(Algorithm::Rc4_128, 3, *bits)), *want, "{name}: flag word {bits:#010x}");
    }
}

#[test]
fn owner_access_gives_every_flag() {
    let made = create(
        &NewEncryption {
            algorithm: Algorithm::Rc4_128,
            user_password: "user",
            owner_password: "owner",
            permissions: 0,
            encrypt_metadata: true,
            seed: [42; 32],
        },
        ID,
    )
    .expect("the handler is created");
    assert_eq!(made.auth(), Auth::Owner, "the owner password gives owner access");
    assert_eq!(cells(&made.permissions()), [T; 8]);
}

#[test]
fn revision_two_print_modify_copy_and_annotate_follow_the_table() {
    for (bits, want) in [(-4, [T, T, T, T]), (-44, [T, F, T, F]), (-17, [T, T, F, T])] {
        let p = user_view(Algorithm::Rc4_40, 2, bits);
        assert_eq!([p.print(), p.modify(), p.copy(), p.annotate()], want, "revision 2, flag word {bits}");
    }
}

#[test]
fn revision_two_with_every_flag_set_grants_accessibility_and_form_filling() {
    let p = user_view(Algorithm::Rc4_40, 2, -4);
    assert!(p.extract_for_accessibility(), "bit 5 is set, and at revision 2 it covers accessibility");
    assert!(p.fill_forms(), "bit 6 is set");
}

#[test]
fn revision_three_assembly_needs_its_own_flag() {
    let p = user_view(Algorithm::Rc4_128, 3, -1025);
    assert!(p.modify(), "bit 4 is set");
    assert!(!p.assemble(), "assembly needs bit 11; bit 4 alone does not grant it");
}

#[test]
fn revision_three_high_quality_print_needs_the_print_flag() {
    let p = user_view(Algorithm::Rc4_128, 3, -5);
    assert!(!p.print(), "bit 3 is clear");
    assert!(!p.print_high_quality(), "high-quality print needs bit 3 and bit 12");
}

#[test]
fn revision_two_accessibility_follows_the_copy_flag() {
    let p = user_view(Algorithm::Rc4_40, 2, -17);
    assert!(!p.copy(), "bit 5 is clear");
    assert!(!p.extract_for_accessibility(), "at revision 2 accessibility is bit 5");
}

#[test]
fn revision_two_form_filling_ignores_bit_nine() {
    let p = user_view(Algorithm::Rc4_40, 2, -44);
    assert!(!p.annotate(), "bit 6 is clear");
    assert!(!p.fill_forms(), "at revision 2 form filling is bit 6 alone");
}

#[test]
fn revision_two_ignores_the_flags_only_later_revisions_define() {
    // Bit 4 is clear and bit 11 set: bit 11 means nothing at revision 2, so assembly stays refused.
    let p = user_view(Algorithm::Rc4_40, 2, -44);
    assert!(!p.assemble(), "at revision 2 assembly is bit 4 alone");
    // Bit 3 is set and bit 12 clear: printing at revision 2 has one quality.
    let p = user_view(Algorithm::Rc4_40, 2, -2049);
    assert!(p.print_high_quality(), "at revision 2 high-quality print is bit 3 alone");
}
