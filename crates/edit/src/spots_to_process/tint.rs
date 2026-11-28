//! A PDF function (ISO 32000-1 §7.10) read from the document and evaluated by the renderer's own
//! evaluator, so a converted colour is the one the page already shows on screen.
//!
//! The evaluator reads one self-contained object, so the function is written out with every
//! reference replaced by what it names. A function stream's data is written decoded. A stitching
//! function whose pieces are streams can't be written that way and is refused.

use craft_pdf::interpret::Function;
use craft_pdf::syntax::object::{FromBytes, Object as Parsed};
use pdfcraft_cos::{Dict, Document, Object, Stream};

/// Nesting followed while writing a function out.
const MAX_DEPTH: usize = 16;
/// Values written out for one function.
const MAX_VALUES: usize = 1 << 16;
/// The most a function stream's data may decode to.
const MAX_DATA: usize = 16 << 20;

/// A function and the number of inputs its `/Domain` gives it.
pub(super) struct Func {
    f: Function,
    inputs: usize,
}

impl Func {
    /// The function `obj` names, or why it can't be read.
    pub(super) fn load(doc: &Document, obj: &Object) -> Result<Self, String> {
        let mut budget = MAX_VALUES;
        let whole = inline(doc, obj, 0, &mut budget, true)?;
        let inputs = whole.as_dict().and_then(|d| d.get(b"Domain")).and_then(Object::as_array).map_or(0, |d| d.len() / 2);
        let mut bytes = Vec::new();
        pdfcraft_cos::serialize(&whole, &mut bytes);
        let parsed = Parsed::from_bytes(&bytes).ok_or("it is not a function")?;
        let f = Function::new(&parsed).ok_or("it is not a function the renderer evaluates")?;
        Ok(Self { f, inputs })
    }

    pub(super) fn inputs(&self) -> usize {
        self.inputs
    }

    /// The outputs for `input`, or `None` when the function fails on it.
    pub(super) fn eval(&self, input: &[f64]) -> Option<Vec<f64>> {
        let out = self.f.eval(input.iter().map(|v| *v as f32).collect())?;
        Some(out.iter().map(|v| f64::from(*v)).collect())
    }
}

/// `o` with every reference replaced by its object, and a stream (only allowed as the function
/// itself, `top`) with its data decoded.
fn inline(doc: &Document, o: &Object, depth: usize, budget: &mut usize, top: bool) -> Result<Object, String> {
    if depth > MAX_DEPTH {
        return Err("it nests too deeply".into());
    }
    *budget = budget.checked_sub(1).ok_or("it is too large")?;
    Ok(match o {
        Object::Ref(r) => return inline(doc, &doc.get(*r), depth + 1, budget, top),
        Object::Array(items) => Object::Array(items.iter().map(|i| inline(doc, i, depth + 1, budget, false)).collect::<Result<_, _>>()?),
        Object::Dict(d) => Object::Dict(entries(doc, d, depth, budget)?),
        Object::Stream(s) if top => {
            let data = s.decoded_within(MAX_DATA).map_err(|e| format!("its data cannot be read ({e})"))?;
            let mut dict = entries(doc, &s.dict, depth, budget)?;
            for k in [&b"Length"[..], b"Filter", b"DecodeParms", b"DL"] {
                dict.remove(k);
            }
            Object::Stream(Stream::from_raw(dict, data))
        }
        Object::Stream(_) => return Err("one of its pieces is a stream, which is not read inside another function here".into()),
        other => other.clone(),
    })
}

fn entries(doc: &Document, d: &Dict, depth: usize, budget: &mut usize) -> Result<Dict, String> {
    let mut out = Dict::new();
    for (k, v) in d.iter() {
        out.set(k.clone(), inline(doc, v, depth + 1, budget, false)?);
    }
    Ok(out)
}
