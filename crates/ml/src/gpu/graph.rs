//! An executor for the convolutional ONNX graphs of the ocrs models (NCHW tensors): Conv (dense
//! as an implicit GEMM, depthwise, single-input-channel direct with a fused MaxPool),
//! ConvTranspose, MaxPool, Slice, Concat, zero Pad, and Relu/Sigmoid fused into the
//! convolution before them.

use std::collections::HashMap;

use super::{Buf, Gpu, GpuError, params, to_u32};
use crate::onnx::{Node, Onnx};

/// A 4D NCHW tensor on the GPU.
#[derive(Clone)]
pub(crate) struct Tensor {
    pub buf: Buf,
    pub shape: [usize; 4],
}

impl Tensor {
    pub fn len(&self) -> Result<usize, GpuError> {
        count(&self.shape)
    }
}

/// The number of values in `shape`, refusing overflow.
pub(crate) fn count(shape: &[usize]) -> Result<usize, GpuError> {
    shape.iter().try_fold(1usize, |a, &d| a.checked_mul(d)).ok_or_else(|| GpuError::TooLarge(format!("{shape:?}")))
}

/// The output length of a convolution or pool over `size` with `pad` on each side.
fn out_len(size: usize, pad_total: usize, k: usize, stride: usize) -> Result<usize, GpuError> {
    let span =
        size.checked_add(pad_total).and_then(|s| s.checked_sub(k)).ok_or_else(|| GpuError::Model(format!("kernel {k} larger than input {size}")))?;
    Ok(span / stride.max(1) + 1)
}

enum Op {
    Conv {
        w: Buf,
        b: Buf,
        cin: usize,
        cout: usize,
        kh: usize,
        kw: usize,
        pad: usize,
        stride: usize,
        act: u32,
        fast: bool,
    },
    Depthwise {
        w: Buf,
        b: Buf,
        kh: usize,
        kw: usize,
        pad: usize,
        stride: usize,
        act: u32,
    },
    /// One input channel, stride 1, with an optional fused MaxPool (kernel = stride).
    Direct {
        w: Buf,
        b: Buf,
        cout: usize,
        kh: usize,
        kw: usize,
        pad: usize,
        act: u32,
        pool: (usize, usize),
    },
    ConvT {
        w: Buf,
        b: Buf,
        cin: usize,
        cout: usize,
        kh: usize,
        kw: usize,
        stride: usize,
        act: u32,
    },
    MaxPool {
        kh: usize,
        kw: usize,
    },
    Slice {
        starts: Vec<i64>,
        ends: Vec<i64>,
        axes: Vec<i64>,
    },
    Concat,
    Act(u32),
    Identity,
}

struct Step {
    op: Op,
    inputs: Vec<String>,
    output: String,
}

pub(crate) struct Graph {
    steps: Vec<Step>,
    input: String,
    pub output: String,
}

fn act_of(op: &str) -> Option<u32> {
    match op {
        "Relu" => Some(1),
        "Sigmoid" => Some(2),
        _ => None,
    }
}

/// One value for a symmetric 2D attribute ([a, a] or [a, a, a, a]); `default` when absent.
fn symmetric(node: &Node, name: &str, default: usize) -> Result<usize, GpuError> {
    match node.ints(name) {
        None => Ok(default),
        Some(v) if v.is_empty() => Ok(default),
        Some(v) if v.iter().all(|&x| Some(&x) == v.first()) => {
            v.first().and_then(|&x| usize::try_from(x).ok()).ok_or_else(|| GpuError::Model(format!("{}: negative {name}", node.op)))
        }
        Some(v) => Err(GpuError::Model(format!("{}: asymmetric {name} {v:?}", node.op))),
    }
}

impl Graph {
    /// The graph of `onnx` up to (not including) the first node of type `stop`, if given.
    /// Returns the graph and the stop node.
    pub fn build(gpu: &Gpu, onnx: &Onnx, stop: Option<&str>) -> Result<(Self, Option<Node>), GpuError> {
        let mut steps = Vec::new();
        let mut stop_node = None;
        let nodes = &onnx.nodes;
        let mut i = 0;
        while let Some(n) = nodes.get(i) {
            if Some(n.op.as_str()) == stop {
                stop_node = Some(n.clone());
                break;
            }
            let mut output = n.output().to_string();
            // An activation right after, reading only this output, is fused into it.
            let mut fused = 0;
            if matches!(n.op.as_str(), "Conv" | "ConvTranspose")
                && let Some(next) = nodes.get(i + 1)
                && let Some(a) = act_of(&next.op)
                && next.input(0) == output
                && onnx.readers(&output) == 1
            {
                fused = a;
                output = next.output().to_string();
                i += 1;
            }
            let bias = |idx: usize, count: usize| -> Result<Buf, GpuError> {
                match n.inputs.get(idx).filter(|s| !s.is_empty()) {
                    Some(name) => {
                        let w = onnx.weight(name)?;
                        if w.data.len() != count {
                            return Err(GpuError::Model(format!("{} bias of {} for {count} channels", n.op, w.data.len())));
                        }
                        gpu.upload(&w.data)
                    }
                    None => gpu.upload(&vec![0f32; count]),
                }
            };
            let op = match n.op.as_str() {
                "Conv" => {
                    let w = onnx.weight(n.input(1))?;
                    let [cout, cin_g, kh, kw] = <[usize; 4]>::try_from(w.dims.clone()).map_err(|_| GpuError::Model("conv weight not 4D".into()))?;
                    let group = usize::try_from(n.int("group").unwrap_or(1)).map_err(|_| GpuError::Model("negative group".into()))?;
                    if symmetric(n, "dilations", 1)? != 1 || n.string("auto_pad").is_some_and(|a| a != "NOTSET") {
                        return Err(GpuError::Model("unsupported Conv attributes".into()));
                    }
                    let (pad, stride) = (symmetric(n, "pads", 0)?, symmetric(n, "strides", 1)?);
                    let b = bias(2, cout)?;
                    if group == 1 && cin_g == 1 && stride == 1 {
                        // A MaxPool right after, reading only this output, is fused in.
                        let mut pool = (1, 1);
                        if let Some(next) = nodes.get(i + 1)
                            && next.op == "MaxPool"
                            && next.input(0) == output
                            && onnx.readers(&output) == 1
                            && let Some(k) = next.ints("kernel_shape")
                            && let [ph, pw] = k[..]
                            && next.ints("strides").is_none_or(|s| s == k)
                            && symmetric(next, "pads", 0)? == 0
                            && next.int("ceil_mode").unwrap_or(0) == 0
                            && ph > 0
                            && pw > 0
                        {
                            pool = (ph as usize, pw as usize);
                            output = next.output().to_string();
                            i += 1;
                        }
                        Op::Direct { w: gpu.upload(&w.data)?, b, cout, kh, kw, pad, act: fused, pool }
                    } else if group == 1 {
                        let cin = cin_g;
                        let fast = cin % 16 == 0;
                        // [co][ci][ky][kx] -> [ky][kx][ci][co] (fast) or [ci][ky][kx][co].
                        let mut t = vec![0f32; w.data.len()];
                        for co in 0..cout {
                            for ci in 0..cin {
                                for ky in 0..kh {
                                    for kx in 0..kw {
                                        let k = if fast { (ky * kw + kx) * cin + ci } else { (ci * kh + ky) * kw + kx };
                                        if let (Some(dst), Some(&v)) = (t.get_mut(k * cout + co), w.data.get(((co * cin + ci) * kh + ky) * kw + kx)) {
                                            *dst = v;
                                        }
                                    }
                                }
                            }
                        }
                        Op::Conv { w: gpu.upload(&t)?, b, cin, cout, kh, kw, pad, stride, act: fused, fast }
                    } else if cin_g == 1 && group == cout {
                        Op::Depthwise { w: gpu.upload(&w.data)?, b, kh, kw, pad, stride, act: fused }
                    } else {
                        return Err(GpuError::Model(format!("grouped conv with group {group}")));
                    }
                }
                "ConvTranspose" => {
                    let w = onnx.weight(n.input(1))?;
                    let [cin, cout, kh, kw] =
                        <[usize; 4]>::try_from(w.dims.clone()).map_err(|_| GpuError::Model("ConvTranspose weight not 4D".into()))?;
                    if n.int("group").unwrap_or(1) != 1 || symmetric(n, "pads", 0)? != 0 || symmetric(n, "output_padding", 0)? != 0 {
                        return Err(GpuError::Model("unsupported ConvTranspose attributes".into()));
                    }
                    Op::ConvT {
                        w: gpu.upload(&w.data)?,
                        b: bias(2, cout)?,
                        cin,
                        cout,
                        kh,
                        kw,
                        stride: symmetric(n, "strides", 1)?.max(1),
                        act: fused,
                    }
                }
                "MaxPool" => {
                    let k = n.ints("kernel_shape").ok_or_else(|| GpuError::Model("MaxPool without kernel_shape".into()))?;
                    let s = n.ints("strides").unwrap_or_else(|| k.clone());
                    let [kh, kw] = k[..] else { return Err(GpuError::Model("MaxPool not 2D".into())) };
                    if k != s || kh <= 0 || kw <= 0 || symmetric(n, "pads", 0)? != 0 || n.int("ceil_mode").unwrap_or(0) != 0 {
                        return Err(GpuError::Model("unsupported MaxPool attributes".into()));
                    }
                    Op::MaxPool { kh: kh as usize, kw: kw as usize }
                }
                "Slice" => {
                    let get = |idx: usize| n.inputs.get(idx).and_then(|s| onnx.ints.get(s)).cloned();
                    if get(4).unwrap_or_default().iter().any(|&s| s != 1) {
                        return Err(GpuError::Model("Slice with steps".into()));
                    }
                    let starts = get(1).ok_or_else(|| GpuError::Model("Slice starts not constant".into()))?;
                    let ends = get(2).ok_or_else(|| GpuError::Model("Slice ends not constant".into()))?;
                    let axes = get(3).unwrap_or_else(|| (0..starts.len() as i64).collect());
                    Op::Slice { starts, ends, axes }
                }
                "Concat" => {
                    if n.int("axis") != Some(1) {
                        return Err(GpuError::Model("Concat not on channels".into()));
                    }
                    Op::Concat
                }
                "Pad" => {
                    let pads = n
                        .inputs
                        .get(1)
                        .and_then(|s| onnx.ints.get(s).cloned().or_else(|| onnx.weights.get(s).map(|w| w.data.iter().map(|&v| v as i64).collect())));
                    if pads.is_some_and(|p| p.iter().any(|&v| v != 0)) {
                        return Err(GpuError::Model("non-zero Pad".into()));
                    }
                    Op::Identity
                }
                op => match act_of(op) {
                    Some(a) => Op::Act(a),
                    None => return Err(GpuError::Model(format!("unsupported op {op}"))),
                },
            };
            let inputs = match op {
                Op::Concat => n.inputs.clone(),
                _ => vec![n.input(0).to_string()],
            };
            steps.push(Step { op, inputs, output });
            i += 1;
        }
        let output = match &stop_node {
            Some(s) => s.input(0).to_string(),
            None => onnx.output.clone(),
        };
        Ok((Self { steps, input: onnx.input.clone(), output }, stop_node))
    }

    /// Record the graph on `x`; returns the output. Every buffer it makes is added to `scratch`,
    /// to go back to the pool after the caller submits.
    pub fn run(&self, gpu: &Gpu, enc: &mut wgpu::CommandEncoder, x: Tensor, scratch: &mut Vec<Buf>) -> Result<Tensor, GpuError> {
        let mut values: HashMap<String, Tensor> = HashMap::new();
        values.insert(self.input.clone(), x);
        let new = |shape: [usize; 4], scratch: &mut Vec<Buf>| -> Result<Tensor, GpuError> {
            let buf = gpu.storage(count(&shape)?)?;
            scratch.push(buf.clone());
            Ok(Tensor { buf, shape })
        };
        for s in &self.steps {
            let first = s.inputs.first().map_or("", String::as_str);
            let a = values.get(first).cloned().ok_or_else(|| GpuError::Model(format!("missing value {first}")))?;
            let [n, c, h, w] = a.shape;
            let out = match &s.op {
                Op::Conv { w: wt, b, cin, cout, kh, kw, pad, stride, act, fast } => {
                    if c != *cin {
                        return Err(GpuError::Model(format!("conv expects {cin} channels, has {c}")));
                    }
                    let (oh, ow) = (out_len(h, 2 * pad, *kh, *stride)?, out_len(w, 2 * pad, *kw, *stride)?);
                    let y = new([n, *cout, oh, ow], scratch)?;
                    let p = gpu.uniform(&params(&[n, *cin, h, w, *cout, oh, ow, *kh, *kw, *pad, *stride, *act as usize, *pad, *stride, 0, 0])?)?;
                    let (pipe, tile) = if *fast && gpu.tuning.conv128 {
                        ("conv128", 128)
                    } else if *fast {
                        ("conv_fast", 64)
                    } else {
                        ("conv", 64)
                    };
                    let pixels = count(&[n, oh, ow])?;
                    gpu.dispatch(
                        enc,
                        pipe,
                        &[&a.buf, wt, b, &y.buf, &p, &a.buf, &a.buf],
                        (to_u32(pixels.div_ceil(tile))?, to_u32(cout.div_ceil(64))?, 1),
                    )?;
                    y
                }
                Op::Depthwise { w: wt, b, kh, kw, pad, stride, act } => {
                    let (oh, ow) = (out_len(h, 2 * pad, *kh, *stride)?, out_len(w, 2 * pad, *kw, *stride)?);
                    let y = new([n, c, oh, ow], scratch)?;
                    let p = gpu.uniform(&params(&[n, c, h, w, oh, ow, *kh, *kw, *pad, *stride, *act as usize, *pad, *stride])?)?;
                    gpu.elementwise(enc, "depthwise", &[&a.buf, wt, b, &y.buf, &p], y.len()?)?;
                    y
                }
                Op::Direct { w: wt, b, cout, kh, kw, pad, act, pool } => {
                    if c != 1 {
                        return Err(GpuError::Model(format!("single-channel conv on {c} channels")));
                    }
                    let (ch, cw) = (out_len(h, 2 * pad, *kh, 1)?, out_len(w, 2 * pad, *kw, 1)?);
                    let (oh, ow) = (ch / pool.0.max(1), cw / pool.1.max(1));
                    let y = new([n, *cout, oh, ow], scratch)?;
                    let p = gpu.uniform(&params(&[n, h, w, *cout, oh, ow, *kh, *kw, *pad, *act as usize, pool.0, pool.1])?)?;
                    gpu.elementwise(enc, "conv_direct", &[&a.buf, wt, b, &y.buf, &p], y.len()?)?;
                    y
                }
                Op::ConvT { w: wt, b, cin, cout, kh, kw, stride, act } => {
                    if c != *cin {
                        return Err(GpuError::Model(format!("ConvTranspose expects {cin} channels, has {c}")));
                    }
                    let grow = |s: usize, k: usize| {
                        s.checked_sub(1)
                            .and_then(|v| v.checked_mul(*stride))
                            .and_then(|v| v.checked_add(k))
                            .ok_or_else(|| GpuError::TooLarge(format!("ConvTranspose of {s}")))
                    };
                    let (oh, ow) = (grow(h, *kh)?, grow(w, *kw)?);
                    let y = new([n, *cout, oh, ow], scratch)?;
                    let p = gpu.uniform(&params(&[n, *cin, h, w, *cout, oh, ow, *kh, *kw, *stride, *act as usize, 0])?)?;
                    gpu.elementwise(enc, "convt", &[&a.buf, wt, b, &y.buf, &p], y.len()?)?;
                    y
                }
                Op::MaxPool { kh, kw } => {
                    let (oh, ow) = (h / kh, w / kw);
                    let y = new([n, c, oh, ow], scratch)?;
                    let p = gpu.uniform(&params(&[count(&[n, c])?, h, w, oh, ow, *kh, *kw, 0])?)?;
                    gpu.elementwise(enc, "maxpool", &[&a.buf, &y.buf, &p], y.len()?)?;
                    y
                }
                Op::Slice { starts, ends, axes } => {
                    let mut lo = [0usize; 4];
                    let mut out = a.shape;
                    for ((&st, &en), &ax) in starts.iter().zip(ends).zip(axes) {
                        let ax = usize::try_from(if ax < 0 { ax + 4 } else { ax })
                            .ok()
                            .filter(|&x| x < 4)
                            .ok_or_else(|| GpuError::Model(format!("Slice axis {ax}")))?;
                        let dim = a.shape[ax] as i64;
                        let clamp = |v: i64| (if v < 0 { v.saturating_add(dim) } else { v }).clamp(0, dim) as usize;
                        let (s0, e0) = (clamp(st), clamp(en));
                        lo[ax] = s0;
                        out[ax] = e0.saturating_sub(s0);
                    }
                    let y = new(out, scratch)?;
                    let p = gpu.uniform(&params(&[n, c, h, w, out[0], out[1], out[2], out[3], lo[0], lo[1], lo[2], lo[3]])?)?;
                    gpu.elementwise(enc, "slice", &[&a.buf, &y.buf, &p], y.len()?)?;
                    y
                }
                Op::Concat => {
                    let parts: Vec<Tensor> = s
                        .inputs
                        .iter()
                        .map(|i| values.get(i).cloned().ok_or_else(|| GpuError::Model(format!("missing value {i}"))))
                        .collect::<Result<_, _>>()?;
                    if parts.iter().any(|t| t.shape[0] != n || t.shape[2] != h || t.shape[3] != w) {
                        return Err(GpuError::Model("Concat shapes differ".into()));
                    }
                    let ctot = parts.iter().try_fold(0usize, |a, t| a.checked_add(t.shape[1])).ok_or_else(|| GpuError::TooLarge("Concat".into()))?;
                    let y = new([n, ctot, h, w], scratch)?;
                    let mut off = 0;
                    for t in &parts {
                        let p = gpu.uniform(&params(&[n, t.shape[1], count(&[h, w])?, ctot, off, 0, 0, 0])?)?;
                        gpu.elementwise(enc, "concat", &[&t.buf, &y.buf, &p], t.len()?)?;
                        off += t.shape[1];
                    }
                    y
                }
                Op::Act(act) => {
                    let y = new(a.shape, scratch)?;
                    let p = gpu.uniform(&[to_u32(y.len()?)?, *act, 0, 0])?;
                    gpu.elementwise(enc, "activate", &[&a.buf, &y.buf, &p], y.len()?)?;
                    y
                }
                Op::Identity => a.clone(),
            };
            values.insert(s.output.clone(), out);
        }
        values.remove(&self.output).ok_or_else(|| GpuError::Model(format!("graph output {} not computed", self.output)))
    }
}
