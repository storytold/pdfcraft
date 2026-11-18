//! Contributor-original rectangle font, generated entirely in memory: no binary fixture.
fn be16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_be_bytes());
}
fn be32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_be_bytes());
}
fn number(n: i16) -> Vec<u8> {
    let mut v = vec![28];
    v.extend_from_slice(&n.to_be_bytes());
    v
}
fn index(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    be16(&mut out, items.len() as u16);
    if items.is_empty() {
        return out;
    }
    out.push(1);
    let mut at = 1;
    out.push(at);
    for i in items {
        at += i.len() as u8;
        out.push(at);
    }
    for i in items {
        out.extend(i);
    }
    out
}
pub fn cff_font() -> Vec<u8> {
    let name = index(&[b"RectFont".to_vec()]);
    let mut dict = Vec::new();
    for n in [0, 0, 500, 700] {
        dict.extend(number(n));
    }
    dict.push(5);
    // All CFF integers are fixed-width, so the CharStrings absolute offset is stable.
    let offset = 4 + name.len() + index(&[vec![0; dict.len() + 4]]).len() + 4;
    dict.extend(number(offset as i16));
    dict.push(17);
    let mut rectangle = number(600);
    rectangle.extend([139, 139, 21]);
    for n in [500, 0, 0, 700, -500, 0, 0, -700] {
        rectangle.extend(number(n));
    }
    rectangle.extend([5, 14]);
    let mut cff = vec![1, 0, 4, 4];
    cff.extend(name);
    cff.extend(index(&[dict]));
    cff.extend([0, 0, 0, 0]);
    cff.extend(index(&[
        vec![14],
        {
            let mut v = number(300);
            v.push(14);
            v
        },
        rectangle,
    ]));
    let mut head = vec![0; 54];
    head[0..4].copy_from_slice(&[0, 1, 0, 0]);
    head[12..16].copy_from_slice(&0x5f0f3cf5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[40..42].copy_from_slice(&500i16.to_be_bytes());
    head[42..44].copy_from_slice(&700i16.to_be_bytes());
    let mut hhea = vec![0; 36];
    hhea[0..4].copy_from_slice(&[0, 1, 0, 0]);
    hhea[4..6].copy_from_slice(&700i16.to_be_bytes());
    hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
    hhea[34..36].copy_from_slice(&3u16.to_be_bytes());
    let mut hmtx = Vec::new();
    for w in [600, 300, 600] {
        be16(&mut hmtx, w);
        be16(&mut hmtx, 0);
    }
    let mut maxp = vec![0, 0, 0x50, 0];
    be16(&mut maxp, 3);
    let mut cmap = vec![0, 0, 0, 1, 0, 3, 0, 10];
    be32(&mut cmap, 12);
    be16(&mut cmap, 12);
    be16(&mut cmap, 0);
    be32(&mut cmap, 40);
    be32(&mut cmap, 0);
    be32(&mut cmap, 2);
    for (cp, gid) in [(32, 1), (65, 2)] {
        be32(&mut cmap, cp);
        be32(&mut cmap, cp);
        be32(&mut cmap, gid);
    }
    let names = [(1, "Rect Font"), (2, "Regular"), (6, "RectFont-Regular"), (13, "MIT License")];
    let mut name = Vec::new();
    be16(&mut name, 0);
    be16(&mut name, names.len() as u16);
    be16(&mut name, (6 + 12 * names.len()) as u16);
    let mut data = Vec::new();
    for (id, text) in names {
        let bytes: Vec<_> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        for n in [3, 1, 0x409, id, bytes.len() as u16, data.len() as u16] {
            be16(&mut name, n);
        }
        data.extend(bytes);
    }
    name.extend(data);
    let tables = [(*b"CFF ", cff), (*b"cmap", cmap), (*b"head", head), (*b"hhea", hhea), (*b"hmtx", hmtx), (*b"maxp", maxp), (*b"name", name)];
    let mut out = b"OTTO".to_vec();
    be16(&mut out, 7);
    be16(&mut out, 64);
    be16(&mut out, 2);
    be16(&mut out, 48);
    let mut payload = Vec::new();
    let mut offset = 12 + 16 * tables.len();
    for (tag, bytes) in tables {
        out.extend(tag);
        let mut padded = bytes.clone();
        while padded.len() % 4 != 0 {
            padded.push(0);
        }
        let sum = padded.as_chunks::<4>().0.iter().fold(0u32, |s, b| s.wrapping_add(u32::from_be_bytes(*b)));
        be32(&mut out, sum);
        be32(&mut out, offset as u32);
        be32(&mut out, bytes.len() as u32);
        offset += padded.len();
        payload.extend(padded);
    }
    out.extend(payload);
    out
}
