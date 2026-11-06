//! The GOST R 34.11-2012 hash function ("Streebog"), per RFC 6986.
//!
//! A plain byte-level implementation of the standard's transformations, with
//! the round-key schedule and both digest widths (256 and 512 bits). It is a
//! format, not a certified cryptographic module (СКЗИ): the certified signing
//! operation stays in the token, and this module only computes the message
//! digest that PKCS#11 GOST mechanisms take as input and that the CMS
//! `digestAlgorithm` field names (design overview in
//! [pdfcraft#5468](https://github.com/storytold/pdfcraft/issues/5468)).
//!
//! The 8×256 multiplication tables for the linear transform are derived from
//! the standard's substitution π, permutation τ and matrix A at compile time
//! (`const`), so the source carries the RFC's numbers instead of 16 KiB of
//! precomputed tables.

/// The substitution π (RFC 6986 §6.2, "Pi").
const PI: [u8; 256] = [
    252, 238, 221, 17, 207, 110, 49, 22, 251, 196, 250, 218, 35, 197, 4, 77, 233, 119, 240, 219, 147, 46, 153, 186, 23, 54, 241, 187, 20, 205, 95,
    193, 249, 24, 101, 90, 226, 92, 239, 33, 129, 28, 60, 66, 139, 1, 142, 79, 5, 132, 2, 174, 227, 106, 143, 160, 6, 11, 237, 152, 127, 212, 211,
    31, 235, 52, 44, 81, 234, 200, 72, 171, 242, 42, 104, 162, 253, 58, 206, 204, 181, 112, 14, 86, 8, 12, 118, 18, 191, 114, 19, 71, 156, 183, 93,
    135, 21, 161, 150, 41, 16, 123, 154, 199, 243, 145, 120, 111, 157, 158, 178, 177, 50, 117, 25, 61, 255, 53, 138, 126, 109, 84, 198, 128, 195,
    189, 13, 87, 223, 245, 36, 169, 62, 168, 67, 201, 215, 121, 214, 246, 124, 34, 185, 3, 224, 15, 236, 222, 122, 148, 176, 188, 220, 232, 40, 80,
    78, 51, 10, 74, 167, 151, 96, 115, 30, 0, 98, 68, 26, 184, 56, 130, 100, 159, 38, 65, 173, 69, 70, 146, 39, 94, 85, 47, 140, 163, 165, 125, 105,
    213, 149, 59, 7, 88, 179, 64, 134, 172, 29, 247, 48, 55, 107, 228, 136, 217, 231, 137, 225, 27, 131, 73, 76, 63, 248, 254, 141, 83, 170, 144,
    202, 216, 133, 97, 32, 113, 103, 164, 45, 43, 9, 91, 203, 155, 37, 208, 190, 229, 108, 82, 89, 166, 116, 210, 230, 244, 180, 192, 209, 102, 175,
    194, 57, 75, 99, 182,
];

/// The permutation τ (RFC 6986 §6.3, "Tau").
const TAU: [usize; 64] = [
    0, 8, 16, 24, 32, 40, 48, 56, 1, 9, 17, 25, 33, 41, 49, 57, 2, 10, 18, 26, 34, 42, 50, 58, 3, 11, 19, 27, 35, 43, 51, 59, 4, 12, 20, 28, 36, 44,
    52, 60, 5, 13, 21, 29, 37, 45, 53, 61, 6, 14, 22, 30, 38, 46, 54, 62, 7, 15, 23, 31, 39, 47, 55, 63,
];

/// A hexadecimal digit; anything else reads as 0 (the inputs below are fixed).
const fn nib(c: u8) -> u64 {
    match c {
        b'0'..=b'9' => (c - b'0') as u64,
        b'a'..=b'f' => (c - b'a' + 10) as u64,
        _ => 0,
    }
}

/// One row of the linear-transform matrix A (RFC 6986 §6.4), read big-endian.
const fn a_row(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut v = 0;
    let mut i = 0;
    while i < 16 {
        v = (v << 4) | nib(b[i]);
        i += 1;
    }
    v
}

/// The 64 rows of the matrix A, in the RFC's order.
const A: [u64; 64] = [
    a_row("8e20faa72ba0b470"),
    a_row("47107ddd9b505a38"),
    a_row("ad08b0e0c3282d1c"),
    a_row("d8045870ef14980e"),
    a_row("6c022c38f90a4c07"),
    a_row("3601161cf205268d"),
    a_row("1b8e0b0e798c13c8"),
    a_row("83478b07b2468764"),
    a_row("a011d380818e8f40"),
    a_row("5086e740ce47c920"),
    a_row("2843fd2067adea10"),
    a_row("14aff010bdd87508"),
    a_row("0ad97808d06cb404"),
    a_row("05e23c0468365a02"),
    a_row("8c711e02341b2d01"),
    a_row("46b60f011a83988e"),
    a_row("90dab52a387ae76f"),
    a_row("486dd4151c3dfdb9"),
    a_row("24b86a840e90f0d2"),
    a_row("125c354207487869"),
    a_row("092e94218d243cba"),
    a_row("8a174a9ec8121e5d"),
    a_row("4585254f64090fa0"),
    a_row("accc9ca9328a8950"),
    a_row("9d4df05d5f661451"),
    a_row("c0a878a0a1330aa6"),
    a_row("60543c50de970553"),
    a_row("302a1e286fc58ca7"),
    a_row("18150f14b9ec46dd"),
    a_row("0c84890ad27623e0"),
    a_row("0642ca05693b9f70"),
    a_row("0321658cba93c138"),
    a_row("86275df09ce8aaa8"),
    a_row("439da0784e745554"),
    a_row("afc0503c273aa42a"),
    a_row("d960281e9d1d5215"),
    a_row("e230140fc0802984"),
    a_row("71180a8960409a42"),
    a_row("b60c05ca30204d21"),
    a_row("5b068c651810a89e"),
    a_row("456c34887a3805b9"),
    a_row("ac361a443d1c8cd2"),
    a_row("561b0d22900e4669"),
    a_row("2b838811480723ba"),
    a_row("9bcf4486248d9f5d"),
    a_row("c3e9224312c8c1a0"),
    a_row("effa11af0964ee50"),
    a_row("f97d86d98a327728"),
    a_row("e4fa2054a80b329c"),
    a_row("727d102a548b194e"),
    a_row("39b008152acb8227"),
    a_row("9258048415eb419d"),
    a_row("492c024284fbaec0"),
    a_row("aa16012142f35760"),
    a_row("550b8e9e21f7a530"),
    a_row("a48b474f9ef5dc18"),
    a_row("70a6a56e2440598e"),
    a_row("3853dc371220a247"),
    a_row("1ca76e95091051ad"),
    a_row("0edd37c48a08a6d8"),
    a_row("07e095624504536c"),
    a_row("8d70c431ac02a736"),
    a_row("c83862965601dd1b"),
    a_row("641c314b2b8ee083"),
];

/// An iteration constant C[1..=12] (RFC 6986 §6.5) as a 512-bit block. The RFC
/// writes vectors most-significant byte first; the byte arrays this module
/// computes with (as every Streebog implementation, from pygost to gost-engine)
/// keep them the other way round, so the string is read in from the right.
const fn c_block(s: &str) -> [u8; 64] {
    let b = s.as_bytes();
    let mut out = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        out[i] = ((nib(b[126 - 2 * i]) << 4) | nib(b[127 - 2 * i])) as u8;
        i += 1;
    }
    out
}

/// C[i] from one 128-hex-digit string (four RFC lines, concatenated with
/// `concat!` so the source keeps the RFC's line breaks in the comments).
const fn c_const(s: &'static str) -> [u8; 64] {
    c_block(s)
}

const C: [[u8; 64]; 12] = [
    c_const(concat!(
        "b1085bda1ecadae9ebcb2f81c0657c1f",
        "2f6a76432e45d016714eb88d7585c4fc",
        "4b7ce09192676901a2422a08a460d315",
        "05767436cc744d23dd806559f2a64507",
    )),
    c_const(concat!(
        "6fa3b58aa99d2f1a4fe39d460f70b5d7",
        "f3feea720a232b9861d55e0f16b50131",
        "9ab5176b12d699585cb561c2db0aa7ca",
        "55dda21bd7cbcd56e679047021b19bb7",
    )),
    c_const(concat!(
        "f574dcac2bce2fc70a39fc286a3d8435",
        "06f15e5f529c1f8bf2ea7514b1297b7b",
        "d3e20fe490359eb1c1c93a376062db09",
        "c2b6f443867adb31991e96f50aba0ab2",
    )),
    c_const(concat!(
        "ef1fdfb3e81566d2f948e1a05d71e4dd",
        "488e857e335c3c7d9d721cad685e353f",
        "a9d72c82ed03d675d8b71333935203be",
        "3453eaa193e837f1220cbebc84e3d12e",
    )),
    c_const(concat!(
        "4bea6bacad4747999a3f410c6ca92363",
        "7f151c1f1686104a359e35d7800fffbd",
        "bfcd1747253af5a3dfff00b723271a16",
        "7a56a27ea9ea63f5601758fd7c6cfe57",
    )),
    c_const(concat!(
        "ae4faeae1d3ad3d96fa4c33b7a3039c0",
        "2d66c4f95142a46c187f9ab49af08ec6",
        "cffaa6b71c9ab7b40af21f66c2bec6b6",
        "bf71c57236904f35fa68407a46647d6e",
    )),
    c_const(concat!(
        "f4c70e16eeaac5ec51ac86febf240954",
        "399ec6c7e6bf87c9d3473e33197a93c9",
        "0992abc52d822c3706476983284a0504",
        "3517454ca23c4af38886564d3a14d493",
    )),
    c_const(concat!(
        "9b1f5b424d93c9a703e7aa020c6e4141",
        "4eb7f8719c36de1e89b4443b4ddbc49a",
        "f4892bcb929b069069d18d2bd1a5c42f",
        "36acc2355951a8d9a47f0dd4bf02e71e",
    )),
    c_const(concat!(
        "378f5a541631229b944c9ad8ec165fde",
        "3a7d3a1b258942243cd955b7e00d0984",
        "800a440bdbb2ceb17b2b8a9aa6079c54",
        "0e38dc92cb1f2a607261445183235adb",
    )),
    c_const(concat!(
        "abbedea680056f52382ae548b2e4f3f3",
        "8941e71cff8a78db1fffe18a1b336103",
        "9fe76702af69334b7a1e6c303b7652f4",
        "3698fad1153bb6c374b4c7fb98459ced",
    )),
    c_const(concat!(
        "7bcd9ed0efc889fb3002c6cd635afe94",
        "d8fa6bbbebab07612001802114846679",
        "8a1d71efea48b9caefbacd1d7d476e98",
        "dea2594ac06fd85d6bcaa4cd81f32d1b",
    )),
    c_const(concat!(
        "378ee767f11631bad21380b00449b17a",
        "cda43c32bcdf1d77f82012d430219f9b",
        "5d80ef9d1891cc86e71da4aa88e12852",
        "faf417d5d9b21b9948bc924af11bd720",
    )),
];

/// For each byte position k in a 64-bit word and each byte value v, the linear
/// transform's contribution: A-rows XORed for the set bits of v. Derived from
/// π, τ and A at compile time instead of carrying 16 KiB of tables in source.
const fn build_lpt() -> [[u64; 256]; 8] {
    let mut tables = [[0u64; 256]; 8];
    let mut k = 0;
    while k < 8 {
        let mut v = 0;
        while v < 256 {
            let mut acc = 0u64;
            let mut r = 0;
            while r < 8 {
                if v & (1 << r) != 0 {
                    acc ^= A[63 - 8 * k - r];
                }
                r += 1;
            }
            tables[k][v] = acc;
            v += 1;
        }
        k += 1;
    }
    tables
}

const LPT: [[u64; 256]; 8] = build_lpt();

fn xor(a: &mut [u8; 64], b: &[u8; 64]) {
    for i in 0..64 {
        a[i] ^= b[i];
    }
}

/// Addition modulo 2^512 over little-endian bytes.
fn add512(a: &[u8; 64], b: &[u8; 64]) -> [u8; 64] {
    let mut out = [0u8; 64];
    let mut carry = 0usize;
    for i in 0..64 {
        carry = a[i] as usize + b[i] as usize + (carry >> 8);
        out[i] = (carry & 0xff) as u8;
    }
    out
}

/// S, then P, then L over a 512-bit block (RFC 6986 §7).
fn lps(data: &mut [u8; 64]) {
    let mut s = [0u8; 64];
    for (i, byte) in data.iter().enumerate() {
        s[TAU[i]] = PI[*byte as usize];
    }
    for (w, word) in s.as_chunks::<8>().0.iter().enumerate() {
        let mut acc = 0u64;
        for (k, &b) in word.iter().enumerate() {
            acc ^= LPT[k][b as usize];
        }
        data[w * 8..w * 8 + 8].copy_from_slice(&acc.to_le_bytes());
    }
}

/// The block cipher's round function E (RFC 6986 §8): twelve LPSX rounds with
/// a key schedule stepped by the iteration constants, then a final XOR.
fn e(key: &mut [u8; 64], msg: &mut [u8; 64]) {
    for c in C.iter().take(12) {
        xor(msg, key);
        lps(msg);
        xor(key, c);
        lps(key);
    }
    xor(msg, key);
}

/// The compression function g_N (RFC 6986 §8).
fn g_n(n: u64, h: &[u8; 64], msg: &[u8; 64]) -> [u8; 64] {
    let mut key = *h;
    let len = n.to_le_bytes();
    for (i, &b) in len.iter().enumerate() {
        key[i] ^= b;
    }
    lps(&mut key);
    let mut inner = *msg;
    e(&mut key, &mut inner);
    xor(&mut inner, h);
    xor(&mut inner, msg);
    inner
}

/// The hash of `data` with the given 512-bit initial value; the caller trims
/// the result for the 256-bit width (RFC 6986 §9).
fn hash_with(data: &[u8], iv: [u8; 64]) -> [u8; 64] {
    let mut h = iv;
    let mut sum = [0u8; 64];
    let mut bits: u64 = 0;
    let (blocks, rest) = data.as_chunks::<64>();
    for block in blocks {
        let mut m = [0u8; 64];
        m.copy_from_slice(block);
        h = g_n(bits, &h, &m);
        bits = bits.wrapping_add(512);
        sum = add512(&sum, &m);
    }
    // Padding: a 1 bit after the message, zero bits up to the block edge; the
    // block then goes through the compression function with the message length.
    let mut m = [0u8; 64];
    m[..rest.len()].copy_from_slice(rest);
    m[rest.len()] = 1;
    h = g_n(bits, &h, &m);
    bits = bits.wrapping_add(rest.len() as u64 * 8);
    sum = add512(&sum, &m);
    let mut len = [0u8; 64];
    len[..8].copy_from_slice(&bits.to_le_bytes());
    h = g_n(0, &h, &len);
    h = g_n(0, &h, &sum);
    h
}

/// The 256-bit GOST R 34.11-2012 digest of `data`.
pub fn digest_256(data: &[u8]) -> [u8; 32] {
    let h = hash_with(data, [1u8; 64]);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h[32..]);
    out
}

/// The 512-bit GOST R 34.11-2012 digest of `data`.
pub fn digest_512(data: &[u8]) -> [u8; 64] {
    hash_with(data, [0u8; 64])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6986 writes test vectors most-significant byte first, while this
    /// module (like every Streebog implementation, from pygost to gost-engine)
    /// works on the reversed bytes; the RFC messages and digests below are
    /// stored the way the standard prints them, and converted on use.
    fn display_bytes(hex: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(hex.len() / 2);
        for pair in hex.as_bytes().rchunks(2) {
            out.push(((nib(pair[0]) << 4) | nib(pair[1])) as u8);
        }
        out
    }

    /// Digest values quoted in the implementations' own byte order.
    fn raw_bytes(hex: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(hex.len() / 2);
        for pair in hex.as_bytes().chunks(2) {
            out.push(((nib(pair[0]) << 4) | nib(pair[1])) as u8);
        }
        out
    }

    /// M1 from RFC 6986 §10.1: 63 bytes.
    const M1: &str = "323130393837363534333231303938373635343332313039383736353433323130393837363534333231303938373635343332313039383736353433323130";
    /// M2 from RFC 6986 §10.2: 72 bytes.
    const M2: &str = "fbe2e5f0eee3c820fbeafaebef20fffbf0e1e0f0f520e0ed20e8ece0ebe5f0f2f120fff0eeec20f120faf2fee5e2202ce8f6f3ede220e8e6eee1e8f0f2d1202ce8f0f2e5e220e5d1";

    #[test]
    fn rfc_6986_vectors() {
        let m1 = display_bytes(M1);
        let m2 = display_bytes(M2);
        let cases: &[(&Vec<u8>, &str, &str)] = &[
            (
                &m1,
                "486f64c1917879417fef082b3381a4e211c324f074654c38823a7b76f830ad00fa1fbae42b1285c0352f227524bc9ab16254288dd6863dccd5b9f54a1ad0541b",
                "00557be5e584fd52a449b16b0251d05d27f94ab76cbaa6da890b59d8ef1e159d",
            ),
            (
                &m2,
                "28fbc9bada033b1460642bdcddb90c3fb3e56c497ccd0f62b8a2ad4935e85f037613966de4ee00531ae60f3b5a47f8dae06915d5f2f194996fcabf2622e6881e",
                "508f7e553c06501d749a66fc28c6cac0b005746d97537fa85d9e40904efed29d",
            ),
        ];
        for (msg, want512, want256) in cases {
            assert_eq!(&digest_512(msg)[..], &display_bytes(want512)[..]);
            assert_eq!(&digest_256(msg)[..], &display_bytes(want256)[..]);
        }
    }

    #[test]
    fn empty_message() {
        // The 512-bit digest of the empty message (h starts at 0^512) and the
        // 256-bit one (h starts at (0x01)^64), as pinned by the reference
        // implementation behind the RFC vectors above.
        assert_eq!(
            &digest_512(&[])[..],
            &raw_bytes(
                "8e945da209aa869f0455928529bcae4679e9873ab707b55315f56ceb98bef0a7362f715528356ee83cda5f2aac4c6ad2ba3a715c1bcd81cb8e9f90bf4c1c1a8a"
            )[..]
        );
        assert_eq!(&digest_256(&[])[..], &raw_bytes("3f539a213e97c802cc229d474c6aa32a825a360b2a933a949fd925208d9ce1bb")[..]);
    }

    #[test]
    fn short_message() {
        // ASCII input goes in unchanged, in its natural byte order.
        assert_eq!(&digest_256(b"foobar")[..], &raw_bytes("e3c9fd89226d93b489a9fe27d686806e24a514e3787bca053c698ec4616ceb78")[..]);
    }
}
