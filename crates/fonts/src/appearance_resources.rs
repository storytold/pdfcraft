//! Generated Type 3 resources staged independently of any working/display document.
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// A generated appearance with self-contained font resources, reusable across document copies.
#[derive(Clone, Debug)]
pub struct PreparedAppearance {
    stream: Stream,
    objects: Vec<(ObjRef, Arc<Object>)>,
}

impl PreparedAppearance {
    /// Keep an ordinary appearance whose references already belong to the target document.
    pub fn plain(stream: Stream) -> Self {
        Self { stream, objects: Vec::new() }
    }

    /// Shape into a fresh font-resource arena. Failure cannot modify the caller's overlay.
    /// Only font objects may be added to the arena. Non-font resources in the returned
    /// stream (such as picture icons) must already reference the target document.
    pub fn draw(draw: impl FnOnce(&mut Document) -> Result<Stream, String>) -> Result<Self, String> {
        let mut fonts = Document::new_empty();
        let initial: HashSet<_> = fonts.object_numbers().into_iter().collect();
        let stream = draw(&mut fonts)?;
        let objects: Vec<_> = fonts
            .object_numbers()
            .into_iter()
            .filter(|n| !initial.contains(n))
            .map(|num| {
                let r = ObjRef { num, generation: fonts.generation(num) };
                (r, fonts.get(r))
            })
            .collect();
        let known: HashSet<_> = objects.iter().map(|(r, _)| *r).collect();
        let font = stream.dict.get(b"Resources").and_then(Object::as_dict).and_then(|r| r.get(b"Font"));
        if !objects.iter().all(|(_, obj)| local_refs(obj, &known, 0)) || !font.is_none_or(|font| local_refs(font, &known, 0)) {
            return Err("generated appearance resources contain an invalid reference or exceed the nesting limit".into());
        }
        Ok(Self { stream, objects })
    }

    /// Rebind every generated font reference to this copy. Original icon references stay intact.
    /// All fallible shaping/validation finishes before any object is appended.
    pub fn install(&self, doc: &mut Document) -> Stream {
        if self.objects.is_empty() {
            return self.stream.clone();
        }
        let placed: Vec<_> = self.objects.iter().map(|(r, _)| (*r, doc.add(Object::Null))).collect();
        let map: HashMap<_, _> = placed.iter().copied().collect();
        for ((_, object), (_, target)) in self.objects.iter().zip(placed) {
            let mut object = (**object).clone();
            rebind(&mut object, &map, 0);
            doc.set(target, object);
        }
        let mut stream = self.stream.clone();
        if let Some(font) = stream.dict.get_mut(b"Resources").and_then(Object::as_dict_mut).and_then(|r| r.get_mut(b"Font")) {
            rebind(font, &map, 0);
        }
        stream
    }

    /// Conservative retained-byte estimate for bounded appearance caches.
    pub fn bytes(&self) -> usize {
        if self.objects.is_empty() {
            return self.stream.raw.len();
        }
        // Conservative allowances for fixed-format Type 3 dictionaries (at most 240 glyphs).
        self.objects.iter().fold(self.stream.raw.len().saturating_add(65_536), |size, (_, object)| {
            size.saturating_add(match &**object {
                Object::Stream(s) => s.raw.len().saturating_add(1024),
                _ => 65_536,
            })
        })
    }
}

/// These are generated objects, never source dictionaries. Validate their bounded trees first.
fn local_refs(object: &Object, known: &HashSet<ObjRef>, depth: usize) -> bool {
    if depth >= 32 {
        return false;
    }
    match object {
        Object::Ref(r) => known.contains(r),
        Object::Array(a) => a.iter().all(|o| local_refs(o, known, depth + 1)),
        Object::Dict(d) => d.iter().all(|(_, o)| local_refs(o, known, depth + 1)),
        Object::Stream(s) => s.dict.iter().all(|(_, o)| local_refs(o, known, depth + 1)),
        _ => true,
    }
}

fn rebind_dict(dict: &mut Dict, map: &HashMap<ObjRef, ObjRef>, depth: usize) {
    let keys: Vec<_> = dict.iter().map(|(key, _)| key.clone()).collect();
    for key in keys {
        if let Some(value) = dict.get_mut(&key) {
            rebind(value, map, depth + 1);
        }
    }
}

fn rebind(object: &mut Object, map: &HashMap<ObjRef, ObjRef>, depth: usize) {
    // draw() already proved the depth and that the mapping covers every generated reference.
    if depth >= 32 {
        return;
    }
    match object {
        Object::Ref(r) => {
            if let Some(new) = map.get(r) {
                *r = *new;
            }
        }
        Object::Array(a) => {
            for o in a {
                rebind(o, map, depth + 1);
            }
        }
        Object::Dict(d) => rebind_dict(d, map, depth),
        Object::Stream(s) => rebind_dict(&mut s.dict, map, depth),
        _ => {}
    }
}
