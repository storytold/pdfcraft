//! The ocrs text recognition model on the GPU: its convolutional trunk on each batch of line
//! images, then the two bidirectional GRU layers (ONNX semantics, `linear_before_reset = 1`)
//! over every line at once — one dispatch per layer, each line running for its own length — and
//! the output layer, with the arg-max of each step taken on the CPU after reading back.

use std::sync::Arc;

use super::graph::{Graph, Tensor, count};
use super::{Buf, Gpu, GpuError, params, to_u32};
use crate::onnx::Onnx;

/// The model's line height.
pub const LINE_HEIGHT: usize = 64;
/// The GRU's hidden size the kernels are written for.
const HIDDEN: usize = 256;

struct GruLayer {
    w_t: Buf,
    wb: Buf,
    r: Buf,
    rb: Buf,
    input: usize,
}

pub struct Recognizer {
    trunk: Graph,
    pool_kh: usize,
    layers: Vec<GruLayer>,
    lin_w: Buf,
    lin_b: Buf,
    classes: usize,
}

/// What recognising a set of batches gives: per line, its sequence of best classes.
pub struct Recognized {
    /// For each line in input order (batch by batch), the class with the highest score at each
    /// of its steps.
    pub best: Vec<Vec<u32>>,
}

impl Recognizer {
    pub fn new(gpu: &Gpu, onnx: &Onnx) -> Result<Self, GpuError> {
        let (trunk, pool) = Graph::build(gpu, onnx, Some("AveragePool"))?;
        let pool = pool.ok_or_else(|| GpuError::Model("no AveragePool in the recognition model".into()))?;
        let k = pool.ints("kernel_shape").ok_or_else(|| GpuError::Model("AveragePool without kernel_shape".into()))?;
        let pool_kh = match k[..] {
            [kh, 1] if kh > 0 && pool.ints("strides").as_deref() == Some(&k[..]) => kh as usize,
            _ => return Err(GpuError::Model(format!("unsupported AveragePool {k:?}"))),
        };
        let mut layers = Vec::new();
        for n in onnx.nodes.iter().filter(|n| n.op == "GRU") {
            if n.int("hidden_size") != Some(HIDDEN as i64)
                || n.int("linear_before_reset") != Some(1)
                || n.string("direction").as_deref() != Some("bidirectional")
            {
                return Err(GpuError::Model("unsupported GRU attributes".into()));
            }
            let (w, r, b) = (onnx.weight(n.input(1))?, onnx.weight(n.input(2))?, onnx.weight(n.input(3))?);
            let input = match w.dims[..] {
                [2, rows, input] if rows == 3 * HIDDEN => input,
                _ => return Err(GpuError::Model(format!("GRU W {:?}", w.dims))),
            };
            if r.dims != [2, 3 * HIDDEN, HIDDEN] || b.dims != [2, 6 * HIDDEN] {
                return Err(GpuError::Model(format!("GRU R {:?} B {:?}", r.dims, b.dims)));
            }
            // W [2][768][in], rows of the projection -> [in][1536] for the GEMM.
            let rows = 2 * 3 * HIDDEN;
            let mut w_t = vec![0f32; w.data.len()];
            for row in 0..rows {
                for i in 0..input {
                    if let (Some(dst), Some(&v)) = (w_t.get_mut(i * rows + row), w.data.get(row * input + i)) {
                        *dst = v;
                    }
                }
            }
            let (mut wb, mut rb) = (Vec::new(), Vec::new());
            for dir in 0..2 {
                let at = dir * 6 * HIDDEN;
                wb.extend_from_slice(b.data.get(at..at + 3 * HIDDEN).unwrap_or_default());
                rb.extend_from_slice(b.data.get(at + 3 * HIDDEN..at + 6 * HIDDEN).unwrap_or_default());
            }
            let r_buf = if gpu.tuning.f16 {
                let bits: Vec<u16> = r.data.iter().map(|&v| half::f16::from_f32(v).to_bits()).collect();
                let mut bytes = bytemuck::cast_slice::<u16, u8>(&bits).to_vec();
                bytes.resize(bytes.len().next_multiple_of(4), 0);
                gpu.upload_bytes(&bytes)?
            } else {
                gpu.upload(&r.data)?
            };
            layers.push(GruLayer { w_t: gpu.upload(&w_t)?, wb: gpu.upload(&wb)?, r: r_buf, rb: gpu.upload(&rb)?, input });
        }
        if layers.len() != 2 || layers.get(1).map(|l| l.input) != Some(2 * HIDDEN) {
            return Err(GpuError::Model(format!("{} GRU layers", layers.len())));
        }
        let matmul = onnx.nodes.iter().find(|n| n.op == "MatMul").ok_or_else(|| GpuError::Model("no MatMul".into()))?;
        let lin = onnx.weight(matmul.input(1))?;
        let classes = match lin.dims[..] {
            [i, c] if i == 2 * HIDDEN && c > 0 => c,
            _ => return Err(GpuError::Model(format!("output layer {:?}", lin.dims))),
        };
        let add = onnx
            .nodes
            .iter()
            .find(|n| n.op == "Add" && n.inputs.iter().any(|i| i == matmul.output()))
            .ok_or_else(|| GpuError::Model("no bias Add".into()))?;
        let bias = add
            .inputs
            .iter()
            .find_map(|i| onnx.weights.get(i))
            .filter(|w| w.data.len() == classes)
            .ok_or_else(|| GpuError::Model("no bias weight".into()))?;
        Ok(Self { trunk, pool_kh, layers, lin_w: gpu.upload(&lin.data)?, lin_b: gpu.upload(&bias.data)?, classes })
    }

    /// The classes the model reads (blank first).
    pub fn classes(&self) -> usize {
        self.classes
    }

    /// Recognise `batches` of line images, each `(data, lines, width)` with `data` the
    /// [lines, 1, 64, width] input in ocrs' normalisation. Returns, per line in order, the best
    /// class at each of its steps.
    pub fn recognize(&self, gpu: &Gpu, batches: &[(Vec<f32>, usize, usize)]) -> Result<Recognized, GpuError> {
        for (data, n, w) in batches {
            if *n == 0 || *w == 0 || Some(data.len()) != count(&[*n, LINE_HEIGHT, *w]).ok() {
                return Err(GpuError::Model(format!("a batch of {} values for {n} lines of width {w}", data.len())));
            }
        }
        if batches.is_empty() {
            return Ok(Recognized { best: Vec::new() });
        }
        gpu.scoped(|| self.run(gpu, batches))
    }

    fn run(&self, gpu: &Gpu, batches: &[(Vec<f32>, usize, usize)]) -> Result<Recognized, GpuError> {
        let mut scratch: Vec<Buf> = Vec::new();
        // The trunks first, each submitted on its own so its activations can be reused.
        let mut trunks = Vec::new();
        for (data, n, w) in batches {
            let mut enc = gpu.encoder();
            let input = gpu.upload(data)?;
            let mut s = Vec::new();
            let out = self.trunk.run(gpu, &mut enc, Tensor { buf: input, shape: [*n, 1, LINE_HEIGHT, *w] }, &mut s)?;
            gpu.submit_wait(enc)?;
            s.retain(|b| !Arc::ptr_eq(b, &out.buf));
            gpu.recycle(s);
            scratch.push(out.buf.clone());
            trunks.push(out);
        }
        let ntot = batches.iter().try_fold(0usize, |a, b| a.checked_add(b.1)).ok_or_else(|| GpuError::TooLarge("lines".into()))?;
        let tmax = trunks.iter().map(|t| t.shape[3]).max().unwrap_or(0);
        let feat = trunks.first().map_or(0, |t| t.shape[1]);
        let first = self.layers.first().ok_or_else(|| GpuError::Model("no GRU".into()))?;
        if feat != first.input {
            return Err(GpuError::Model(format!("trunk gives {feat} features, the GRU takes {}", first.input)));
        }
        let mut enc = gpu.encoder();
        let x = gpu.storage(count(&[tmax, ntot, feat])?)?;
        scratch.push(x.clone());
        let mut lens: Vec<u32> = Vec::with_capacity(ntot);
        let mut noff = 0;
        for t in &trunks {
            let [n, c, h, w] = t.shape;
            if h < self.pool_kh || (h - self.pool_kh) / self.pool_kh + 1 != 1 {
                return Err(GpuError::Model(format!("trunk height {h} does not pool to one row")));
            }
            let p = gpu.uniform(&params(&[n, c, h, w, self.pool_kh, ntot, noff, 0])?)?;
            gpu.elementwise(&mut enc, "avgpool_t", &[&t.buf, &x, &p], count(&[n, c, w])?)?;
            lens.extend(std::iter::repeat_n(to_u32(w)?, n));
            noff += n;
        }
        let lens_buf = gpu.upload_u32(&lens)?;
        let m = count(&[tmax, ntot])?;
        let gates = gpu.storage(count(&[m, 6 * HIDDEN])?)?;
        scratch.push(gates.clone());
        let mut input = x;
        for layer in &self.layers {
            let out = gpu.storage(count(&[m, 2 * HIDDEN])?)?;
            scratch.push(out.clone());
            let d = gpu.uniform(&params(&[m, 6 * HIDDEN, layer.input, 0])?)?;
            gpu.dispatch(
                &mut enc,
                "gemm",
                &[&input, &layer.w_t, &layer.wb, &gates, &d],
                (to_u32(m.div_ceil(64))?, to_u32((6 * HIDDEN).div_ceil(64))?, 1),
            )?;
            let d2 = gpu.uniform(&params(&[tmax, ntot, 0, 0])?)?;
            gpu.dispatch(
                &mut enc,
                "recurrence",
                &[&gates, &layer.r, &layer.rb, &out, &d2, &lens_buf],
                (to_u32(ntot.div_ceil(gpu.tuning.lines))?, 2, 1),
            )?;
            input = out;
        }
        let logits = gpu.storage(count(&[m, self.classes])?)?;
        scratch.push(logits.clone());
        let d = gpu.uniform(&params(&[m, self.classes, 2 * HIDDEN, 0])?)?;
        gpu.dispatch(
            &mut enc,
            "gemm",
            &[&input, &self.lin_w, &self.lin_b, &logits, &d],
            (to_u32(m.div_ceil(64))?, to_u32(self.classes.div_ceil(64))?, 1),
        )?;
        let raw = gpu.read(enc, &logits, count(&[m, self.classes])?)?;
        gpu.recycle(scratch);
        // The arg-max of each step of each line (log-softmax keeps it), over its own length.
        let mut best = Vec::with_capacity(ntot);
        for (line, &len) in lens.iter().enumerate() {
            let steps: Vec<u32> = (0..len as usize)
                .map(|t| {
                    let at = (t * ntot + line) * self.classes;
                    raw.get(at..at + self.classes)
                        .map(|row| {
                            row.iter().enumerate().max_by(|a, b| f32::from_bits(*a.1).total_cmp(&f32::from_bits(*b.1))).map_or(0, |(i, _)| i as u32)
                        })
                        .unwrap_or(0)
                })
                .collect();
            best.push(steps);
        }
        Ok(Recognized { best })
    }
}
