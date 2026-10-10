//! PDF functions.
//!
//! PDF has the concept of functions, representing objects that take a certain number of values
//! as input, do some processing on them and then return some output.

mod type0;
mod type2;
mod type3;
mod type4;

use crate::function::type0::Type0;
use crate::function::type2::Type2;
use crate::function::type3::Type3;
use crate::function::type4::Type4;
use hayro_syntax::object::Dict;
use hayro_syntax::object::dict::keys::{DOMAIN, FUNCTION_TYPE, RANGE};
use hayro_syntax::object::{Object, dict_or_stream};
use smallvec::SmallVec;
use std::sync::Arc;

/// The input/output type of functions.
pub(crate) type Values = SmallVec<[f32; 4]>;
type TupleVec = SmallVec<[(f32, f32); 4]>;

// PdfCraft patch: PDF functions are untrusted object graphs. A normal color/transfer function
// has only a few nodes; cap both nesting and total construction work, including repeated
// references, before creating their children. These are resource limits, not PDF format limits.
const MAX_FUNCTION_DEPTH: usize = 64;
const MAX_FUNCTION_NODES: usize = 10_000;

struct ConstructionBudget {
    remaining: usize,
    // Stable backing-byte identity catches cycles without rejecting shared siblings. Looking
    // at pointers only compares identities; no pointer is ever dereferenced.
    path: Vec<(*const u8, usize)>,
}

#[derive(Debug)]
enum FunctionType {
    Type0(Type0),
    Type2(Type2),
    Type3(Type3),
    Type4(Type4),
}

/// A PDF function.
#[derive(Debug, Clone)]
pub struct Function(Arc<FunctionType>);

impl Function {
    /// Create a new function.
    pub fn new(obj: &Object<'_>) -> Option<Self> {
        Self::new_inner(
            obj,
            &mut ConstructionBudget {
                remaining: MAX_FUNCTION_NODES,
                path: Vec::new(),
            },
            0,
        )
    }

    fn new_inner(obj: &Object<'_>, budget: &mut ConstructionBudget, depth: usize) -> Option<Self> {
        if depth >= MAX_FUNCTION_DEPTH {
            warn!("PDF function nesting exceeds the limit of {MAX_FUNCTION_DEPTH} levels");
            return None;
        }
        if budget.remaining == 0 {
            warn!("PDF function construction exceeds the limit of {MAX_FUNCTION_NODES} nodes");
            return None;
        }
        budget.remaining -= 1;
        let (dict, stream) = dict_or_stream(obj)?;
        let identity = (dict.data().as_ptr(), dict.data().len());
        if budget.path.contains(&identity) {
            warn!("PDF function contains a cyclic stitching-function reference");
            return None;
        }
        budget.path.push(identity);
        let result = (|| {
            let function_type = match dict.get::<u8>(FUNCTION_TYPE)? {
                0 => FunctionType::Type0(Type0::new(stream?)?),
                2 => FunctionType::Type2(Type2::new(dict)?),
                3 => FunctionType::Type3(Type3::new(dict, budget, depth + 1)?),
                4 => FunctionType::Type4(Type4::new(stream?)?),
                _ => return None,
            };
            Some(Self(Arc::new(function_type)))
        })();
        budget.path.pop();
        result
    }

    /// Evaluate the function with the given input.
    pub fn eval(&self, input: Values) -> Option<Values> {
        match self.0.as_ref() {
            FunctionType::Type0(t0) => t0.eval(input),
            FunctionType::Type2(t2) => Some(t2.eval(*input.first()?)),
            FunctionType::Type3(t3) => t3.eval(*input.first()?),
            FunctionType::Type4(t4) => Some(t4.eval(input)?),
        }
    }
}

#[derive(Debug, Clone)]
struct Clamper {
    domain: TupleVec,
    range: Option<TupleVec>,
}

impl Clamper {
    fn new(dict: &Dict<'_>) -> Option<Self> {
        let domain = dict.get::<TupleVec>(DOMAIN)?;
        let range = dict.get::<TupleVec>(RANGE);

        Some(Self { domain, range })
    }

    fn clamp_input(&self, input: &mut [f32]) {
        if input.len() != self.domain.len() {
            warn!("the domain of the function didn't match the input arguments");
        }

        for ((min, max), val) in self.domain.iter().zip(input.iter_mut()) {
            *val = val.min(*max).max(*min);
        }
    }

    fn clamp_output(&self, output: &mut [f32]) {
        if let Some(range) = &self.range {
            if range.len() != output.len() {
                warn!("the range of the function didn't match the output arguments");
            }

            for ((min, max), val) in range.iter().zip(output.iter_mut()) {
                *val = val.min(*max).max(*min);
            }
        }
    }
}

/// Linearly interpolate the value `x`, assuming that it lies within the range `x_min` and `x_max`,
/// to the range `y_min` and `y_max`.
pub(crate) fn interpolate(x: f32, x_min: f32, x_max: f32, y_min: f32, y_max: f32) -> f32 {
    y_min + (x - x_min) * (y_max - y_min) / (x_max - x_min)
}
