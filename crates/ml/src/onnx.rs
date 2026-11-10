//! An ONNX model's nodes and float weights, read with `rten-onnx` (rten's own parser, already in
//! the tree through rten's `onnx_format`). Only what the GPU executor needs is kept: nodes with
//! their attributes, float tensors (initializers and Constant nodes) and integer constants.

use std::collections::HashMap;

use rten_onnx::onnx::{AttributeProto, ModelProto, TensorProto};

/// The most values one tensor may hold (2^28 floats, 1 GiB): a model file claiming more is
/// refused rather than allocated.
const MAX_VALUES: usize = 1 << 28;

#[derive(Debug, thiserror::Error)]
pub enum OnnxError {
    #[error("{0}: {1}")]
    Read(String, String),
    #[error("model: {0}")]
    Model(String),
}

/// A float tensor: its dimensions and row-major data.
#[derive(Clone, Debug)]
pub struct Weight {
    pub dims: Vec<usize>,
    pub data: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub op: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    attrs: Vec<AttributeProto>,
}

impl Node {
    fn attr(&self, name: &str) -> Option<&AttributeProto> {
        self.attrs.iter().find(|a| a.name.as_deref() == Some(name))
    }

    pub fn ints(&self, name: &str) -> Option<Vec<i64>> {
        self.attr(name).map(|a| a.ints.clone())
    }

    pub fn int(&self, name: &str) -> Option<i64> {
        self.attr(name).and_then(|a| a.i)
    }

    pub fn string(&self, name: &str) -> Option<String> {
        self.attr(name).and_then(|a| a.s.clone())
    }

    /// The first input, or "" for a node without one.
    pub fn input(&self, k: usize) -> &str {
        self.inputs.get(k).map_or("", String::as_str)
    }

    /// The first output, or "".
    pub fn output(&self) -> &str {
        self.outputs.first().map_or("", String::as_str)
    }
}

pub struct Onnx {
    pub nodes: Vec<Node>,
    pub weights: HashMap<String, Weight>,
    /// Integer constants (Slice starts/ends/axes and the like).
    pub ints: HashMap<String, Vec<i64>>,
    pub input: String,
    pub output: String,
}

const FLOAT: i32 = 1;
const INT64: i32 = 7;

enum Values {
    Float(Vec<usize>, Vec<f32>),
    Int(Vec<i64>),
    Other,
}

fn values(t: &TensorProto) -> Result<Values, OnnxError> {
    let bad = |m: &str| OnnxError::Model(format!("{:?}: {m}", t.name));
    let dims: Vec<usize> = t.dims.iter().map(|&d| usize::try_from(d).map_err(|_| bad("negative dimension"))).collect::<Result<_, _>>()?;
    let count = dims.iter().try_fold(1usize, |a, &d| a.checked_mul(d)).filter(|&c| c <= MAX_VALUES).ok_or_else(|| bad("too many values"))?;
    let raw = t.raw_data.as_ref().map(|r| r.borrow().clone());
    match t.data_type.map(|d| d.0) {
        Some(FLOAT) => {
            let data: Vec<f32> = match raw {
                Some(bytes) => bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect(),
                None => t.float_data.clone(),
            };
            if data.len() != count {
                return Err(bad(&format!("{} values for {count}", data.len())));
            }
            Ok(Values::Float(dims, data))
        }
        Some(INT64) => {
            let data: Vec<i64> = match raw {
                Some(bytes) => bytes.as_chunks::<8>().0.iter().map(|c| i64::from_le_bytes(*c)).collect(),
                None => t.int64_data.clone(),
            };
            if data.len() != count {
                return Err(bad(&format!("{} values for {count}", data.len())));
            }
            Ok(Values::Int(data))
        }
        _ => Ok(Values::Other),
    }
}

impl Onnx {
    pub fn load(path: &std::path::Path) -> Result<Self, OnnxError> {
        let bytes = std::fs::read(path).map_err(|e| OnnxError::Read(path.display().to_string(), e.to_string()))?;
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, OnnxError> {
        let model = ModelProto::parse_buf(bytes).map_err(|e| OnnxError::Model(format!("{e:?}")))?;
        let graph = model.graph.ok_or_else(|| OnnxError::Model("no graph".into()))?;
        let (mut weights, mut ints) = (HashMap::new(), HashMap::new());
        let mut add = |name: String, t: &TensorProto| -> Result<(), OnnxError> {
            match values(t)? {
                Values::Float(dims, data) => {
                    weights.insert(name, Weight { dims, data });
                }
                Values::Int(i) => {
                    ints.insert(name, i);
                }
                Values::Other => {}
            }
            Ok(())
        };
        for t in &graph.initializer {
            add(t.name.clone().unwrap_or_default(), t)?;
        }
        let mut nodes = Vec::new();
        for n in &graph.node {
            let op = n.op_type.clone().unwrap_or_default();
            // Constants become weights or integer lists, like initializers.
            if op == "Constant" {
                if let (Some(out), Some(t)) = (n.output.first(), n.attribute.iter().find_map(|a| a.t.as_ref())) {
                    add(out.clone(), t)?;
                }
                continue;
            }
            if n.output.is_empty() {
                return Err(OnnxError::Model(format!("{op} node without output")));
            }
            nodes.push(Node { op, inputs: n.input.clone(), outputs: n.output.clone(), attrs: n.attribute.clone() });
        }
        let input = graph
            .input
            .iter()
            .filter_map(|i| i.name.clone())
            .find(|n| !weights.contains_key(n))
            .ok_or_else(|| OnnxError::Model("no graph input".into()))?;
        let output = graph.output.first().and_then(|o| o.name.clone()).ok_or_else(|| OnnxError::Model("no graph output".into()))?;
        Ok(Self { nodes, weights, ints, input, output })
    }

    pub fn weight(&self, name: &str) -> Result<&Weight, OnnxError> {
        self.weights.get(name).ok_or_else(|| OnnxError::Model(format!("missing weight {name}")))
    }

    /// How many nodes read `name`.
    pub fn readers(&self, name: &str) -> usize {
        self.nodes.iter().filter(|n| n.inputs.iter().any(|i| i == name)).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Garbage is an error, not a panic. (Not tested: 11 or more bytes of 0xff make rten-onnx
    /// 0.26's parser loop without end, robertknight/rten#1537; the shipped models are SHA-256
    /// checked, and rten's own loader on the CPU path parses the same way.)
    #[test]
    fn a_broken_model_is_an_error() {
        for bytes in [&b""[..], b"\x00\x01\x02", b"not a model at all"] {
            assert!(Onnx::parse(bytes).is_err(), "{bytes:?}");
        }
    }
}
