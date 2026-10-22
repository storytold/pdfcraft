//! Encryption numbers from an **outside implementation**. Six of the seven rows below are qpdf's own,
//! written by qpdf core 12.2.0 through pikepdf 9.11, and their `/O`, `/U`, `/OE`, `/UE`, `/Perms`
//! and `/ID` values were copied out of those files rather than computed here. The seventh row,
//! `crafted-r3-40bit-hand-derived`, is ours and says so where it is defined - no producer we can run
//! here writes revision 3 at 40 bits (qpdf core 12.2.0 emits `/Length 128` at `R=3`, measured below),
//! so nothing outside computed that one either.
//!
//! That distinction is the whole point. `create()` and `open()` share one derivation, so a wrong-but
//! self-consistent implementation round-trips cleanly against itself: across two review rounds we
//! broke the key derivation several ways and the unit suite stayed green (deleting Algorithm 2 step f
//! leaves all 17 unit tests passing while our rows fail). Quoting an outsider's bytes
//! breaks the circle - `open()` recomputes `/U` (or the revision 6 hash chain) and compares it to what
//! was stored, so a derivation that diverges cannot authenticate at all.
//!
//! Revisions covered: 2 (export-grade RC4, where the five-byte key makes a truncation error visible that
//! every pre-existing R3/R4 vector in this repository hides as a no-op), 3, and 4 and 6 each with metadata
//! encrypted *and* plain. For revision 4 that pairing is the only outside view of Algorithm 2 step
//! f's four `FF` bytes; revision 6 shows the same flag differently - its `/U` differs between the two
//! metadata variants, which is measured here rather than attributed to a named algorithm step. Only the
//! numbers are committed - never the PDFs, and no generator script that lives outside
//! this repository. To re-derive the six qpdf rows, take one blank 200x200 page and save it with
//! pikepdf 9.11 (qpdf core 12.2.0) using `Encryption(owner=OWNER, user=USER,`
//! `allow=Permissions(extract=False), R=<revision>, aes=…, metadata=…)` plus `save(..., static_id=True)`,
//! then read `/Encrypt` back. Below revision 4 both flags are forced rather than chosen, because pikepdf
//! refuses AES and refuses encrypted metadata there - so those two rows need `aes=False, metadata=False`,
//! while revisions 4 and 6 take `aes=True` with the case's own metadata flag. Run to run, every quoted
//! field of those rows reproduces at revisions 2-4, and whole files too for revisions 2 and 3; a
//! revision 4 file falls short of that by one stream, because `/CFM /AESV2` prepends a fresh random IV.
//! Revision 6 reproduces structurally only, its salts being new each run. What stays checked rather than
//! trusted - `open()` recovers the user password from `/O` and verifies `/U` at revisions 2-4
//! (Algorithm 7), matches both stored hashes at revision 6 and unwraps `/OE`/`/UE` into the file key
//! there; this file then decrypts `/Perms` with that key, which authentication itself never reads. One
//! thing no assertion here verifies is the crypt-filter names: renaming a row's `/StmF` or replacing its
//! filter set leaves all three tests green, so those are held to review rather than to a test. For how
//! this repository treats qpdf as an oracle, see `crates/oracle/README.md`.

use pdfcraft_crypt::{Auth, EncryptDict, Method, SecurityHandler, aes_cbc_decrypt};

const USER: &str = "open pw";
const OWNER: &str = "owner pw";

/// One file qpdf wrote. Bytes are held as hex so the table stays a `const`: the literals on screen are
/// then exactly what qpdf emitted, with no runtime step in between to trust.
struct Case {
    label: &'static str,
    v: i64,
    r: i64,
    length_bits: i64,
    p: i32,
    encrypt_metadata: bool,
    id0_hex: &'static str,
    o_hex: &'static str,
    u_hex: &'static str,
    oe_hex: &'static str,
    ue_hex: &'static str,
    perms_hex: &'static str,
    crypt_filters: fn() -> Vec<(Vec<u8>, Method)>,
    stm_f: &'static str,
    str_f: &'static str,
    key_len_bytes: usize,
}

impl Case {
    fn dict(&self) -> EncryptDict {
        EncryptDict {
            filter: b"Standard".to_vec(),
            v: self.v,
            r: self.r,
            length_bits: self.length_bits,
            o: hex(self.o_hex),
            u: hex(self.u_hex),
            oe: hex(self.oe_hex),
            ue: hex(self.ue_hex),
            perms: hex(self.perms_hex),
            p: self.p,
            encrypt_metadata: self.encrypt_metadata,
            crypt_filters: (self.crypt_filters)(),
            stm_f: self.stm_f.as_bytes().to_vec(),
            str_f: self.str_f.as_bytes().to_vec(),
            ef_f: Vec::new(),
        }
    }

    fn id0(&self) -> Vec<u8> {
        hex(self.id0_hex)
    }
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn no_filters() -> Vec<(Vec<u8>, Method)> {
    Vec::new()
}

fn aes128_filter() -> Vec<(Vec<u8>, Method)> {
    vec![(b"StdCF".to_vec(), Method::Aes128)]
}

fn aes256_filter() -> Vec<(Vec<u8>, Method)> {
    vec![(b"StdCF".to_vec(), Method::Aes256)]
}

/// `/ID[0]` is shared by every case because qpdf wrote them with `static_id`.
const CASES: &[Case] = &[
    Case {
        label: "r2-40bit",
        v: 1,
        r: 2,
        length_bits: 40,
        p: -28,
        encrypt_metadata: true,
        id0_hex: "31415926535897932384626433832795",
        o_hex: "166a894580aae60ae93fa9d83fc576f26f13dbe4574ce6c57497bec3ece7f125",
        u_hex: "69ff2fb98261fd094174f93f0388859f356080ca486a38ce2d5ac23b98f5f89c",
        oe_hex: "",
        ue_hex: "",
        perms_hex: "",
        crypt_filters: no_filters,
        stm_f: "",
        str_f: "",
        key_len_bytes: 5,
    },
    Case {
        label: "r3-128rc4",
        v: 2,
        r: 3,
        length_bits: 128,
        p: -1044,
        encrypt_metadata: true,
        id0_hex: "31415926535897932384626433832795",
        o_hex: "2910182a30a271d410d8dada035cbd77c0b57a683bd55cd1cd9eee35a2b64848",
        u_hex: "1b4b8fb3fd213b4eadc142ab9f385fc80122456a91bae5134273a6db134c87c4",
        oe_hex: "",
        ue_hex: "",
        perms_hex: "",
        crypt_filters: no_filters,
        stm_f: "",
        str_f: "",
        key_len_bytes: 16,
    },
    Case {
        label: "r4-aes128-meta-enc",
        v: 4,
        r: 4,
        length_bits: 128,
        p: -1044,
        encrypt_metadata: true,
        id0_hex: "31415926535897932384626433832795",
        o_hex: "2910182a30a271d410d8dada035cbd77c0b57a683bd55cd1cd9eee35a2b64848",
        u_hex: "1b4b8fb3fd213b4eadc142ab9f385fc80122456a91bae5134273a6db134c87c4",
        oe_hex: "",
        ue_hex: "",
        perms_hex: "",
        crypt_filters: aes128_filter,
        stm_f: "StdCF",
        str_f: "StdCF",
        key_len_bytes: 16,
    },
    Case {
        label: "r4-aes128-meta-plain",
        v: 4,
        r: 4,
        length_bits: 128,
        p: -1044,
        encrypt_metadata: false,
        id0_hex: "31415926535897932384626433832795",
        o_hex: "2910182a30a271d410d8dada035cbd77c0b57a683bd55cd1cd9eee35a2b64848",
        u_hex: "d6d363fc817815701f0242a581cb39e60122456a91bae5134273a6db134c87c4",
        oe_hex: "",
        ue_hex: "",
        perms_hex: "",
        crypt_filters: aes128_filter,
        stm_f: "StdCF",
        str_f: "StdCF",
        key_len_bytes: 16,
    },
    Case {
        label: "r6-aes256-meta-enc",
        v: 5,
        r: 6,
        length_bits: 256,
        p: -1044,
        encrypt_metadata: true,
        id0_hex: "31415926535897932384626433832795",
        o_hex: "1c791d66227fdfd7c1d225eb81fb3408d8bcfac1a0ff71cc57b10eb29029e5f6965cccb6b94e461d9da354a9dbd6eea7",
        u_hex: "64ffbf79168ed7b325267cdba8dae211d4c2c5ebdffd339c7ac36c33e26f23b7f478c60ae42d50b5d20ceac56d659151",
        oe_hex: "d2f6ef0bcc6ce93228be760c7eee8f4259af7a6044a4c4e04a0d48ab0893ef1e",
        ue_hex: "963035aa24fb009e6d26b3cffd31492ffe6f4e563e4dcae8d7b7e3203c4a2f56",
        perms_hex: "5675a86560873d2532e856a37045247e",
        crypt_filters: aes256_filter,
        stm_f: "StdCF",
        str_f: "StdCF",
        key_len_bytes: 32,
    },
    Case {
        label: "r6-aes256-meta-plain",
        v: 5,
        r: 6,
        length_bits: 256,
        p: -1044,
        encrypt_metadata: false,
        id0_hex: "31415926535897932384626433832795",
        o_hex: "e227e586c4dc5a362c7271d960565cf3a37145e33af5bf5a08c73b52a1d819e152c737176dc08774e400798d6534e95e",
        u_hex: "dbb84b847bb2d80865cc3c5a7c751d4c659d2c96ed1750f8be6f9c3b93fe3d11f494b43e5f17a21c85f5a4b171055afb",
        oe_hex: "8c31f2392bc2a093850269a3404cbe01cea38e5e0fe308fe8ac50f5222cff103",
        ue_hex: "2b0c44e4bddb3c9c01431b4ed3c7da42dbd1ef47e610d626ec2124535a0a4469",
        perms_hex: "adbb69fb9fffde31ba90aaaf5eb4bdaf",
        crypt_filters: aes256_filter,
        stm_f: "StdCF",
        str_f: "StdCF",
        key_len_bytes: 32,
    },
    // HAND-DERIVED, not produced by qpdf: an independent re-implementation of ISO Algorithms 2/3
    // (n = 5) computed this pair, cross-validated by having the same routine reproduce qpdf's
    // n = 16 `/O` byte for byte. It exists because no producer we can run here emits R3 at 40 bits -
    // qpdf core 12.2.0 writes `/Length 128` there, and this crate's own create() maps Rc4_40 to revision 2 - so
    // the only route we have to `key_len` = 5 with the derivation loop RUNNING (r >= 3) is to craft
    // it by hand. With that combination live, the loop's truncation stops being a no-op: mutating
    // `Md5::digest(&digest[..n])` to hash the whole digest makes this password stop authenticating.
    Case {
        label: "crafted-r3-40bit-hand-derived",
        v: 2,
        r: 3,
        length_bits: 40,
        p: -1044,
        encrypt_metadata: true,
        id0_hex: "31415926535897932384626433832795",
        o_hex: "50e50251fd20acb28ba50013142d19a47249d06b977d28b63cb3ce9d56fb5d50",
        u_hex: "000756f24fe648256830aec6155ca6f100000000000000000000000000000000",
        oe_hex: "",
        ue_hex: "",
        perms_hex: "",
        crypt_filters: no_filters,
        stm_f: "",
        str_f: "",
        key_len_bytes: 5,
    },
];

#[test]
fn outside_derived_dictionaries_authenticate_with_the_intended_role() {
    for case in CASES {
        let d = case.dict();
        let id0 = case.id0();

        // The user password must open as a user. Owners are tried first, so an `Owner` here would mean
        // the two passwords had collapsed onto one set of bytes - on these spellings they have not.
        let user = SecurityHandler::open(d.clone(), &id0, Some(USER))
            .unwrap_or_else(|e| panic!("{}: qpdf's user password was refused by our reader: {e}", case.label));
        assert_eq!(user.auth(), Auth::User, "{}", case.label);

        // The owner password must open as owner: revisions 2-4 recover the user password from `/O`
        // (Algorithm 7), revision 6 hashes against qpdf's own `/O`.
        let owner =
            SecurityHandler::open(d.clone(), &id0, Some(OWNER)).unwrap_or_else(|e| panic!("{}: qpdf's owner password was refused: {e}", case.label));
        assert_eq!(owner.auth(), Auth::Owner, "{}", case.label);

        // A wrong password may never open it; and the key must be exactly as wide as the revision
        // promises - those five bytes are what makes a revision 2 truncation detectable at all.
        assert!(SecurityHandler::open(d.clone(), &id0, Some("not the password")).is_err(), "{}: a wrong password opened", case.label);
        assert_eq!(user.file_key().len(), case.key_len_bytes, "{}: file key width", case.label);
        assert_eq!(owner.file_key(), user.file_key(), "{}: whichever password opens it, one key", case.label);

        // A user session must not claim owner rights. We deliberately do NOT assert individual
        // permission flags here: which bit means "extract" moves between revision 2 and revisions 4
        // (qpdf wrote `/P = -28` for the export-grade file, where our accessor's bit-10 reading of
        // "extract for accessibility" is a reserved field), and pinning that mapping belongs with the
        // permissions tests, not with this oracle. What matters here is only that authentication, role
        // and key width agree with what an outside implementation produced.
        assert!(!user.permissions().owner, "{}: a user session claimed owner rights", case.label);
    }
}

#[test]
fn metadata_flag_from_outside_changes_the_file_key_it_certifies() {
    // `/EncryptMetadata` false folds four set bytes into a revision 4 key (Algorithm 2, step f) and
    // changes what revision 6 stores in `/U`. qpdf wrote both variants of each, so an implementation
    // that ignored the flag would give identical keys. Internally this is no longer untested -
    // `tests/proptests.rs` from #2358 varies the flag and asserts the keys differ - but internal tests
    // still share our derivation, so what these rows add is a second implementation's own numbers.
    for (enc_label, plain_label) in [("r4-aes128-meta-enc", "r4-aes128-meta-plain"), ("r6-aes256-meta-enc", "r6-aes256-meta-plain")] {
        let a = CASES.iter().find(|c| c.label == enc_label).expect("encrypted-metadata fixture present");
        let b = CASES.iter().find(|c| c.label == plain_label).expect("plain-metadata fixture present");

        let ha = SecurityHandler::open(a.dict(), &a.id0(), Some(USER)).unwrap_or_else(|e| panic!("{enc_label}: refused by our reader: {e}"));
        let hb = SecurityHandler::open(b.dict(), &b.id0(), Some(USER))
            .unwrap_or_else(|e| panic!("{plain_label}: refused by our reader - is step f being ignored? {e}"));

        assert_ne!(ha.file_key(), hb.file_key(), "{enc_label} and {plain_label} must not share a key: the metadata flag is hashed in");
        assert!(ha.encrypts_metadata(), "{enc_label} reports encrypted metadata");
        assert!(!hb.encrypts_metadata(), "{plain_label} reports plain metadata, as qpdf wrote it");
    }

    // The outsider's numbers must differ across revisions; equality here would mean a transcription
    // error, which would quietly void every assertion above.
    let r2 = CASES.iter().find(|c| c.r == 2).unwrap();
    let r3 = CASES.iter().find(|c| c.r == 3).unwrap();
    assert_ne!(hex(r2.u_hex), hex(r3.u_hex), "revision 2 and revision 3 /U differ because the key width differs");
    assert_eq!(r2.u_hex.len(), 64, "revision 2 /U is RC4 over the padding: 32 bytes");
    assert_eq!(r3.u_hex.len(), 64, "revision 3 /U is 32 bytes: sixteen derived, plus padding qpdf fills (our crafted row leaves it zero)");
}

#[test]
fn qpdf_perms_decode_to_the_shape_both_implementations_agree_on() {
    // `/Perms` is the one value in this dictionary that no test here verified before: `cos` writes it
    // out and reads it back, but nothing decrypts it. Under qpdf's own bytes we can - `/P`
    // little-endian, four set bytes, `T`/`F`, the `adb` sentinel (Algorithm 10 here; vendored lopdf
    // checks the same fields as its Algorithm 13, though only three bytes of `/P`).
    let mut checked = 0;
    for case in CASES.iter().filter(|c| !c.perms_hex.is_empty()) {
        let h = SecurityHandler::open(case.dict(), &case.id0(), Some(USER))
            .unwrap_or_else(|e| panic!("{}: refused before /Perms could be read: {e}", case.label));
        let open = aes_cbc_decrypt(h.file_key(), &[0u8; 16], &hex(case.perms_hex), false);

        assert_eq!(open.len(), 16, "{}: /Perms is one encrypted block", case.label);
        assert_eq!(&open[..4], &case.p.to_le_bytes(), "{}: /P must round trip against qpdf's bytes", case.label);
        assert_eq!(&open[4..8], &[0xFF; 4], "{}: the four bytes after /P are set", case.label);
        assert_eq!(open[8], if case.encrypt_metadata { b'T' } else { b'F' }, "{}: metadata byte as qpdf wrote it", case.label);
        assert_eq!(&open[9..12], b"adb", "{}: sentinel from qpdf's bytes", case.label);
        assert_ne!(&open[12..16], &[0u8; 4], "{}: qpdf's filler is not frozen at zero", case.label);
        checked += 1;
    }
    assert!(checked >= 2, "expected /Perms on both revision 6 cases; saw {checked}");
}
