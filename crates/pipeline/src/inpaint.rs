//! Content-aware fill for Remove spots: exemplar-based completion (Wexler, Shechtman & Irani,
//! "Space-Time Completion of Video", 2007) with PatchMatch nearest-neighbour search (Barnes,
//! Shechtman, Finkelstein & Goldman, 2009), coarse to fine.
//!
//! Every pixel of the hole is voted for by the 7 × 7 patches that cover it, each copied from its
//! best match among the patches around the hole (and around the spot's source area, which also
//! seeds the fill). Alternating search and voting makes the fill continue the surrounding structure
//! instead of pasting one offset copy.
//!
//! The coarse levels are set by the hole's radius (4, 8, 16 … px), not by the render size, and the
//! random choices are a hash of the position, not a sequence: a preview and a full-size export
//! synthesize the same structure, and the export only refines it locally. A work region over
//! [`MAX_WORK`] pixels (a big stroke on a big export) is synthesized at a reduced scale and the
//! fill copied to full resolution from the image with the scaled-up offsets, which keeps memory and
//! time bounded and the detail full-size.
//!
//! ponytail: matches are translations only, so structure that changes angle or scale across the
//! hole (rows or tiles in strong perspective) can continue at the wrong angle; rotated and scaled
//! candidates (generalized PatchMatch) would fix that. Heal or Clone with a chosen source is the
//! workaround meanwhile.

use lightcraft_raster::Rgb32f;

/// Patch radius (7 × 7 patches).
const P: i64 = 3;
/// Hole radius (px) at the coarsest level.
const COARSEST: f32 = 4.0;
/// Most pixels synthesized on one level (~55 bytes each while searching): a bigger work region is
/// synthesized at a reduced scale, and the fill copied to full resolution from the image itself.
const MAX_WORK: usize = 3_000_000;

/// A level's nearest-neighbour field with the level's width, height and scale.
type Field = (Vec<(i32, i32)>, usize, usize, f32);

/// One pyramid level of the work region, in a perceptual (square-root) encoding.
struct Level {
    w: usize,
    h: usize,
    px: Vec<[f32; 3]>,
    hole: Vec<bool>,
    /// Scale relative to the image (≥ 1).
    k: f32,
}

/// Fill the pixels of `img` where `mask` is set (a `bw` × `bh` box at `(bx, by)`, row-major) with
/// texture from around them. `r` = the spot radius (px); `hint` = offset (px) to a source area that
/// is searched too and seeds the fill; `seed` varies the random choices between spots. Returns the
/// box's pixels (unmasked ones unchanged), or `None` when nothing around the hole can be copied
/// (the caller falls back to healing from `hint`).
#[allow(clippy::too_many_arguments)]
pub fn fill(img: &Rgb32f, bx: usize, by: usize, bw: usize, bh: usize, mask: &[bool], r: f32, hint: (f32, f32), seed: u64) -> Option<Vec<[f32; 3]>> {
    let (iw, ih) = (img.width as i64, img.height as i64);
    if mask.len() != bw.checked_mul(bh)? || bw == 0 || bh == 0 || !r.is_finite() || r <= 0.0 {
        return None;
    }
    let (bx, by, bw, bh) = (bx as i64, by as i64, bw as i64, bh as i64);
    if bx + bw > iw || by + bh > ih {
        return None;
    }
    let mut out = box_pixels(img, bx as usize, by as usize, bw as usize, bh as usize);
    if !mask.contains(&true) {
        return Some(out);
    }
    // hostile settings can carry any offset: keep it finite and on the image
    let cl = |v: f32, lim: i64| if v.is_finite() { (v.round() as i64).clamp(-lim, lim) } else { 0 };
    let (hx, hy) = (cl(hint.0, iw), cl(hint.1, ih));
    // the work region: the box with a margin of two radii, and the source area
    let pad = ((2.0 * r).ceil() as i64).saturating_add(P + 1);
    let rx0 = bx.saturating_sub(pad).min(bx + hx - P - 1).clamp(0, iw);
    let ry0 = by.saturating_sub(pad).min(by + hy - P - 1).clamp(0, ih);
    let rx1 = (bx + bw).saturating_add(pad).max(bx + bw + hx + P + 1).clamp(0, iw);
    let ry1 = (by + bh).saturating_add(pad).max(by + bh + hy + P + 1).clamp(0, ih);
    let (w, h) = ((rx1 - rx0) as usize, (ry1 - ry0) as usize);
    // whether the image pixel (x, y) is in the hole
    let in_hole = |x: i64, y: i64| {
        let (mx, my) = (x - bx, y - by);
        mx >= 0 && my >= 0 && mx < bw && my < bh && mask.get((my * bw + mx) as usize).copied().unwrap_or(false)
    };
    // the finest level synthesized (the region itself unless it's big), in a perceptual encoding
    let s0 = (w.saturating_mul(h) as f32 / MAX_WORK as f32).sqrt().max(1.0);
    let (fw, fh, px, hole) = boxed(w, h, s0, |x, y| {
        let (ax, ay) = (rx0 + x as i64, ry0 + y as i64);
        (!in_hole(ax, ay)).then(|| img.data.get(ay as usize * img.width + ax as usize).copied().unwrap_or_default().map(|c| c.max(0.0).sqrt()))
    });
    let fine = Level { w: fw, h: fh, px, hole, k: s0 };
    // coarser levels: hole radius 4, 8, 16 … px while clearly below the finest's
    let mut levels = Vec::new();
    let mut rl = COARSEST;
    while rl * 1.25 < r / s0 && levels.len() < 12 {
        let k = r / rl;
        let (lw, lh, px, hole) = boxed(fw, fh, k / s0, |x, y| (!fine.hole[y * fw + x]).then(|| fine.px[y * fw + x]));
        levels.push(Level { w: lw, h: lh, px, hole, k });
        rl *= 2.0;
    }
    levels.push(fine);
    let hint = (hx as f32, hy as f32);
    let mut prev: Option<Field> = None;
    let last = levels.len() - 1;
    for (li, lv) in levels.iter_mut().enumerate() {
        let nnf = synthesize(lv, hint, prev.as_ref(), li, li == last, seed)?;
        prev = Some((nnf, lv.w, lv.h, lv.k));
    }
    let (fine, (nnf, ..)) = (levels.last()?, prev?);
    for y in 0..bh {
        for x in 0..bw {
            let i = (y * bw + x) as usize;
            if !mask.get(i).copied().unwrap_or(false) {
                continue;
            }
            let (ax, ay) = (bx + x, by + y);
            let cell = |px: i64, py: i64| {
                let c = |v: i64, o: i64, n: usize| ((((v - o) as f32 + 0.5) / s0).max(0.0) as usize).min(n - 1);
                (c(px, rx0, fine.w), c(py, ry0, fine.h))
            };
            let v = if s0 <= 1.0 {
                // synthesized at full size: the level is the result
                let (cx, cy) = cell(ax, ay);
                fine.px[cy * fine.w + cx].map(|c| c * c)
            } else {
                // copy at full size: each patch around the pixel brings the pixel its (scaled-up)
                // offset points at, as the vote on the synthesized level did
                let (mut s, mut n) = ([0.0f32; 3], 0.0f32);
                for dy in -P..=P {
                    for dx in -P..=P {
                        let (cx, cy) = cell(ax + dx, ay + dy);
                        let q = nnf[cy * fine.w + cx];
                        if q.0 < 0 {
                            continue;
                        }
                        let (sx, sy) = (ax + ((q.0 as f32 - cx as f32) * s0).round() as i64, ay + ((q.1 as f32 - cy as f32) * s0).round() as i64);
                        if sx < 0 || sy < 0 || sx >= iw || sy >= ih || in_hole(sx, sy) {
                            continue;
                        }
                        let Some(p) = img.data.get((sy * iw + sx) as usize) else { continue };
                        s = [s[0] + p[0], s[1] + p[1], s[2] + p[2]];
                        n += 1.0;
                    }
                }
                let (cx, cy) = cell(ax, ay);
                if n > 0.0 { s.map(|c| c / n) } else { fine.px[cy * fine.w + cx].map(|c| c * c) }
            };
            if let Some(o) = out.get_mut(i) {
                *o = v;
            }
        }
    }
    Some(out)
}

fn box_pixels(img: &Rgb32f, bx: usize, by: usize, bw: usize, bh: usize) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(bw * bh);
    for y in by..by + bh {
        for x in bx..bx + bw {
            out.push(img.data.get(y * img.width + x).copied().unwrap_or_default());
        }
    }
    out
}

/// Box-average a `w` × `h` grid by `k` (≥ 1; 1 = as is). `get` gives a pixel, `None` in the hole;
/// a level pixel is a hole when any pixel it covers is.
fn boxed(w: usize, h: usize, k: f32, get: impl Fn(usize, usize) -> Option<[f32; 3]>) -> (usize, usize, Vec<[f32; 3]>, Vec<bool>) {
    let (lw, lh) = (((w as f32 / k).ceil() as usize).max(1), ((h as f32 / k).ceil() as usize).max(1));
    let span = |c: usize, lim: usize| {
        let a = ((c as f32 * k) as usize).min(lim.saturating_sub(1));
        (a, (((c + 1) as f32 * k) as usize).max(a + 1).min(lim))
    };
    let mut px = vec![[0.0f32; 3]; lw * lh];
    let mut hole = vec![false; lw * lh];
    for cy in 0..lh {
        let (y0, y1) = span(cy, h);
        for cx in 0..lw {
            let (x0, x1) = span(cx, w);
            let (mut s, mut n, mut any) = ([0.0f32; 3], 0.0f32, false);
            for y in y0..y1 {
                for x in x0..x1 {
                    match get(x, y) {
                        Some(p) => {
                            s = [s[0] + p[0], s[1] + p[1], s[2] + p[2]];
                            n += 1.0;
                        }
                        None => any = true,
                    }
                }
            }
            let i = cy * lw + cx;
            hole[i] = any;
            if n > 0.0 {
                px[i] = s.map(|c| c / n);
            }
        }
    }
    (lw, lh, px, hole)
}

/// A stateless random number: the same inputs give the same value whatever else was drawn.
fn rnd(seed: u64, a: u64, b: u64, c: u64) -> u64 {
    let mut z = seed ^ a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ c.wrapping_mul(0x1656_67B1_9E37_79F9);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Run search and voting on one level: fills `lv.px` in the hole and returns its nearest-neighbour
/// field (source patch centre per pixel; meaningful where a patch touches the hole). `li` counts
/// levels from the coarsest; `prev` is the coarser level's field, size and scale.
fn synthesize(lv: &mut Level, hint: (f32, f32), prev: Option<&Field>, li: usize, finest: bool, seed: u64) -> Option<Vec<(i32, i32)>> {
    let (w, h) = (lv.w, lv.h);
    let (wi, hi) = (w as i64, h as i64);
    // summed-area table of hole pixels
    let mut sat = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += u32::from(lv.hole[y * w + x]);
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let holes_in = |x0: i64, y0: i64, x1: i64, y1: i64| {
        // inclusive rect clipped to the level
        let (x0, y0, x1, y1) = (x0.max(0) as usize, y0.max(0) as usize, (x1 + 1).min(wi) as usize, (y1 + 1).min(hi) as usize);
        if x1 <= x0 || y1 <= y0 {
            return 0;
        }
        sat[y1 * (w + 1) + x1] + sat[y0 * (w + 1) + x0] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0]
    };
    // valid sources: the whole patch on the level and outside the hole
    let mut ok = vec![false; w * h];
    let mut sources = Vec::new();
    let mut targets = Vec::new();
    let mut is_t = vec![false; w * h];
    for y in 0..hi {
        for x in 0..wi {
            let i = (y * wi + x) as usize;
            let inside = x >= P && y >= P && x + P < wi && y + P < hi;
            let n = holes_in(x - P, y - P, x + P, y + P);
            if inside && n == 0 {
                ok[i] = true;
                sources.push((x as i32, y as i32));
            }
            if n > 0 {
                is_t[i] = true;
                targets.push(i);
            }
        }
    }
    if sources.is_empty() {
        return None;
    }
    let lvl_salt = (lv.k * 1024.0) as u64;
    let pos = |t: usize| ((t % w) as u64) | ((t / w) as u64) << 32;
    let random_source = |t: usize| sources[(rnd(seed, lvl_salt, pos(t), 1) % sources.len() as u64) as usize];
    let valid = |q: (i64, i64)| q.0 >= 0 && q.1 >= 0 && q.0 < wi && q.1 < hi && ok[(q.1 * wi + q.0) as usize];
    // initial field: from the coarser level, else the hint
    // (-1, -1) where no patch touches the hole
    let mut nnf = vec![(-1i32, -1i32); w * h];
    for &t in &targets {
        let p = ((t % w) as i64, (t / w) as i64);
        let guess = match prev {
            Some((pf, pw, ph, pk)) => {
                let ratio = pk / lv.k;
                let pc = ((((p.0 as f32 + 0.5) / ratio) as usize).min(pw - 1), (((p.1 as f32 + 0.5) / ratio) as usize).min(ph - 1));
                let q = pf[pc.1 * pw + pc.0];
                let off = ((q.0 as f32 - pc.0 as f32) * ratio, (q.1 as f32 - pc.1 as f32) * ratio);
                if q.0 < 0 { (-1, -1) } else { (p.0 + off.0.round() as i64, p.1 + off.1.round() as i64) }
            }
            None => (p.0 + (hint.0 / lv.k).round() as i64, p.1 + (hint.1 / lv.k).round() as i64),
        };
        nnf[t] = if valid(guess) { (guess.0 as i32, guess.1 as i32) } else { random_source(t) };
    }
    // most iterations where the structure is decided; finer levels refine nearby
    let iters = if prev.is_none() {
        8
    } else if finest {
        2
    } else {
        8usize.saturating_sub(2 * li).max(2)
    };
    let field =
        Search { ok: &ok, is_t: &is_t, targets: &targets, iters, rmax: if prev.is_none() { w.max(h) as i64 } else { 6 }, seed, salt: lvl_salt };
    if prev.is_some() {
        vote(lv, &nnf, &is_t, None);
        field.run(lv, &mut nnf);
        return Some(nnf);
    }
    // Two starts on the coarsest level, keeping the more coherent result: the hint copied in (best
    // when it points along the structure), or a smooth fill grown in from the boundary with the hint
    // only a first candidate (best when it points at other texture, which a copy would lock in).
    let mut grown = nnf.clone();
    vote(lv, &nnf, &is_t, None);
    let copied = field.run(lv, &mut nnf);
    let copied_px = lv.px.clone();
    grow_in(lv);
    if field.run(lv, &mut grown) < copied {
        return Some(grown);
    }
    lv.px = copied_px;
    Some(nnf)
}

/// PatchMatch search and voting on one level (shared by the two coarsest-level starts).
struct Search<'a> {
    ok: &'a [bool],
    is_t: &'a [bool],
    targets: &'a [usize],
    iters: usize,
    rmax: i64,
    seed: u64,
    salt: u64,
}

impl Search<'_> {
    /// Alternate search and voting; returns the mean patch distance of the last search (lower =
    /// more coherent).
    fn run(&self, lv: &mut Level, nnf: &mut [(i32, i32)]) -> f32 {
        let (w, h) = (lv.w, lv.h);
        let (wi, hi) = (w as i64, h as i64);
        let (targets, is_t, seed, lvl_salt, rmax) = (self.targets, self.is_t, self.seed, self.salt, self.rmax);
        let pos = |t: usize| ((t % w) as u64) | ((t / w) as u64) << 32;
        let valid = |q: (i64, i64)| q.0 >= 0 && q.1 >= 0 && q.0 < wi && q.1 < hi && self.ok[(q.1 * wi + q.0) as usize];
        let mut cost = vec![f32::MAX; w * h];
        let mut energy = f32::MAX;
        for it in 0..self.iters {
            for &t in targets {
                let p = ((t % w) as i64, (t / w) as i64);
                let q = nnf[t];
                cost[t] = dist(lv, p, (q.0 as i64, q.1 as i64), f32::MAX);
            }
            for pass in 0..2u64 {
                let dir: i64 = if pass == 0 { 1 } else { -1 };
                let order: Box<dyn Iterator<Item = &usize>> = if pass == 0 { Box::new(targets.iter()) } else { Box::new(targets.iter().rev()) };
                for &t in order {
                    let p = ((t % w) as i64, (t / w) as i64);
                    let (mut best, mut bc) = ((nnf[t].0 as i64, nnf[t].1 as i64), cost[t]);
                    // propagation: the neighbour's match, shifted
                    for (dx, dy) in [(dir, 0), (0, dir)] {
                        let nb = (p.0 - dx, p.1 - dy);
                        if nb.0 < 0 || nb.1 < 0 || nb.0 >= wi || nb.1 >= hi || !is_t[(nb.1 * wi + nb.0) as usize] {
                            continue;
                        }
                        let m = nnf[(nb.1 * wi + nb.0) as usize];
                        let q = (m.0 as i64 + dx, m.1 as i64 + dy);
                        if q != best && valid(q) {
                            let c = dist(lv, p, q, bc);
                            if c < bc {
                                (best, bc) = (q, c);
                            }
                        }
                    }
                    // random search in shrinking windows around the best match
                    let mut rad = rmax;
                    let mut k = 0u64;
                    while rad >= 1 {
                        let z = rnd(seed, lvl_salt ^ ((it as u64) << 20) ^ (pass << 40), pos(t), k);
                        let span = (2 * rad + 1) as u64;
                        let q = (best.0 + (z % span) as i64 - rad, best.1 + ((z >> 32) % span) as i64 - rad);
                        if q != best && valid(q) {
                            let c = dist(lv, p, q, bc);
                            if c < bc {
                                (best, bc) = (q, c);
                            }
                        }
                        rad /= 2;
                        k += 1;
                    }
                    nnf[t] = (best.0 as i32, best.1 as i32);
                    cost[t] = bc;
                }
            }
            // weight the votes by how well each patch matched (Wexler et al.: σ = the 75th percentile)
            let mut cs: Vec<f32> = targets.iter().map(|&t| cost[t]).filter(|c| c.is_finite() && *c < f32::MAX).collect();
            let k75 = cs.len() * 3 / 4;
            let s2 = if k75 < cs.len() { *cs.select_nth_unstable_by(k75, f32::total_cmp).1 } else { 0.0 };
            let wts: Vec<f32> = cost.iter().map(|c| if s2 > 0.0 && *c < f32::MAX { (-c / (2.0 * s2)).exp() } else { 1.0 }).collect();
            vote(lv, nnf, is_t, Some(&wts));
            energy = cs.iter().sum::<f32>() / cs.len().max(1) as f32;
        }
        energy
    }
}

/// Fill the hole with the mean of its known neighbours, ring by ring from the boundary inward.
fn grow_in(lv: &mut Level) {
    let (w, h) = (lv.w, lv.h);
    let mut known: Vec<bool> = lv.hole.iter().map(|m| !m).collect();
    let mut todo: Vec<usize> = (0..w * h).filter(|&i| !known[i]).collect();
    while !todo.is_empty() {
        let mut ring = Vec::new();
        for &i in &todo {
            let (x, y) = (i % w, i / w);
            let (mut s, mut n) = ([0.0f32; 3], 0.0f32);
            for ny in y.saturating_sub(1)..(y + 2).min(h) {
                for nx in x.saturating_sub(1)..(x + 2).min(w) {
                    let j = ny * w + nx;
                    if known[j] {
                        let v = lv.px[j];
                        s = [s[0] + v[0], s[1] + v[1], s[2] + v[2]];
                        n += 1.0;
                    }
                }
            }
            if n > 0.0 {
                ring.push((i, s.map(|c| c / n)));
            }
        }
        if ring.is_empty() {
            return; // nothing known on the level
        }
        for &(i, v) in &ring {
            lv.px[i] = v;
            known[i] = true;
        }
        todo.retain(|&i| !known[i]);
    }
}

/// Mean squared difference between the patches at `p` (may leave the level) and `q` (a valid
/// source); stops early once it can't beat `limit`.
fn dist(lv: &Level, p: (i64, i64), q: (i64, i64), limit: f32) -> f32 {
    let (wi, hi) = (lv.w as i64, lv.h as i64);
    let stop = if limit == f32::MAX { f32::MAX } else { limit * ((2 * P + 1) * (2 * P + 1)) as f32 };
    let (mut s, mut n) = (0.0f32, 0u32);
    if p.0 >= P && p.1 >= P && p.0 + P < wi && p.1 + P < hi {
        // the whole patch on the level: compare row slices
        let len = (2 * P + 1) as usize;
        for dy in -P..=P {
            let (ia, ib) = (((p.1 + dy) * wi + p.0 - P) as usize, ((q.1 + dy) * wi + q.0 - P) as usize);
            let (Some(ra), Some(rb)) = (lv.px.get(ia..ia + len), lv.px.get(ib..ib + len)) else { return f32::MAX };
            for (a, b) in ra.iter().zip(rb) {
                s += (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
            }
            if s > stop {
                return f32::MAX;
            }
        }
        return s / (len * len) as f32;
    }
    for dy in -P..=P {
        let (ty, sy) = (p.1 + dy, q.1 + dy);
        if ty < 0 || ty >= hi {
            continue;
        }
        for dx in -P..=P {
            let tx = p.0 + dx;
            if tx < 0 || tx >= wi {
                continue;
            }
            let (a, b) = (lv.px[(ty * wi + tx) as usize], lv.px[(sy * wi + q.0 + dx) as usize]);
            s += (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
            n += 1;
        }
        if s > stop {
            return f32::MAX;
        }
    }
    if n == 0 { f32::MAX } else { s / n as f32 }
}

/// Each hole pixel becomes the mean of what the patches covering it copy there.
fn vote(lv: &mut Level, nnf: &[(i32, i32)], is_t: &[bool], wts: Option<&[f32]>) {
    let (w, h) = (lv.w as i64, lv.h as i64);
    let mut next = lv.px.clone();
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if !lv.hole[i] {
                continue;
            }
            let (mut s, mut n) = ([0.0f32; 3], 0.0f32);
            for py in (y - P).max(0)..=(y + P).min(h - 1) {
                for pxx in (x - P).max(0)..=(x + P).min(w - 1) {
                    let pi = (py * w + pxx) as usize;
                    if !is_t[pi] {
                        continue;
                    }
                    let q = nnf[pi];
                    let (sx, sy) = (q.0 as i64 + x - pxx, q.1 as i64 + y - py);
                    if let Some(v) = (sx >= 0 && sy >= 0 && sx < w && sy < h).then(|| lv.px[(sy * w + sx) as usize]) {
                        let k = wts.map_or(1.0, |ws| ws[pi]) + 1e-6;
                        s = [s[0] + v[0] * k, s[1] + v[1] * k, s[2] + v[2] * k];
                        n += k;
                    }
                }
            }
            if n > 0.0 {
                next[i] = s.map(|c| c / n);
            }
        }
    }
    lv.px = next;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc(bw: usize, bh: usize, c: (f32, f32), r: f32) -> Vec<bool> {
        (0..bw * bh).map(|i| (((i % bw) as f32 + 0.5 - c.0).powi(2) + ((i / bw) as f32 + 0.5 - c.1).powi(2)).sqrt() < r).collect()
    }

    /// Four quadrants meeting under the hole: one copied offset can't continue all four edges;
    /// synthesis extends each quadrant into it.
    #[test]
    fn continues_the_structure_around_the_hole() {
        let q = [[0.05, 0.05, 0.05], [0.8, 0.1, 0.1], [0.1, 0.6, 0.1], [0.1, 0.1, 0.9]];
        let img = Rgb32f::from_fn(160, 160, |x, y| q[usize::from(x >= 80) + 2 * usize::from(y >= 80)]);
        let r = 14.0;
        let mask = disc(40, 40, (20.0, 20.0), r);
        let out = fill(&img, 60, 60, 40, 40, &mask, r, (3.0 * r, 3.0 * r), 7).unwrap();
        let at = |x: usize, y: usize| out[(y - 60) * 40 + (x - 60)];
        for (x, y, want) in [(72, 72, 0), (88, 72, 1), (72, 88, 2), (88, 88, 3)] {
            let got = at(x, y);
            let err: f32 = (0..3).map(|c| (got[c] - q[want][c]).abs()).sum();
            assert!(err < 0.05, "({x}, {y}): {got:?} vs {:?}", q[want]);
        }
        // pixels outside the hole are untouched
        assert_eq!(at(60, 60), img.get(60, 60));
    }

    /// A work region over [`MAX_WORK`] is synthesized at a reduced scale and copied up to full size.
    #[test]
    fn big_regions_are_synthesized_at_a_reduced_scale() {
        let q = [[0.05, 0.05, 0.05], [0.8, 0.1, 0.1], [0.1, 0.6, 0.1], [0.1, 0.1, 0.9]];
        let img = Rgb32f::from_fn(1900, 1900, |x, y| q[usize::from(x >= 950) + 2 * usize::from(y >= 950)]);
        let (r, n) = (320.0, 642);
        let mask = disc(n, n, (321.0, 321.0), r);
        let out = fill(&img, 629, 629, n, n, &mask, r, (2.4 * r, 0.0), 7).unwrap();
        let at = |x: usize, y: usize| out[(y - 629) * n + (x - 629)];
        for (x, y, want) in [(800, 800, 0), (1100, 800, 1), (800, 1100, 2), (1100, 1100, 3)] {
            let got = at(x, y);
            let err: f32 = (0..3).map(|c| (got[c] - q[want][c]).abs()).sum();
            assert!(err < 0.05, "({x}, {y}): {got:?} vs {:?}", q[want]);
        }
    }

    #[test]
    fn deterministic() {
        let img = Rgb32f::from_fn(120, 90, |x, y| [((x * 7 + y * 3) % 11) as f32 / 11.0, (x % 5) as f32 / 5.0, 0.3]);
        let mask = disc(30, 30, (15.0, 15.0), 9.0);
        let a = fill(&img, 40, 30, 30, 30, &mask, 9.0, (25.0, 0.0), 3);
        assert!(a.is_some());
        assert_eq!(a, fill(&img, 40, 30, 30, 30, &mask, 9.0, (25.0, 0.0), 3));
    }

    /// The source offset is a candidate, not a template: pointed sideways across rows that converge
    /// (so they lean another way there), a copy of it would lock the wrong angle in.
    #[test]
    fn a_misleading_source_does_not_take_over() {
        let img = Rgb32f::from_fn(200, 140, |x, y| {
            if y < 40 {
                return [0.6, 0.4, 0.4];
            }
            let a = ((x as f32 - 100.0) / (y as f32 - 40.0)).atan();
            if (a * 40.0).sin() > 0.0 { [0.3, 0.12, 0.7] } else { [0.08, 0.07, 0.01] }
        });
        let (r, n, (bx, by)) = (9.0, 20, (40, 80));
        let mask: Vec<bool> =
            (0..n * n).map(|i| (((bx + i % n) as f32 + 0.5 - 50.0).powi(2) + ((by + i / n) as f32 + 0.5 - 90.0).powi(2)).sqrt() < r).collect();
        for seed in 1..=3 {
            let out = fill(&img, bx, by, n, n, &mask, r, (30.0, 0.0), seed).unwrap();
            let (mut err, mut k) = (0.0f32, 0.0f32);
            for (i, m) in mask.iter().enumerate() {
                if *m {
                    let want = img.get(bx + i % n, by + i / n);
                    err += (0..3).map(|c| (out[i][c] - want[c]).abs()).sum::<f32>();
                    k += 1.0;
                }
            }
            assert!(err / k < 0.25, "seed {seed}: mean error {}", err / k);
        }
    }

    /// Hostile or degenerate input: holes at the border or covering everything, odd hints, tiny
    /// images, wrong mask sizes — never a panic.
    #[test]
    fn degenerate_input_does_not_panic() {
        let img = Rgb32f::from_fn(64, 48, |x, y| [x as f32 / 64.0, y as f32 / 48.0, f32::NAN]);
        let all = vec![true; 64 * 48];
        assert!(fill(&img, 0, 0, 64, 48, &all, 40.0, (0.0, 0.0), 1).is_none());
        for (bx, by, r, hint) in [(0, 0, 6.0, (f32::NAN, 1e30)), (52, 36, 6.0, (-1e9, 0.0)), (20, 20, 0.5, (2.0, 2.0)), (0, 30, 9.0, (64.0, -48.0))] {
            let mask = disc(12, 12, (6.0, 6.0), 6.0);
            let out = fill(&img, bx, by, 12, 12, &mask, r, hint, 9);
            if let Some(o) = out {
                assert_eq!(o.len(), 144);
            }
        }
        assert!(fill(&img, 60, 0, 12, 12, &[true; 144], 6.0, (0.0, 0.0), 0).is_none(), "box off the image");
        assert!(fill(&img, 0, 0, 12, 12, &[true; 3], 6.0, (0.0, 0.0), 0).is_none(), "mask size");
        assert!(fill(&img, 0, 0, 12, 12, &[true; 144], f32::INFINITY, (0.0, 0.0), 0).is_none());
        let _ = fill(&img, 20, 20, 12, 12, &[true; 144], 1e30, (0.0, 0.0), 0);
        let tiny = Rgb32f::filled(3, 3, [0.5; 3]);
        let _ = fill(&tiny, 0, 0, 3, 3, &[false, false, false, false, true, false, false, false, false], 1.0, (1.0, 1.0), 0);
    }
}
