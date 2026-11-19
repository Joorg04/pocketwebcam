//! Pure image maths for background blur and virtual backgrounds: turning the model's
//! person probabilities into a smooth full-resolution alpha mask, blurring a frame, and
//! blending the person over a background. No model code here, so it's fully unit-tested.
//!
//! Frames are packed YUY2 (`Y0 U Y1 V` per pair of pixels, BT.601 limited range), `w*h*2`
//! bytes, row stride `w*2`. For an odd width the last pixel of a row has only `Y U`: byte
//! `2x` is pixel x's luma and byte `2x+1` is U for even x, V for odd x.

/// How much of the previous frame's mask is kept by [`mask_from_probs`] (temporal smoothing
/// against flicker; higher = steadier but laggier edges).
pub const TEMPORAL_KEEP: f32 = 0.5;

/// A person mask at model resolution. `alpha` is row-major, 255 = person, 0 = background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<u8>,
}

/// Builds a mask from model probabilities (`w*h`, 0.0..=1.0; missing or NaN values count as
/// background): blends with `prev` (`out = keep*prev + (1-keep)*new`, skipped when `prev` has
/// a different size), then feathers with a 3x3 box filter.
pub fn mask_from_probs(probs: &[f32], w: usize, h: usize, prev: Option<&Mask>, keep: f32) -> Mask {
    let n = w * h;
    let keep = if keep.is_nan() {
        0.0
    } else {
        keep.clamp(0.0, 1.0)
    };
    let prev = prev.filter(|p| p.width == w && p.height == h && p.alpha.len() == n);
    let raw: Vec<u8> = (0..n)
        .map(|i| {
            let p = probs.get(i).copied().unwrap_or(0.0);
            let mut a = if p.is_nan() {
                0.0
            } else {
                p.clamp(0.0, 1.0) * 255.0
            };
            if let Some(prev) = prev {
                a = keep * prev.alpha[i] as f32 + (1.0 - keep) * a;
            }
            a.round() as u8
        })
        .collect();

    // 3x3 box feather, edges clamped.
    let mut alpha = vec![0u8; n];
    for y in 0..h {
        let rows = [y.saturating_sub(1), y, (y + 1).min(h - 1)];
        for x in 0..w {
            let cols = [x.saturating_sub(1), x, (x + 1).min(w - 1)];
            let mut sum = 0u32;
            for r in rows {
                for c in cols {
                    sum += raw[r * w + c] as u32;
                }
            }
            alpha[y * w + x] = ((sum + 4) / 9) as u8;
        }
    }
    Mask {
        width: w,
        height: h,
        alpha,
    }
}

/// Smoothstep from 0.2 to 0.8, so the bilinear ramp of an upscaled mask becomes a short,
/// soft edge instead of a wide halo.
static EDGE_LUT: [u8; 256] = edge_lut();

const fn edge_lut() -> [u8; 256] {
    const LO: u64 = 51;
    const HI: u64 = 204;
    const D: u64 = HI - LO;
    let mut lut = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let t = if (i as u64) <= LO {
            0
        } else if (i as u64) >= HI {
            D
        } else {
            i as u64 - LO
        };
        let num = 255 * t * t * (3 * D - 2 * t);
        lut[i] = ((num + D * D * D / 2) / (D * D * D)) as u8;
        i += 1;
    }
    lut
}

/// Per-output-coordinate source indices and 8-bit weights for a pixel-centre-aligned
/// bilinear resample from `src` to `dst` samples: `(i0, i1, f)` with value
/// `(s[i0]*(256-f) + s[i1]*f) / 256`.
fn resample_table(src: usize, dst: usize, table: &mut Vec<(u32, u32, u32)>) {
    table.clear();
    for x in 0..dst {
        // pos = ((x + 0.5) * src / dst - 0.5) in 1/256 units.
        let pos = ((2 * x + 1) * src * 256 / (2 * dst)) as i64 - 128;
        let pos = pos.max(0) as usize;
        let (i0, f) = (pos >> 8, (pos & 255) as u32);
        if i0 + 1 >= src {
            table.push((src as u32 - 1, src as u32 - 1, 0));
        } else {
            table.push((i0 as u32, i0 as u32 + 1, f));
        }
    }
}

/// Upscales `mask` to `w*h` per-pixel alpha in `out` (bilinear, then an edge-sharpening
/// curve), reusing `out`'s allocation. An empty or malformed mask gives all 255 (frame shown
/// unchanged).
pub fn upscale_mask(mask: &Mask, w: usize, h: usize, out: &mut Vec<u8>) {
    out.clear();
    out.resize(w * h, 255);
    let (mw, mh) = (mask.width, mask.height);
    if mw == 0 || mh == 0 || mask.alpha.len() < mw * mh || w == 0 || h == 0 {
        return;
    }
    let mut xt = Vec::with_capacity(w);
    resample_table(mw, w, &mut xt);
    let mut yt = Vec::with_capacity(h);
    resample_table(mh, h, &mut yt);
    let mut tmp = vec![0u32; mw];
    for (row, &(y0, y1, fy)) in out.chunks_exact_mut(w).zip(&yt) {
        let r0 = &mask.alpha[y0 as usize * mw..][..mw];
        let r1 = &mask.alpha[y1 as usize * mw..][..mw];
        for ((t, &a), &b) in tmp.iter_mut().zip(r0).zip(r1) {
            *t = a as u32 * (256 - fy) + b as u32 * fy;
        }
        for (o, &(x0, x1, fx)) in row.iter_mut().zip(&xt) {
            let v = (tmp[x0 as usize] * (256 - fx) + tmp[x1 as usize] * fx + 32768) >> 16;
            *o = EDGE_LUT[v as usize];
        }
    }
}

/// Fills `out` with a black `w*h` YUY2 frame.
fn black_yuy2(out: &mut Vec<u8>, w: usize, h: usize) {
    out.clear();
    out.extend([16u8, 128].iter().cycle().take(w * h * 2));
}

/// Reusable scratch buffers for blurring frames (the "Blur" background).
#[derive(Default)]
pub struct Blur {
    planes: [Vec<u8>; 3],
    tmp: Vec<u8>,
    sums: Vec<u32>,
    acc: [Vec<u32>; 3],
    rows: [Vec<u32>; 3],
    xt: Vec<(u32, u32, u32)>,
    yt: Vec<(u32, u32, u32)>,
    out: Vec<u8>,
}

impl Blur {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `src` (YUY2, `w*h*2` bytes) heavily blurred, at full size. `strength` in
    /// 0.0..=1.0 (clamped) sets the radius; 0 still blurs a little. A short `src` gives a
    /// black frame.
    pub fn run(&mut self, src: &[u8], w: usize, h: usize, strength: f32) -> &[u8] {
        if src.len() < w * h * 2 || w == 0 || h == 0 {
            black_yuy2(&mut self.out, w, h);
            return &self.out;
        }
        let (qw, qh) = (w.div_ceil(4), h.div_ceil(4));
        self.downscale(src, w, h, qw, qh);
        let strength = if strength.is_nan() {
            0.0
        } else {
            strength.clamp(0.0, 1.0)
        };
        let radius = (1.0 + strength * 10.0).round() as usize;
        for p in 0..3 {
            for _ in 0..3 {
                box_rows(&mut self.planes[p], &mut self.tmp, qw, qh, radius);
                box_cols(
                    &mut self.planes[p],
                    &mut self.tmp,
                    &mut self.sums,
                    qw,
                    qh,
                    radius,
                );
            }
        }
        self.upscale(w, h, qw, qh);
        &self.out
    }

    /// Area-averages 4x4 pixel cells into three `qw*qh` planes (Y, U, V).
    fn downscale(&mut self, src: &[u8], w: usize, h: usize, qw: usize, qh: usize) {
        for p in 0..3 {
            self.planes[p].clear();
            self.planes[p].resize(qw * qh, 0);
            self.acc[p].clear();
            self.acc[p].resize(qw, 0);
        }
        let full = w / 4;
        for qy in 0..qh {
            let rows = (qy * 4..(qy * 4 + 4).min(h)).count();
            for a in &mut self.acc {
                a.fill(0);
            }
            let [ay, au, av] = &mut self.acc;
            for y in qy * 4..qy * 4 + rows {
                let line = &src[y * w * 2..][..w * 2];
                let (cells, tail) = line.split_at(full * 8);
                for (((c, sy), su), sv) in cells
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .zip(ay.iter_mut())
                    .zip(au.iter_mut())
                    .zip(av.iter_mut())
                {
                    *sy += c[0] as u32 + c[2] as u32 + c[4] as u32 + c[6] as u32;
                    *su += c[1] as u32 + c[5] as u32;
                    *sv += c[3] as u32 + c[7] as u32;
                }
                for (i, &b) in tail.iter().enumerate() {
                    let target = if i % 2 == 0 {
                        &mut ay[full]
                    } else if i % 4 == 1 {
                        &mut au[full]
                    } else {
                        &mut av[full]
                    };
                    *target += b as u32;
                }
            }
            let rows = rows as u32;
            for qx in 0..qw {
                let nx = (w - qx * 4).min(4) as u32;
                let counts = [nx * rows, nx.div_ceil(2) * rows, (nx / 2) * rows];
                for ((plane, acc), c) in self.planes.iter_mut().zip(&self.acc).zip(counts) {
                    // A last cell one pixel wide has no V sample: reuse its neighbour's.
                    plane[qy * qw + qx] = match (acc[qx] + c / 2).checked_div(c) {
                        Some(v) => v as u8,
                        None if qx > 0 => plane[qy * qw + qx - 1],
                        None => 128,
                    };
                }
            }
        }
    }

    /// Bilinearly upscales the three planes back into a `w*h` YUY2 frame in `self.out`.
    fn upscale(&mut self, w: usize, h: usize, qw: usize, qh: usize) {
        self.out.clear();
        self.out.resize(w * h * 2, 0);
        resample_table(qw, w, &mut self.xt);
        resample_table(qh, h, &mut self.yt);
        for r in &mut self.rows {
            r.clear();
            r.resize(qw, 0);
        }
        let [ry, ru, rv] = &mut self.rows;
        for (line, &(y0, y1, fy)) in self.out.chunks_exact_mut(w * 2).zip(&self.yt) {
            for (plane, row) in self.planes.iter().zip([&mut *ry, &mut *ru, &mut *rv]) {
                let r0 = &plane[y0 as usize * qw..][..qw];
                let r1 = &plane[y1 as usize * qw..][..qw];
                for ((t, &a), &b) in row.iter_mut().zip(r0).zip(r1) {
                    *t = a as u32 * (256 - fy) + b as u32 * fy;
                }
            }
            let lerp = |row: &[u32], (x0, x1, fx): (u32, u32, u32)| {
                ((row[x0 as usize] * (256 - fx) + row[x1 as usize] * fx + 32768) >> 16) as u8
            };
            let (pairs, tail) = line.as_chunks_mut::<4>();
            for (px, xs) in pairs.iter_mut().zip(self.xt.as_chunks::<2>().0) {
                px[0] = lerp(ry, xs[0]);
                px[1] = lerp(ru, xs[0]);
                px[2] = lerp(ry, xs[1]);
                px[3] = lerp(rv, xs[1]);
            }
            if tail.len() == 2 {
                let xs = self.xt[w - 1];
                tail[0] = lerp(ry, xs);
                tail[1] = lerp(ru, xs);
            }
        }
    }
}

/// Reciprocal of `2r+1` in 1/65536 units, for an O(1) box average.
fn box_inv(radius: usize) -> u32 {
    (65536 + radius as u32) / (2 * radius as u32 + 1)
}

/// Horizontal sliding-window box blur of each row (edges clamped), in place via `tmp`.
fn box_rows(plane: &mut [u8], tmp: &mut Vec<u8>, w: usize, h: usize, r: usize) {
    tmp.clear();
    tmp.resize(w * h, 0);
    let inv = box_inv(r);
    let last = w - 1;
    for (src, dst) in plane.chunks_exact(w).zip(tmp.chunks_exact_mut(w)) {
        let mut sum: u32 = (0..=2 * r)
            .map(|i| src[i.saturating_sub(r).min(last)] as u32)
            .sum();
        for x in 0..w {
            dst[x] = ((sum * inv + 32768) >> 16) as u8;
            sum += src[(x + r + 1).min(last)] as u32;
            sum -= src[x.saturating_sub(r)] as u32;
        }
    }
    plane.copy_from_slice(tmp);
}

/// Vertical sliding-window box blur of each column (edges clamped), a row at a time so the
/// inner loops run along contiguous memory.
fn box_cols(
    plane: &mut [u8],
    tmp: &mut Vec<u8>,
    sums: &mut Vec<u32>,
    w: usize,
    h: usize,
    r: usize,
) {
    tmp.clear();
    tmp.resize(w * h, 0);
    sums.clear();
    sums.resize(w, 0);
    let inv = box_inv(r);
    let last = h - 1;
    for i in 0..=2 * r {
        let row = &plane[i.saturating_sub(r).min(last) * w..][..w];
        for (s, &v) in sums.iter_mut().zip(row) {
            *s += v as u32;
        }
    }
    for y in 0..h {
        let dst = &mut tmp[y * w..][..w];
        for (d, &s) in dst.iter_mut().zip(sums.iter()) {
            *d = ((s * inv + 32768) >> 16) as u8;
        }
        let add = &plane[(y + r + 1).min(last) * w..][..w];
        let sub = &plane[y.saturating_sub(r) * w..][..w];
        for ((s, &a), &b) in sums.iter_mut().zip(add).zip(sub) {
            *s = *s + a as u32 - b as u32;
        }
    }
    plane.copy_from_slice(tmp);
}

/// `(a*f + (255-a)*b) / 255`, rounded. Fits in u16, so the block loop below vectorises well.
#[inline(always)]
fn blend(a: u16, f: u8, b: u8) -> u8 {
    let t = a * f as u16 + (255 - a) * b as u16 + 128;
    ((t + (t >> 8)) >> 8) as u8
}

/// Pixels per block in [`composite_yuy2`].
const BLOCK: usize = 16;

/// Blends one block of `BLOCK` pixels.
#[inline(always)]
fn blend_block(f: &mut [u8; BLOCK * 2], b: &[u8; BLOCK * 2], a: &[u8; BLOCK]) {
    // Alpha per byte: own alpha for Y, the pair's mean for U and V.
    let mut ab = [0u16; BLOCK * 2];
    for (i, pair) in a.as_chunks::<2>().0.iter().enumerate() {
        let (a0, a1) = (pair[0] as u16, pair[1] as u16);
        let ac = (a0 + a1 + 1) >> 1;
        ab[i * 4..i * 4 + 4].copy_from_slice(&[a0, ac, a1, ac]);
    }
    for ((f, &b), &a) in f.iter_mut().zip(b).zip(&ab) {
        *f = blend(a, *f, b);
    }
}

/// Blends `frame` (person) over `background` in place, both YUY2 `w*h*2`, with per-pixel
/// `alpha` (`w*h`, 255 = keep frame). Luma uses each pixel's alpha, chroma the pair's mean.
/// Does nothing if any buffer is too short.
pub fn composite_yuy2(frame: &mut [u8], background: &[u8], alpha: &[u8], w: usize, h: usize) {
    let n = w * h;
    if frame.len() < n * 2 || background.len() < n * 2 || alpha.len() < n || n == 0 {
        return;
    }
    for ((line, bg), al) in frame
        .chunks_exact_mut(w * 2)
        .zip(background.chunks_exact(w * 2))
        .zip(alpha.chunks_exact(w))
        .take(h)
    {
        let (blocks, rest) = line.as_chunks_mut::<{ BLOCK * 2 }>();
        let (bblocks, brest) = bg.as_chunks::<{ BLOCK * 2 }>();
        let (ablocks, arest) = al.as_chunks::<BLOCK>();
        for ((f, b), a) in blocks.iter_mut().zip(bblocks).zip(ablocks) {
            blend_block(f, b, a);
        }
        let (pairs, tail) = rest.as_chunks_mut::<4>();
        let (bpairs, btail) = brest.as_chunks::<4>();
        let (apairs, atail) = arest.as_chunks::<2>();
        for ((f, b), a) in pairs.iter_mut().zip(bpairs).zip(apairs) {
            let (a0, a1) = (a[0] as u16, a[1] as u16);
            let ac = (a0 + a1 + 1) >> 1;
            f[0] = blend(a0, f[0], b[0]);
            f[1] = blend(ac, f[1], b[1]);
            f[2] = blend(a1, f[2], b[2]);
            f[3] = blend(ac, f[3], b[3]);
        }
        if tail.len() == 2 {
            let a = atail[0] as u16;
            tail[0] = blend(a, tail[0], btail[0]);
            tail[1] = blend(a, tail[1], btail[1]);
        }
    }
}

/// BT.601 limited-range Y, U, V from 0..=255 R, G, B.
fn rgb_to_yuv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    (
        16.0 + 0.257 * r + 0.504 * g + 0.098 * b,
        128.0 - 0.148 * r - 0.291 * g + 0.439 * b,
        128.0 + 0.439 * r - 0.368 * g - 0.071 * b,
    )
}

/// Samples RGB at continuous source position (`sx`, `sy`) covering `fw` x `fh` source pixels
/// per output pixel: box average when shrinking, bilinear when enlarging.
fn sample_rgb(rgba: &[u8], iw: usize, ih: usize, sx: f32, sy: f32, fw: f32, fh: f32) -> [f32; 3] {
    let px = |x: usize, y: usize| {
        let o = (y * iw + x) * 4;
        [rgba[o] as f32, rgba[o + 1] as f32, rgba[o + 2] as f32]
    };
    if fw > 1.0 || fh > 1.0 {
        let x0 = ((sx - fw / 2.0).floor().max(0.0) as usize).min(iw - 1);
        let x1 = ((sx + fw / 2.0).ceil() as usize).clamp(x0 + 1, iw);
        let y0 = ((sy - fh / 2.0).floor().max(0.0) as usize).min(ih - 1);
        let y1 = ((sy + fh / 2.0).ceil() as usize).clamp(y0 + 1, ih);
        let mut acc = [0.0f32; 3];
        for y in y0..y1 {
            for x in x0..x1 {
                let p = px(x, y);
                for c in 0..3 {
                    acc[c] += p[c];
                }
            }
        }
        let n = ((x1 - x0) * (y1 - y0)) as f32;
        return acc.map(|v| v / n);
    }
    let fx = (sx - 0.5).clamp(0.0, (iw - 1) as f32);
    let fy = (sy - 0.5).clamp(0.0, (ih - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(iw - 1), (y0 + 1).min(ih - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bot = c[i] + (d[i] - c[i]) * tx;
        top + (bot - top) * ty
    })
}

/// Converts an RGBA image (`iw*ih*4`) to a `w*h` YUY2 frame, scaled to cover the frame and
/// centre-cropped. Malformed input gives a black frame.
pub fn rgba_to_yuy2_cover(rgba: &[u8], iw: usize, ih: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::new();
    black_yuy2(&mut out, w, h);
    if iw == 0 || ih == 0 || rgba.len() < iw * ih * 4 || w == 0 || h == 0 {
        return out;
    }
    // Source pixels per output pixel, and the top-left of the centred crop.
    let step = (iw as f32 / w as f32).min(ih as f32 / h as f32);
    let ox = (iw as f32 - w as f32 * step) / 2.0;
    let oy = (ih as f32 - h as f32 * step) / 2.0;
    let q = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    for y in 0..h {
        let sy = oy + (y as f32 + 0.5) * step;
        let line = &mut out[y * w * 2..][..w * 2];
        let mut x = 0;
        while x < w {
            let s0 = sample_rgb(rgba, iw, ih, ox + (x as f32 + 0.5) * step, sy, step, step);
            let (y0, u0, v0) = rgb_to_yuv(s0[0], s0[1], s0[2]);
            line[x * 2] = q(y0);
            if x + 1 < w {
                let sx1 = ox + (x as f32 + 1.5) * step;
                let s1 = sample_rgb(rgba, iw, ih, sx1, sy, step, step);
                let (y1, u1, v1) = rgb_to_yuv(s1[0], s1[1], s1[2]);
                line[x * 2 + 1] = q((u0 + u1) / 2.0);
                line[x * 2 + 2] = q(y1);
                line[x * 2 + 3] = q((v0 + v1) / 2.0);
            } else {
                line[x * 2 + 1] = q(u0);
            }
            x += 2;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of one colour; each row starts with a whole `Y U Y V` pair.
    fn solid(w: usize, h: usize, px: [u8; 4]) -> Vec<u8> {
        let row: Vec<u8> = px.iter().copied().cycle().take(w * 2).collect();
        row.repeat(h)
    }

    /// A deterministic "busy" frame with every byte varying.
    fn noisy(w: usize, h: usize) -> Vec<u8> {
        let mut s = 12345u32;
        (0..w * h * 2)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 24) as u8
            })
            .collect()
    }

    fn checker(w: usize, h: usize) -> Vec<u8> {
        let mut f = solid(w, h, [16, 128, 16, 128]);
        for y in 0..h {
            for x in 0..w {
                f[(y * w + x) * 2] = if (x / 8 + y / 8) % 2 == 0 { 30 } else { 220 };
            }
        }
        f
    }

    fn luma_variance(f: &[u8]) -> f64 {
        let ys: Vec<f64> = f.iter().step_by(2).map(|&v| v as f64).collect();
        let mean = ys.iter().sum::<f64>() / ys.len() as f64;
        ys.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / ys.len() as f64
    }

    #[test]
    fn full_alpha_keeps_frame() {
        let (w, h) = (37, 9);
        let orig = noisy(w, h);
        let mut frame = orig.clone();
        let bg = solid(w, h, [200, 30, 200, 30]);
        composite_yuy2(&mut frame, &bg, &vec![255; w * h], w, h);
        assert_eq!(frame, orig);
    }

    #[test]
    fn zero_alpha_gives_background() {
        let (w, h) = (36, 10);
        let mut frame = noisy(w, h);
        let bg = noisy(w + 1, h)[..w * h * 2].to_vec();
        composite_yuy2(&mut frame, &bg, &vec![0; w * h], w, h);
        assert_eq!(frame, bg);
    }

    #[test]
    fn half_mask_splits_left_and_right() {
        let (w, h) = (64, 36);
        let fpx = [180, 90, 180, 160];
        let bpx = [40, 150, 40, 100];
        let mut frame = solid(w, h, fpx);
        let bg = solid(w, h, bpx);
        let small = Mask {
            width: 16,
            height: 9,
            alpha: (0..16 * 9)
                .map(|i| if i % 16 < 8 { 255 } else { 0 })
                .collect(),
        };
        let mut alpha = Vec::new();
        upscale_mask(&small, w, h, &mut alpha);
        assert_eq!(alpha.len(), w * h);
        composite_yuy2(&mut frame, &bg, &alpha, w, h);
        for y in 0..h {
            assert_eq!(&frame[y * w * 2..][..8], &[fpx, fpx].concat()[..]);
            assert_eq!(
                &frame[y * w * 2 + (w - 4) * 2..][..8],
                &[bpx, bpx].concat()[..]
            );
        }
    }

    #[test]
    fn upscale_of_uniform_mask_is_uniform_and_reuses_buffer() {
        let m = Mask {
            width: 4,
            height: 3,
            alpha: vec![255; 12],
        };
        let mut out = Vec::with_capacity(100 * 50);
        let cap = out.capacity();
        upscale_mask(&m, 100, 50, &mut out);
        assert!(out.iter().all(|&a| a == 255));
        assert_eq!(out.capacity(), cap);
        let zero = Mask {
            alpha: vec![0; 12],
            ..m
        };
        upscale_mask(&zero, 7, 3, &mut out);
        assert_eq!(out, vec![0; 21]);
        // Malformed mask: frame shown unchanged.
        let bad = Mask {
            width: 4,
            height: 3,
            alpha: vec![0; 5],
        };
        upscale_mask(&bad, 5, 5, &mut out);
        assert_eq!(out, vec![255; 25]);
    }

    #[test]
    fn edge_lut_is_monotonic_and_spans_full_range() {
        assert_eq!(EDGE_LUT[0], 0);
        assert_eq!(EDGE_LUT[255], 255);
        assert!(EDGE_LUT.windows(2).all(|p| p[0] <= p[1]));
        assert!((EDGE_LUT[128] as i32 - 128).abs() <= 2);
    }

    #[test]
    fn ema_lands_halfway() {
        let (w, h) = (8, 4);
        let prev = Mask {
            width: w,
            height: h,
            alpha: vec![0; w * h],
        };
        let m = mask_from_probs(&vec![1.0; w * h], w, h, Some(&prev), 0.5);
        assert!(
            m.alpha.iter().all(|&a| (127..=129).contains(&a)),
            "{:?}",
            m.alpha
        );
        let first = mask_from_probs(&vec![1.0; w * h], w, h, None, 0.5);
        assert!(first.alpha.iter().all(|&a| a == 255));
    }

    #[test]
    fn wrong_size_prev_is_ignored() {
        let prev = Mask {
            width: 3,
            height: 3,
            alpha: vec![0; 9],
        };
        let m = mask_from_probs(&[1.0; 12], 4, 3, Some(&prev), 0.9);
        assert_eq!(m.alpha, vec![255; 12]);
    }

    #[test]
    fn mask_handles_bad_probs() {
        let m = mask_from_probs(&[f32::NAN, 2.0, -1.0], 2, 2, None, TEMPORAL_KEEP);
        assert_eq!(m.alpha.len(), 4);
        let empty = mask_from_probs(&[], 0, 0, None, TEMPORAL_KEEP);
        assert!(empty.alpha.is_empty());
    }

    #[test]
    fn feather_softens_a_single_spike() {
        let mut probs = vec![0.0; 25];
        probs[12] = 1.0;
        let m = mask_from_probs(&probs, 5, 5, None, TEMPORAL_KEEP);
        assert_eq!(m.alpha[12], 28);
        assert_eq!(m.alpha[6], 28);
        assert_eq!(m.alpha[0], 0);
    }

    #[test]
    fn blur_keeps_constant_frames_constant() {
        let px = [120, 90, 121, 170];
        for (w, h) in [(64, 36), (37, 11), (1920 / 8, 1080 / 8)] {
            let f = solid(w, h, px);
            let mut b = Blur::new();
            for strength in [0.0, 0.5, 1.0] {
                let out = b.run(&f, w, h, strength);
                assert_eq!(out.len(), w * h * 2);
                for (i, (&o, &s)) in out.iter().zip(&f).enumerate() {
                    assert!(
                        (o as i32 - s as i32).abs() <= 1,
                        "byte {i} of {w}x{h}: {o} vs {s}"
                    );
                }
            }
        }
    }

    #[test]
    fn blur_lowers_checkerboard_variance() {
        let (w, h) = (128, 72);
        let f = checker(w, h);
        let before = luma_variance(&f);
        let mut b = Blur::new();
        let weak = luma_variance(b.run(&f, w, h, 0.0));
        assert!(weak < before / 4.0, "{weak} vs {before}");
        // On noise, a stronger blur smooths more.
        let f = noisy(w, h);
        let weak = luma_variance(b.run(&f, w, h, 0.0));
        let strong = luma_variance(b.run(&f, w, h, 1.0));
        assert!(
            strong < weak && weak < luma_variance(&f),
            "{strong} vs {weak}"
        );
    }

    #[test]
    fn odd_and_tiny_sizes() {
        let mut b = Blur::new();
        for (w, h) in [(1, 1), (2, 1), (3, 2), (5, 7), (7, 3), (33, 17)] {
            let f = noisy(w, h);
            assert_eq!(b.run(&f, w, h, 0.7).len(), w * h * 2);
            let m = mask_from_probs(&[0.5; 6], 3, 2, None, TEMPORAL_KEEP);
            let mut a = Vec::new();
            upscale_mask(&m, w, h, &mut a);
            assert_eq!(a.len(), w * h);
            let mut frame = f.clone();
            composite_yuy2(&mut frame, &solid(w, h, [16, 128, 16, 128]), &a, w, h);
            assert_eq!(frame.len(), w * h * 2);
            assert_eq!(
                rgba_to_yuy2_cover(&[255; 4 * 6], 3, 2, w, h).len(),
                w * h * 2
            );
        }
        assert!(b.run(&[], 0, 0, 1.0).is_empty());
    }

    #[test]
    fn short_inputs_do_not_panic() {
        let (w, h) = (16, 8);
        let mut b = Blur::new();
        let out = b.run(&[1, 2, 3], w, h, 0.5);
        assert_eq!(out.len(), w * h * 2);
        let orig = noisy(w, h);
        let mut frame = orig.clone();
        composite_yuy2(&mut frame, &[0; 10], &[0; 10], w, h);
        assert_eq!(frame, orig);
        composite_yuy2(&mut frame[..5], &orig, &[0; 128], w, h);
        assert_eq!(
            rgba_to_yuy2_cover(&[0; 7], 4, 4, w, h),
            solid(w, h, [16, 128, 16, 128])
        );
        assert_eq!(rgba_to_yuy2_cover(&[], 0, 3, w, h).len(), w * h * 2);
    }

    #[test]
    fn cover_fit_of_solid_colour() {
        // Pure red, 3:4 source into a 16:9 frame (crops top and bottom).
        let (iw, ih) = (30, 40);
        let rgba: Vec<u8> = [255, 0, 0, 255].repeat(iw * ih);
        for (w, h) in [(64, 36), (9, 5), (640, 360)] {
            let out = rgba_to_yuy2_cover(&rgba, iw, ih, w, h);
            assert_eq!(out.len(), w * h * 2);
            for line in out.chunks_exact(w * 2) {
                for (i, &b) in line.iter().enumerate() {
                    let want = [82, 90, 82, 240][i % 4];
                    assert!((b as i32 - want).abs() <= 2, "byte {i} of {w}x{h}: {b}");
                }
            }
        }
    }

    #[test]
    fn cover_fit_crops_centre() {
        // Left half black, right half white, wide source into a square frame: only the
        // middle is kept, so the split stays in the middle.
        let (iw, ih) = (40, 10);
        let mut rgba = vec![255u8; iw * ih * 4];
        for y in 0..ih {
            for x in 0..iw / 2 {
                rgba[(y * iw + x) * 4..][..3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let (w, h) = (10, 10);
        let out = rgba_to_yuy2_cover(&rgba, iw, ih, w, h);
        assert!((out[0] as i32 - 16).abs() <= 2);
        assert!((out[(w - 1) * 2] as i32 - 235).abs() <= 2);
    }

    /// Run with `cargo test --release --lib background::composite -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_composite_1080p() {
        use std::hint::black_box;
        use std::time::Instant;
        let (w, h) = (1920, 1080);
        const ITERS: u32 = 100;
        let src = noisy(w, h);
        let mut frame = src.clone();
        let bg = rgba_to_yuy2_cover(&[90, 120, 200, 255].repeat(64 * 36), 64, 36, w, h);
        let probs: Vec<f32> = (0..256 * 144).map(|i| (i % 256) as f32 / 255.0).collect();
        let mask = mask_from_probs(&probs, 256, 144, None, TEMPORAL_KEEP);
        let mut alpha = Vec::new();
        upscale_mask(&mask, w, h, &mut alpha);
        let mut blur = Blur::new();

        let t = Instant::now();
        for _ in 0..ITERS {
            frame.copy_from_slice(&src);
            let b = blur.run(black_box(&src), w, h, 0.6);
            composite_yuy2(&mut frame, b, &alpha, w, h);
            black_box(&frame);
        }
        let blur_ms = t.elapsed().as_secs_f64() * 1000.0 / ITERS as f64;

        let t = Instant::now();
        for _ in 0..ITERS {
            frame.copy_from_slice(&src);
            composite_yuy2(&mut frame, black_box(&bg), &alpha, w, h);
            black_box(&frame);
        }
        let image_ms = t.elapsed().as_secs_f64() * 1000.0 / ITERS as f64;

        let t = Instant::now();
        for _ in 0..ITERS {
            upscale_mask(black_box(&mask), w, h, &mut alpha);
            black_box(&alpha);
        }
        let upscale_ms = t.elapsed().as_secs_f64() * 1000.0 / ITERS as f64;

        println!("1080p, mean of {ITERS} (each includes a 4 MB frame copy where noted):");
        println!("  (a) blur mode: copy + Blur::run + composite_yuy2: {blur_ms:.2} ms");
        println!("  (b) image mode: copy + composite_yuy2: {image_ms:.2} ms");
        println!("  (c) upscale_mask: {upscale_ms:.2} ms");
    }
}
