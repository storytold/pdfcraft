//! The WGSL compute shaders: convolutions (implicit GEMM, depthwise, single-channel direct,
//! transposed), pooling, copies, the GRU recurrence, and page pre- and post-processing for the
//! ocrs detector. Ported from the wgpu-ocr prototype (github.com/huppiflupp/wgpu-ocr), which
//! measured each variant against rten.

pub(crate) const EPILOGUE: &str = r#"
fn erf_approx(v: f32) -> f32 {
    // Abramowitz & Stegun 7.1.26, absolute error below 1.5e-7.
    let s = sign(v);
    let a = abs(v);
    let t = 1.0 / (1.0 + 0.3275911 * a);
    let poly = ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t;
    return s * (1.0 - poly * exp(-a * a));
}
fn activate(v: f32, act: u32) -> f32 {
    if (act == 1u) { return max(v, 0.0); }
    if (act == 2u) { return 1.0 / (1.0 + exp(-v)); }
    if (act == 3u) { return 0.5 * v * (1.0 + erf_approx(v * 0.70710678118654752)); }
    if (act == 4u) { return v / (1.0 + exp(-v)); }
    return v;
}
"#;

/// Dense convolution as an implicit GEMM, 64 pixels x 64 channels per workgroup, 4x4 per thread.
/// FAST: cin is a multiple of 16 and the weights are ordered [ky][kx][ci][co], so a 16-deep
/// slice of K has one tap (ky, kx) and 16 consecutive input channels: no division per element.
/// Otherwise the weights are [ci][ky][kx][co] and each element decodes its k.
pub(crate) const CONV: &str = r#"
struct P { n: u32, cin: u32, h: u32, w: u32, cout: u32, oh: u32, ow: u32, kh: u32, kw: u32, pad: u32, stride: u32, act: u32, padx: u32, stridex: u32, x0: u32, x1: u32 }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read> wt: array<f32>;
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> y: array<f32>;
@group(0) @binding(4) var<uniform> p: P;
@group(0) @binding(5) var<storage, read> res: array<f32>;  // added to the output when p.x0 == 1
@group(0) @binding(6) var<storage, read> insc: array<f32>;  // [n][cin] input scale when p.x1 == 1
var<workgroup> As: array<f32, 1024>;
var<workgroup> Bs: array<f32, 1024>;
EPILOGUE_FN

fn put(m: u32, c0: u32, v: vec4<f32>) {
    if (m >= p.n * p.oh * p.ow) { return; }
    let ox = m % p.ow;
    let t = m / p.ow;
    let oy = t % p.oh;
    let nb = t / p.oh;
    for (var j = 0u; j < 4u; j++) {
        let c = c0 + j;
        if (c < p.cout) {
            let o = ((nb * p.cout + c) * p.oh + oy) * p.ow + ox;
            var r = activate(v[j] + b[c], p.act);
            if (p.x0 == 1u) { r += res[o]; }
            y[o] = r;
        }
    }
}

@compute @workgroup_size(16, 16)
fn main(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
    let tid = lid.y * 16u + lid.x;
    let m0 = wid.x * 64u;
    let c0 = wid.y * 64u;
    let mtot = p.n * p.oh * p.ow;
    let ktot = p.cin * p.kh * p.kw;
    // The A elements this thread loads are always the same pixel (idx % 64 == tid % 64).
    let mm = tid % 64u;
    let kq = tid / 64u;
    let m = m0 + mm;
    let valid = m < mtot;
    let ox = m % p.ow;
    let tt = m / p.ow;
    let oy = tt % p.oh;
    let nb = tt / p.oh;
    let iy0 = i32(oy * p.stride) - i32(p.pad);
    let ix0 = i32(ox * p.stridex) - i32(p.padx);
    let plane = p.h * p.w;
    let xbase = nb * p.cin * plane;
    var acc0 = vec4<f32>(0.0);
    var acc1 = vec4<f32>(0.0);
    var acc2 = vec4<f32>(0.0);
    var acc3 = vec4<f32>(0.0);
    for (var k0 = 0u; k0 < ktot; k0 += 16u) {
FAST_OR_GENERIC
        workgroupBarrier();
        for (var kk = 0u; kk < 16u; kk++) {
            let ab = kk * 64u + lid.x * 4u;
            let a = vec4<f32>(As[ab], As[ab + 1u], As[ab + 2u], As[ab + 3u]);
            let bb = kk * 64u + lid.y * 4u;
            let bv = vec4<f32>(Bs[bb], Bs[bb + 1u], Bs[bb + 2u], Bs[bb + 3u]);
            acc0 += a.x * bv;
            acc1 += a.y * bv;
            acc2 += a.z * bv;
            acc3 += a.w * bv;
        }
        workgroupBarrier();
    }
    let mb = m0 + lid.x * 4u;
    let cb = c0 + lid.y * 4u;
    put(mb, cb, acc0);
    put(mb + 1u, cb, acc1);
    put(mb + 2u, cb, acc2);
    put(mb + 3u, cb, acc3);
}
"#;

pub(crate) const CONV_FAST_LOAD: &str = r#"
        let tap = k0 / p.cin;
        let ci0 = k0 % p.cin;
        let iy = iy0 + i32(tap / p.kw);
        let ix = ix0 + i32(tap % p.kw);
        let inside = valid && iy >= 0 && iy < i32(p.h) && ix >= 0 && ix < i32(p.w);
        let pix = u32(iy) * p.w + u32(ix);
        for (var e = 0u; e < 4u; e++) {
            let kk = kq + e * 4u;
            var v = 0.0;
            if (inside) {
                v = x[xbase + (ci0 + kk) * plane + pix];
                if (p.x1 == 1u) { v *= insc[nb * p.cin + ci0 + kk]; }
            }
            As[kk * 64u + mm] = v;
            let c = c0 + mm;
            var wv = 0.0;
            if (c < p.cout) { wv = wt[(k0 + kk) * p.cout + c]; }
            Bs[kk * 64u + mm] = wv;
        }
"#;

pub(crate) const CONV_GENERIC_LOAD: &str = r#"
        let khw = p.kh * p.kw;
        for (var e = 0u; e < 4u; e++) {
            let kk = kq + e * 4u;
            let k = k0 + kk;
            var v = 0.0;
            if (valid && k < ktot) {
                let ci = k / khw;
                let r = k % khw;
                let iy = iy0 + i32(r / p.kw);
                let ix = ix0 + i32(r % p.kw);
                if (iy >= 0 && iy < i32(p.h) && ix >= 0 && ix < i32(p.w)) {
                    v = x[xbase + ci * plane + u32(iy) * p.w + u32(ix)];
                    if (p.x1 == 1u) { v *= insc[nb * p.cin + ci]; }
                }
            }
            As[kk * 64u + mm] = v;
            let c = c0 + mm;
            var wv = 0.0;
            if (k < ktot && c < p.cout) { wv = wt[k * p.cout + c]; }
            Bs[kk * 64u + mm] = wv;
        }
"#;

/// One thread per output element; `i` from a 2D grid of 256-wide workgroups.
pub(crate) const INDEX: &str = "let i = (wid.y * nw.x + wid.x) * 256u + lid.x;";

pub(crate) const ENTRY: &str = "@compute @workgroup_size(256)\nfn main(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {";

pub(crate) const DEPTHWISE: &str = r#"
struct P { n: u32, c: u32, h: u32, w: u32, oh: u32, ow: u32, kh: u32, kw: u32, pad: u32, stride: u32, act: u32, padx: u32, stridex: u32, x0: u32, x1: u32, x2: u32 }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read> wt: array<f32>;   // [c][kh][kw]
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> y: array<f32>;
@group(0) @binding(4) var<uniform> p: P;
EPILOGUE_FN
ENTRY
    INDEX
    if (i >= p.n * p.c * p.oh * p.ow) { return; }
    let ox = i % p.ow;
    let oy = (i / p.ow) % p.oh;
    let nc = i / (p.ow * p.oh);
    let c = nc % p.c;
    var acc = b[c];
    for (var ky = 0u; ky < p.kh; ky++) {
        let iy = i32(oy * p.stride + ky) - i32(p.pad);
        if (iy < 0 || iy >= i32(p.h)) { continue; }
        for (var kx = 0u; kx < p.kw; kx++) {
            let ix = i32(ox * p.stridex + kx) - i32(p.padx);
            if (ix < 0 || ix >= i32(p.w)) { continue; }
            acc += x[(nc * p.h + u32(iy)) * p.w + u32(ix)] * wt[(c * p.kh + ky) * p.kw + kx];
        }
    }
    y[i] = activate(acc, p.act);
}
"#;

/// A convolution with one input channel, direct (no GEMM tile to waste), with its activation
/// and, if one follows, its MaxPool fused: the full-size output is never written.
pub(crate) const CONV_DIRECT: &str = r#"
struct P { n: u32, h: u32, w: u32, cout: u32, oh: u32, ow: u32, kh: u32, kw: u32, pad: u32, act: u32, ph: u32, pw: u32 }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read> wt: array<f32>;   // [cout][kh][kw]
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> y: array<f32>;
@group(0) @binding(4) var<uniform> p: P;
EPILOGUE_FN
ENTRY
    INDEX
    if (i >= p.n * p.cout * p.oh * p.ow) { return; }
    let ox = i % p.ow;
    let oy = (i / p.ow) % p.oh;
    let co = (i / (p.ow * p.oh)) % p.cout;
    let nb = i / (p.ow * p.oh * p.cout);
    var best = -3.4e38;
    for (var py = 0u; py < p.ph; py++) {
        for (var px = 0u; px < p.pw; px++) {
            let cy = oy * p.ph + py;
            let cx = ox * p.pw + px;
            var acc = b[co];
            for (var ky = 0u; ky < p.kh; ky++) {
                let iy = i32(cy + ky) - i32(p.pad);
                if (iy < 0 || iy >= i32(p.h)) { continue; }
                for (var kx = 0u; kx < p.kw; kx++) {
                    let ix = i32(cx + kx) - i32(p.pad);
                    if (ix < 0 || ix >= i32(p.w)) { continue; }
                    acc += x[(nb * p.h + u32(iy)) * p.w + u32(ix)] * wt[(co * p.kh + ky) * p.kw + kx];
                }
            }
            best = max(best, activate(acc, p.act));
        }
    }
    y[i] = best;
}
"#;

pub(crate) const CONV_TRANSPOSE: &str = r#"
struct P { n: u32, cin: u32, h: u32, w: u32, cout: u32, oh: u32, ow: u32, kh: u32, kw: u32, stride: u32, act: u32, x0: u32 }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read> wt: array<f32>;   // [cin][cout][kh][kw]
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> y: array<f32>;
@group(0) @binding(4) var<uniform> p: P;
EPILOGUE_FN
ENTRY
    INDEX
    if (i >= p.n * p.cout * p.oh * p.ow) { return; }
    let ox = i % p.ow;
    let oy = (i / p.ow) % p.oh;
    let co = (i / (p.ow * p.oh)) % p.cout;
    let nb = i / (p.ow * p.oh * p.cout);
    var acc = b[co];
    // Only the taps of this pixel's parity: ky = oy mod stride, + stride, ...
    let plane = p.h * p.w;
    let wstep = p.cout * p.kh * p.kw;
    for (var ky = oy % p.stride; ky < p.kh && ky <= oy; ky += p.stride) {
        let iy = (oy - ky) / p.stride;
        if (iy >= p.h) { continue; }
        for (var kx = ox % p.stride; kx < p.kw && kx <= ox; kx += p.stride) {
            let ix = (ox - kx) / p.stride;
            if (ix >= p.w) { continue; }
            var xi = nb * p.cin * plane + iy * p.w + ix;
            var wi = (co * p.kh + ky) * p.kw + kx;
            for (var ci = 0u; ci < p.cin; ci++) {
                acc += x[xi] * wt[wi];
                xi += plane;
                wi += wstep;
            }
        }
    }
    y[i] = activate(acc, p.act);
}
"#;

pub(crate) const MAXPOOL: &str = r#"
struct P { nc: u32, h: u32, w: u32, oh: u32, ow: u32, kh: u32, kw: u32, x0: u32 }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read_write> y: array<f32>;
@group(0) @binding(2) var<uniform> p: P;
ENTRY
    INDEX
    if (i >= p.nc * p.oh * p.ow) { return; }
    let ox = i % p.ow;
    let t = i / p.ow;
    let oy = t % p.oh;
    let c = t / p.oh;
    var m = -3.4e38;
    for (var ky = 0u; ky < p.kh; ky++) {
        for (var kx = 0u; kx < p.kw; kx++) {
            m = max(m, x[(c * p.h + oy * p.kh + ky) * p.w + ox * p.kw + kx]);
        }
    }
    y[i] = m;
}
"#;

/// out[n][c][y][x] = in[n + s.x][c + s.y][y + s.z][x + s.w]
pub(crate) const SLICE: &str = r#"
struct P { i: vec4<u32>, o: vec4<u32>, s: vec4<u32> }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read_write> y: array<f32>;
@group(0) @binding(2) var<uniform> p: P;
ENTRY
    INDEX
    if (i >= p.o.x * p.o.y * p.o.z * p.o.w) { return; }
    let xx = i % p.o.w;
    let yy = (i / p.o.w) % p.o.z;
    let cc = (i / (p.o.w * p.o.z)) % p.o.y;
    let nn = i / (p.o.w * p.o.z * p.o.y);
    y[i] = x[(((nn + p.s.x) * p.i.y + cc + p.s.y) * p.i.z + yy + p.s.z) * p.i.w + xx + p.s.w];
}
"#;

/// Copy x [n][c][hw] into channels [off, off + c) of y [n][ctot][hw].
pub(crate) const CONCAT: &str = r#"
struct P { n: u32, c: u32, hw: u32, ctot: u32, off: u32, x0: u32, x1: u32, x2: u32 }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read_write> y: array<f32>;
@group(0) @binding(2) var<uniform> p: P;
ENTRY
    INDEX
    if (i >= p.n * p.c * p.hw) { return; }
    let s = i % p.hw;
    let c = (i / p.hw) % p.c;
    let nb = i / (p.hw * p.c);
    y[(nb * p.ctot + p.off + c) * p.hw + s] = x[i];
}
"#;

pub(crate) const ACTIVATE: &str = r#"
struct P { count: u32, act: u32, x0: u32, x1: u32 }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read_write> y: array<f32>;
@group(0) @binding(2) var<uniform> p: P;
EPILOGUE_FN
ENTRY
    INDEX
    if (i >= p.count) { return; }
    y[i] = activate(x[i], p.act);
}
"#;

/// AvgPool (kh, 1) to one output row, written into the GRU input [t][line][c] at this batch's
/// line offset.
pub(crate) const AVGPOOL_T: &str = r#"
struct P { n: u32, c: u32, h: u32, w: u32, kh: u32, ntot: u32, noff: u32, x0: u32 }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read_write> y: array<f32>;
@group(0) @binding(2) var<uniform> p: P;
ENTRY
    INDEX
    if (i >= p.n * p.c * p.w) { return; }
    let c = i % p.c;
    let t = (i / p.c) % p.w;
    let nb = i / (p.c * p.w);
    var s = 0.0;
    for (var r = 0u; r < p.kh; r++) {
        s += x[((nb * p.c + c) * p.h + r) * p.w + t];
    }
    y[(t * p.ntot + p.noff + nb) * p.c + c] = s / f32(p.kh);
}
"#;

/// RGB bytes of the page (packed 4 to a word) to ocrs' grey values, padded with black to at
/// least the model size, and resized to it bilinearly as rten's `resize_image` does
/// (half-pixel coordinates, clamped, x then y).
pub(crate) const GREY_RESIZE: &str = r#"
struct P { w: u32, h: u32, pw: u32, ph: u32, ow: u32, oh: u32, sx: f32, sy: f32 }
@group(0) @binding(0) var<storage, read> rgb: array<u32>;
@group(0) @binding(1) var<storage, read_write> y: array<f32>;
@group(0) @binding(2) var<uniform> p: P;
fn byte(i: u32) -> f32 { return f32((rgb[i / 4u] >> ((i % 4u) * 8u)) & 255u); }
fn grey(x: u32, yy: u32) -> f32 {
    if (x >= p.w || yy >= p.h) { return -0.5; }
    let i = (yy * p.w + x) * 3u;
    return -0.5 + byte(i) * (0.299 / 255.0) + byte(i + 1u) * (0.587 / 255.0) + byte(i + 2u) * (0.114 / 255.0);
}
ENTRY
    INDEX
    if (i >= p.ow * p.oh) { return; }
    let ox = i % p.ow;
    let oy = i / p.ow;
    let iy = clamp(p.sy * (f32(oy) + 0.5) - 0.5, 0.0, f32(p.ph) - 1.0);
    let ix = clamp(p.sx * (f32(ox) + 0.5) - 0.5, 0.0, f32(p.pw) - 1.0);
    let y1 = u32(iy);
    let x1 = u32(ix);
    let y2 = min(y1 + 1u, p.ph - 1u);
    let x2 = min(x1 + 1u, p.pw - 1u);
    let wy = iy - f32(y1);
    let wx = ix - f32(x1);
    let top = mix(grey(x1, y1), grey(x2, y1), wx);
    let bottom = mix(grey(x1, y2), grey(x2, y2), wx);
    y[i] = mix(top, bottom, wy);
}
"#;

/// The model's mask (its first `mh` x `mw` part, without the padding) resized to the page as
/// rten does, thresholded, and packed 32 pixels to a word per row.
pub(crate) const MASK_UP: &str = r#"
struct P { stride: u32, mw: u32, mh: u32, w: u32, h: u32, words: u32, sx: f32, sy: f32, thr: f32, x0: u32, x1: u32, x2: u32 }
@group(0) @binding(0) var<storage, read> m: array<f32>;
@group(0) @binding(1) var<storage, read_write> y: array<u32>;
@group(0) @binding(2) var<uniform> p: P;
ENTRY
    INDEX
    if (i >= p.words * p.h) { return; }
    let wx = i % p.words;
    let oy = i / p.words;
    let iy = clamp(p.sy * (f32(oy) + 0.5) - 0.5, 0.0, f32(p.mh) - 1.0);
    let y1 = u32(iy);
    let y2 = min(y1 + 1u, p.mh - 1u);
    let fy = iy - f32(y1);
    var bits = 0u;
    for (var b = 0u; b < 32u; b++) {
        let ox = wx * 32u + b;
        if (ox >= p.w) { break; }
        let ix = clamp(p.sx * (f32(ox) + 0.5) - 0.5, 0.0, f32(p.mw) - 1.0);
        let x1 = u32(ix);
        let x2 = min(x1 + 1u, p.mw - 1u);
        let fx = ix - f32(x1);
        let top = mix(m[y1 * p.stride + x1], m[y1 * p.stride + x2], fx);
        let bottom = mix(m[y2 * p.stride + x1], m[y2 * p.stride + x2], fx);
        if (mix(top, bottom, fy) > p.thr) { bits |= 1u << b; }
    }
    y[i] = bits;
}
"#;

/// y[m, k] = x[m, :] . wt[:, k] + b[k] with wt stored [i][k].
pub(crate) const GEMM: &str = r#"
struct Dims { m: u32, k: u32, i: u32, pad: u32 }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read> wt: array<f32>;
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> y: array<f32>;
@group(0) @binding(4) var<uniform> d: Dims;
var<workgroup> As: array<f32, 1024>;
var<workgroup> Bs: array<f32, 1024>;

fn put(m: u32, c0: u32, v: vec4<f32>) {
    if (m >= d.m) { return; }
    for (var j = 0u; j < 4u; j++) {
        let c = c0 + j;
        if (c < d.k) { y[m * d.k + c] = v[j] + b[c]; }
    }
}

@compute @workgroup_size(16, 16)
fn main(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
    let tid = lid.y * 16u + lid.x;
    let m0 = wid.x * 64u;
    let c0 = wid.y * 64u;
    var acc0 = vec4<f32>(0.0);
    var acc1 = vec4<f32>(0.0);
    var acc2 = vec4<f32>(0.0);
    var acc3 = vec4<f32>(0.0);
    for (var k0 = 0u; k0 < d.i; k0 += 16u) {
        for (var e = 0u; e < 4u; e++) {
            let idx = tid + e * 256u;
            let am = idx / 16u;
            let ak = idx % 16u;
            var v = 0.0;
            if (m0 + am < d.m && k0 + ak < d.i) { v = x[(m0 + am) * d.i + k0 + ak]; }
            As[ak * 64u + am] = v;
            let bk = idx / 64u;
            let bc = idx % 64u;
            var wv = 0.0;
            if (k0 + bk < d.i && c0 + bc < d.k) { wv = wt[(k0 + bk) * d.k + c0 + bc]; }
            Bs[bk * 64u + bc] = wv;
        }
        workgroupBarrier();
        for (var kk = 0u; kk < 16u; kk++) {
            let ab = kk * 64u + lid.x * 4u;
            let a = vec4<f32>(As[ab], As[ab + 1u], As[ab + 2u], As[ab + 3u]);
            let bb = kk * 64u + lid.y * 4u;
            let bv = vec4<f32>(Bs[bb], Bs[bb + 1u], Bs[bb + 2u], Bs[bb + 3u]);
            acc0 += a.x * bv;
            acc1 += a.y * bv;
            acc2 += a.z * bv;
            acc3 += a.w * bv;
        }
        workgroupBarrier();
    }
    let mb = m0 + lid.x * 4u;
    let cb = c0 + lid.y * 4u;
    put(mb, cb, acc0);
    put(mb + 1u, cb, acc1);
    put(mb + 2u, cb, acc2);
    put(mb + 3u, cb, acc3);
}
"#;

/// The fast path with a 128-pixel x 64-channel tile, 8 x 4 outputs per thread, and the tiles
/// read from workgroup memory as vec4: 3 loads per 32 FMAs instead of 8 per 16.
pub(crate) const CONV128: &str = r#"
struct P { n: u32, cin: u32, h: u32, w: u32, cout: u32, oh: u32, ow: u32, kh: u32, kw: u32, pad: u32, stride: u32, act: u32, padx: u32, stridex: u32, x0: u32, x1: u32 }
@group(0) @binding(0) var<storage, read> x: array<f32>;
@group(0) @binding(1) var<storage, read> wt: array<f32>;
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> y: array<f32>;
@group(0) @binding(4) var<uniform> p: P;
@group(0) @binding(5) var<storage, read> res: array<f32>;  // added to the output when p.x0 == 1
@group(0) @binding(6) var<storage, read> insc: array<f32>;  // [n][cin] input scale when p.x1 == 1
var<workgroup> As: array<vec4<f32>, 512>;   // [16 k][128 m / 4]
var<workgroup> Bs: array<vec4<f32>, 256>;   // [16 k][64 c / 4]
EPILOGUE_FN

fn put(m: u32, c0: u32, v: vec4<f32>) {
    if (m >= p.n * p.oh * p.ow) { return; }
    let ox = m % p.ow;
    let t = m / p.ow;
    let oy = t % p.oh;
    let nb = t / p.oh;
    for (var j = 0u; j < 4u; j++) {
        let c = c0 + j;
        if (c < p.cout) {
            let o = ((nb * p.cout + c) * p.oh + oy) * p.ow + ox;
            var r = activate(v[j] + b[c], p.act);
            if (p.x0 == 1u) { r += res[o]; }
            y[o] = r;
        }
    }
}

@compute @workgroup_size(16, 16)
fn main(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
    let tid = lid.y * 16u + lid.x;
    let m0 = wid.x * 128u;
    let c0 = wid.y * 64u;
    let mtot = p.n * p.oh * p.ow;
    let ktot = p.cin * p.kh * p.kw;
    // A: this thread always loads pixel tid % 128, at k rows tid / 128 + 2e.
    let mm = tid % 128u;
    let ka = tid / 128u;
    let m = m0 + mm;
    let valid = m < mtot;
    let ox = m % p.ow;
    let tt = m / p.ow;
    let oy = tt % p.oh;
    let nb = tt / p.oh;
    let iy0 = i32(oy * p.stride) - i32(p.pad);
    let ix0 = i32(ox * p.stridex) - i32(p.padx);
    let plane = p.h * p.w;
    let xbase = nb * p.cin * plane;
    // B: channel tid % 64, k rows tid / 64 + 4e.
    let cc = tid % 64u;
    let kb = tid / 64u;
    var a0 = vec4<f32>(0.0); var a1 = vec4<f32>(0.0); var a2 = vec4<f32>(0.0); var a3 = vec4<f32>(0.0);
    var a4 = vec4<f32>(0.0); var a5 = vec4<f32>(0.0); var a6 = vec4<f32>(0.0); var a7 = vec4<f32>(0.0);
    for (var k0 = 0u; k0 < ktot; k0 += 16u) {
        let tap = k0 / p.cin;
        let ci0 = k0 % p.cin;
        let iy = iy0 + i32(tap / p.kw);
        let ix = ix0 + i32(tap % p.kw);
        let inside = valid && iy >= 0 && iy < i32(p.h) && ix >= 0 && ix < i32(p.w);
        let pix = u32(iy) * p.w + u32(ix);
        for (var e = 0u; e < 8u; e++) {
            let kk = ka + e * 2u;
            var v = 0.0;
            if (inside) {
                v = x[xbase + (ci0 + kk) * plane + pix];
                if (p.x1 == 1u) { v *= insc[nb * p.cin + ci0 + kk]; }
            }
            As[kk * 32u + mm / 4u][mm % 4u] = v;
        }
        for (var e = 0u; e < 4u; e++) {
            let kk = kb + e * 4u;
            var wv = 0.0;
            if (c0 + cc < p.cout) { wv = wt[(k0 + kk) * p.cout + c0 + cc]; }
            Bs[kk * 16u + cc / 4u][cc % 4u] = wv;
        }
        workgroupBarrier();
        for (var kk = 0u; kk < 16u; kk++) {
            let lo = As[kk * 32u + lid.x * 2u];
            let hi = As[kk * 32u + lid.x * 2u + 1u];
            let bv = Bs[kk * 16u + lid.y];
            a0 += lo.x * bv; a1 += lo.y * bv; a2 += lo.z * bv; a3 += lo.w * bv;
            a4 += hi.x * bv; a5 += hi.y * bv; a6 += hi.z * bv; a7 += hi.w * bv;
        }
        workgroupBarrier();
    }
    let mb = m0 + lid.x * 8u;
    let cb = c0 + lid.y * 4u;
    put(mb, cb, a0); put(mb + 1u, cb, a1); put(mb + 2u, cb, a2); put(mb + 3u, cb, a3);
    put(mb + 4u, cb, a4); put(mb + 5u, cb, a5); put(mb + 6u, cb, a6); put(mb + 7u, cb, a7);
}
"#;

/// The recurrence with one thread per row of R (768 per workgroup): three times the loads in
/// flight per step; the gate rows meet in workgroup memory and the first 256 threads combine them.
pub(crate) fn recurrence(lines: usize, f16: bool, unroll: usize) -> String {
    // The q loop in blocks of UNROLL: all loads of a block are issued before its FMAs, so many
    // L2 round trips overlap instead of one per iteration.
    let mut loads = String::new();
    for k in 0..unroll {
        loads += &if f16 {
            format!("            let w{k} = vec4<f32>(r[orow + q0 + {k}u]);\n")
        } else {
            format!("            let w{k} = r[orow + q0 + {k}u];\n")
        };
    }
    let (mut init, mut acc, mut fma, mut part, mut gate) = (String::new(), String::new(), String::new(), String::new(), String::new());
    for l in 0..lines {
        init += &format!("    var len{l} = 0u;\n    if (n0 + {l}u < d.n) {{ len{l} = lens[n0 + {l}u]; }}\n");
        acc += &format!("        var a{l} = 0.0;\n");
        let terms: Vec<String> = (0..unroll).map(|k| format!("dot(w{k}, h[{l}u * 64u + q0 + {k}u])")).collect();
        fma += &format!("            a{l} += {};\n", terms.join(" + "));
        part += &format!("        part[{l}u * 768u + j] = a{l};\n");
        gate += &format!(
            "            if (s < len{l}) {{
                var t = s;
                if (dir == 1u) {{ t = len{l} - 1u - s; }}
                let gb = (t * d.n + n0 + {l}u) * 1536u + dir * 768u;
                let z = sigmoid(g[gb + j] + part[{l}u * 768u + j] + bz);
                let rr = sigmoid(g[gb + 256u + j] + part[{l}u * 768u + 256u + j] + br);
                let c = tanh(g[gb + 512u + j] + rr * (part[{l}u * 768u + 512u + j] + bh));
                let hn = (1.0 - z) * c + z * h[{l}u * 64u + j / 4u][j % 4u];
                h[{l}u * 64u + j / 4u][j % 4u] = hn;
                y[(t * d.n + n0 + {l}u) * 512u + dir * 256u + j] = hn;
            }}
"
        );
    }
    format!(
        r#"
struct Dims {{ t: u32, n: u32, pad0: u32, pad1: u32 }}
@group(0) @binding(0) var<storage, read> g: array<f32>;
@group(0) @binding(1) var<storage, read> r: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> rb: array<f32>;
@group(0) @binding(3) var<storage, read_write> y: array<f32>;
@group(0) @binding(4) var<uniform> d: Dims;
@group(0) @binding(5) var<storage, read> lens: array<u32>;
var<workgroup> h: array<vec4<f32>, {hsize}>;
var<workgroup> part: array<f32, {psize}>;

fn sigmoid(v: f32) -> f32 {{ return 1.0 / (1.0 + exp(-v)); }}

@compute @workgroup_size(768)
fn main(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {{
    let j = lid.x;
    let n0 = wid.x * {lines}u;
    let dir = wid.y;
    let orow = (dir * 768u + j) * 64u;
    let jb = dir * 768u + (j % 256u);
    let bz = rb[jb];
    let br = rb[jb + 256u];
    let bh = rb[jb + 512u];
{init}    for (var i = j; i < {hsize}u; i += 768u) {{ h[i] = vec4<f32>(0.0); }}
    workgroupBarrier();
    for (var s = 0u; s < d.t; s++) {{
{acc}        for (var q0 = 0u; q0 < 64u; q0 += {unroll}u) {{
{loads}{fma}        }}
{part}        workgroupBarrier();
        if (j < 256u) {{
{gate}        }}
        workgroupBarrier();
    }}
}}
"#,
        hsize = lines * 64,
        psize = lines * 768
    )
}

pub(crate) fn shader(src: &str) -> String {
    src.replace("EPILOGUE_FN", EPILOGUE).replace("ENTRY", ENTRY).replace("INDEX", INDEX)
}
