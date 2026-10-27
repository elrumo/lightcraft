use super::colour::{PROFILE_PROXY, PROXY, file_local_look};
use super::hue_sat::fit_hue_sat;
use super::look_fit::{FIT_PREVIEW_MAX_PIXELS, SENSOR_CLIP, clip_mask, correct_vignetting, without_clipped};
use super::pairs::{EDGE_CONTRAST, collect_pairs, fit_matrix, local_contrast};
use super::tone::{median};
use super::*;

/// Long edge of the proxies the camera's vignetting correction is estimated on ([`jpeg_vignetting`]).
const VIGNETTE_PROXY: usize = 96;
/// The estimated correction must brighten the corners by at least this gain (0.07 EV): less is a camera that
/// corrected nothing (Peripheral Illumination Correction off), not worth a lens correction of its own.
const MIN_CORNER_GAIN: f64 = 1.05;
/// ...and by at most this (2 EV): more is a JPEG brightened by something other than a lens correction.
const MAX_CORNER_GAIN: f64 = 4.0;
/// ...and cut the held-out pixels' (outlier-limited) squared error below this share of the error without it.
const MAX_VIGNETTE_HOLDOUT: f64 = 0.9;

/// One proxy pixel the vignetting is estimated on: log2 luminance of the sensor and of the camera JPEG, squared
/// distance from the centre over the half diagonal, and whether it is held out of the fit.
#[derive(Clone, Copy)]
struct RadialSample {
    sensor: f64,
    jpeg: f64,
    r2: f64,
    held_out: bool,
}

/// The lens vignetting correction the camera applied to its JPEG, for a raw whose file carries no lens data of its
/// own (a Canon CR3 with an RF lens: the camera JPEG has Peripheral Illumination Correction applied, and there is no
/// lens database to look the lens up in): [`vignetting_from_proxies`] on the sensor and camera JPEG proxies. Only
/// for CR3s, the format it was measured on (19 EOS R6 photos against their camera JPEGs and Lightroom,
/// `docs/lens-profiles.md`); the caller asks only for a raw without lens data of its own.
pub(crate) fn jpeg_vignetting(raw: &RawImage, bytes: &[u8], transform: &CameraTransform) -> Option<lightcraft_develop::EmbeddedLens> {
    // ponytail: CR3 only; other formats' cameras correct too (Sony shading, Nikon vignette control) once measured.
    // A CR3 without measured matrices builds these proxies twice per decode (`fit_preview` too): pass the estimate
    // out of `starting_colour` if decode time matters.
    if raw.format != RawFormat::Cr3 {
        return None;
    }
    let t0 = web_time::Instant::now();
    let (sensor, reference, clipped) = proxies(raw, bytes, transform, VIGNETTE_PROXY, 0.0, None)?;
    let lens = vignetting_from_proxies(&sensor, &reference, &clipped);
    if lightcraft_pipeline::profiling() {
        eprintln!("[profile] JPEG vignetting {}: {:.1} ms", if lens.is_some() { "kept" } else { "none" }, t0.elapsed().as_secs_f64() * 1e3);
    }
    lens
}

/// The radial gain that takes the sensor proxy to the camera JPEG's: the JPEG is the sensor times a radial gain put
/// through the camera's tone curve, so the gain `1 + k0 ρ² + k1 ρ⁴ + k2 ρ⁸` (ρ: distance from the centre over the
/// half diagonal; all terms ≥ 0, so it rises from 1 at the centre) is fitted alongside a monotone tone curve
/// ([`fit_radial_gain`]) on the proxy pixels away from clipping and edges. Returned as the same
/// [`lightcraft_develop::EmbeddedLens`] vignette a file's own correction is, in the oriented, default-cropped frame
/// (centred, so the orientation doesn't matter). `None` with too few usable pixels near the corners, or a gain that
/// brightens the corners by less than [`MIN_CORNER_GAIN`] or more than [`MAX_CORNER_GAIN`], or doesn't explain
/// held-out pixels clearly better than no gain ([`MAX_VIGNETTE_HOLDOUT`]).
pub(super) fn vignetting_from_proxies(sensor: &Rgb32f, reference: &Rgb32f, clipped: &[bool]) -> Option<lightcraft_develop::EmbeddedLens> {
    let (w, h) = (sensor.width, sensor.height);
    if (reference.width, reference.height) != (w, h) || w < 16 || h < 16 || clipped.len() != w * h {
        return None;
    }
    let half_diagonal2 = (w * w + h * h) as f64 / 4.0;
    let samples: Vec<RadialSample> = (0..w * h)
        .filter_map(|i| {
            let (s, j) = (*sensor.data.get(i)?, *reference.data.get(i)?);
            let (ls, lj) = (luminance_2020(s), luminance_2020(j));
            let usable = s.iter().all(|v| v.is_finite() && *v > 0.0)
                && ls > 0.002
                && (0.01..0.9).contains(&lj)
                && !clipped.get(i).copied().unwrap_or(true)
                && local_contrast(sensor, i) <= EDGE_CONTRAST
                && local_contrast(reference, i) <= EDGE_CONTRAST;
            let (x, y) = (i % w, i / w);
            let (dx, dy) = (x as f64 + 0.5 - w as f64 / 2.0, y as f64 + 0.5 - h as f64 / 2.0);
            usable.then(|| RadialSample {
                sensor: f64::from(ls).log2(),
                jpeg: f64::from(lj).log2(),
                r2: (dx * dx + dy * dy) / half_diagonal2,
                held_out: (x * 17 + y * 11) % 3 == 0,
            })
        })
        .collect();
    let train: Vec<RadialSample> = samples.iter().filter(|s| !s.held_out).copied().collect();
    let held: Vec<RadialSample> = samples.iter().filter(|s| s.held_out).copied().collect();
    if train.len() < 400 || train.iter().filter(|s| s.r2 > 0.5).count() < 64 || held.len() < 150 {
        return None;
    }
    let (k, knots) = fit_radial_gain(&train)?;
    let corner = radial_gain(&k, 1.0);
    let flat = monotone_knots(train.iter().map(|s| (s.sensor, s.jpeg)).collect())?;
    let (with, without) = (radial_error(&held, &k, &knots), radial_error(&held, &[0.0; 3], &flat));
    if lightcraft_pipeline::profiling() {
        eprintln!(
            "[profile] JPEG vignetting: k {k:?}, corner gain {corner:.3}, held-out error {with:.5} vs {without:.5} uncorrected, {} pixels",
            samples.len()
        );
    }
    if !((MIN_CORNER_GAIN..=MAX_CORNER_GAIN).contains(&corner) && with < MAX_VIGNETTE_HOLDOUT * without) {
        return None;
    }
    let vignette = lightcraft_develop::EmbeddedVignette {
        k: [k[0], k[1], 0.0, k[2], 0.0],
        center: lightcraft_geom::Point::new(0.5, 0.5),
        radius: half_diagonal2.sqrt() / w.max(h) as f64,
    };
    Some(lightcraft_develop::EmbeddedLens { vignette: Some(vignette), origin: lightcraft_develop::LensOrigin::Measured, ..Default::default() })
}

/// A CR3's sensor proxy with the vignetting correction its camera applied to the JPEG proxy it is paired with
/// ([`vignetting_from_proxies`]; the proxies are oriented alike): a look fitted to the uncorrected sensor learns some of
/// that correction as brightness, which the render's lens correction ([`jpeg_vignetting`]) would then add again.
pub(super) fn with_jpeg_vignetting(raw: &RawImage, mut sensor: Rgb32f, reference: &Rgb32f, clipped: &[bool]) -> Rgb32f {
    if raw.format == RawFormat::Cr3 {
        let lens = vignetting_from_proxies(&sensor, reference, clipped);
        correct_vignetting(&mut sensor, lens.as_ref(), lightcraft_geom::Orientation::Normal);
    }
    sensor
}

/// The radial gain `1 + k0 ρ² + k1 ρ⁴ + k2 ρ⁸` (all ≥ 0) at `r2` = ρ².
fn radial_gain(k: &[f64; 3], r2: f64) -> f64 {
    1.0 + r2 * (k[0] + r2 * (k[1] + r2 * r2 * k[2]))
}

/// The radial gain ([`radial_gain`]) and the monotone tone curve (log2 → log2) that together best map the sensor
/// samples onto the JPEG's: Gauss-Newton steps on the gain ([`nonnegative_step`]), the curve refitted to the gained
/// sensor before each ([`monotone_knots`]), samples beyond three robust deviations left out of the step. On the 19 EOS
/// R6 photos `1 + k0 ρ² + k1 ρ⁴` alone fell 0.1 EV short of the camera's gain in the outer corners (ρ² > 0.8), and
/// unconstrained it dipped to 0.92 just off the centre; the ρ⁸ term and non-negative terms fix both.
fn fit_radial_gain(samples: &[RadialSample]) -> Option<([f64; 3], Vec<(f64, f64)>)> {
    let mut k = [0.0f64; 3];
    let gained = |k: &[f64; 3], s: &RadialSample| s.sensor + radial_gain(k, s.r2).max(1e-3).log2();
    for _ in 0..24 {
        let knots = monotone_knots(samples.iter().map(|s| (gained(&k, s), s.jpeg)).collect())?;
        let residuals: Vec<f64> = samples.iter().map(|s| s.jpeg - knot_eval(&knots, gained(&k, s)).0).collect();
        let cut = 3.0 * 1.4826 * median(&mut residuals.iter().map(|e| e.abs()).collect::<Vec<_>>())?.max(0.01);
        let (mut a, mut b) = ([[0.0f64; 3]; 3], [0.0f64; 3]);
        for (s, e) in samples.iter().zip(&residuals) {
            if e.abs() > cut {
                continue;
            }
            let slope = knot_eval(&knots, gained(&k, s)).1 / (radial_gain(&k, s.r2).max(1e-3) * std::f64::consts::LN_2);
            let j = [slope * s.r2, slope * s.r2 * s.r2, slope * s.r2.powi(4)];
            for (row, ji) in a.iter_mut().zip(j) {
                for (v, jj) in row.iter_mut().zip(j) {
                    *v += ji * jj;
                }
            }
            for (v, ji) in b.iter_mut().zip(j) {
                *v += ji * e;
            }
        }
        let step = nonnegative_step(&a, &b, &k);
        let next: [f64; 3] = std::array::from_fn(|i| (k[i] + step[i]).max(0.0));
        if !next.iter().all(|v| v.is_finite() && *v < 16.0) {
            return None;
        }
        let moved = next.iter().zip(&k).map(|(n, o)| (n - o).abs()).fold(0.0, f64::max);
        k = next;
        if moved < 1e-4 {
            break;
        }
    }
    let knots = monotone_knots(samples.iter().map(|s| (gained(&k, s), s.jpeg)).collect())?;
    Some((k, knots))
}

/// The Gauss-Newton step `d` of normal equations `A d = b` that keeps `k + d ≥ 0`: the least `dᵀAd/2 − bᵀd` over
/// every choice of which terms move freely (the others go to 0). Leaving all at 0 is always allowed.
fn nonnegative_step(a: &[[f64; 3]; 3], b: &[f64; 3], k: &[f64; 3]) -> [f64; 3] {
    let mut best = (0.0, k.map(|v| -v));
    best.0 = quadratic(a, b, &best.1);
    for free in 1u8..8 {
        let is_free = |i: usize| free & (1 << i) != 0;
        let mut d = k.map(|v| -v);
        let idx: Vec<usize> = (0..3).filter(|&i| is_free(i)).collect();
        // the free terms' rows, with the fixed terms' moves taken to the right-hand side
        let mut m: Vec<Vec<f64>> = idx
            .iter()
            .map(|&i| {
                let rhs = b[i] - (0..3).filter(|&j| !is_free(j)).map(|j| a[i][j] * d[j]).sum::<f64>();
                idx.iter().map(|&j| a[i][j]).chain([rhs]).collect()
            })
            .collect();
        let Some(x) = solve_small(&mut m) else { continue };
        for (&i, v) in idx.iter().zip(x) {
            d[i] = v;
        }
        if d.iter().zip(k).all(|(d, k)| k + d >= -1e-12) {
            let q = quadratic(a, b, &d);
            if q < best.0 {
                best = (q, d);
            }
        }
    }
    best.1
}

/// `dᵀAd/2 − bᵀd`.
fn quadratic(a: &[[f64; 3]; 3], b: &[f64; 3], d: &[f64; 3]) -> f64 {
    let ad: [f64; 3] = std::array::from_fn(|i| (0..3).map(|j| a[i][j] * d[j]).sum());
    (0..3).map(|i| d[i] * (ad[i] / 2.0 - b[i])).sum()
}

/// The solution of a small linear system given as rows `[a…, rhs]`, by elimination with partial pivoting; `None` when
/// singular.
fn solve_small(m: &mut [Vec<f64>]) -> Option<Vec<f64>> {
    let n = m.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&p, &q| m[p][col].abs().total_cmp(&m[q][col].abs()))?;
        let p = m[pivot][col];
        if p.is_nan() || p.abs() <= 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        let (top, rest) = m.split_at_mut(col + 1);
        let pivot_row = top.last()?;
        for target in rest {
            let f = target[col] / pivot_row[col];
            for (t, v) in target.iter_mut().zip(pivot_row).skip(col) {
                *t -= f * v;
            }
        }
    }
    let mut x = vec![0.0; n];
    for (row, r) in m.iter().enumerate().rev() {
        let known: f64 = r.iter().zip(&x).take(n).skip(row + 1).map(|(a, x)| a * x).sum();
        x[row] = (r[n] - known) / r[row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Mean squared error of `samples` under gain `k` and tone curve `knots`, each sample's error limited to 0.5 (EV²)
/// so a few mismatched pixels don't decide.
fn radial_error(samples: &[RadialSample], k: &[f64; 3], knots: &[(f64, f64)]) -> f64 {
    let sum: f64 = samples
        .iter()
        .map(|s| {
            let x = s.sensor + radial_gain(k, s.r2).max(1e-3).log2();
            (s.jpeg - knot_eval(knots, x).0).powi(2).min(0.5)
        })
        .sum();
    sum / samples.len().max(1) as f64
}

/// A monotone curve through `(x, y)` points: the medians of 24 equal-count bins along `x`, each `y` at least the one
/// before. `None` with fewer than 8 points a bin or fewer than 4 distinct knots.
fn monotone_knots(mut points: Vec<(f64, f64)>) -> Option<Vec<(f64, f64)>> {
    const BINS: usize = 24;
    points.retain(|(x, y)| x.is_finite() && y.is_finite());
    if points.len() < BINS * 8 {
        return None;
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    let n = points.len();
    let mut knots: Vec<(f64, f64)> = Vec::with_capacity(BINS);
    for bin in 0..BINS {
        let chunk = points.get(bin * n / BINS..(bin + 1) * n / BINS)?;
        let x = median(&mut chunk.iter().map(|p| p.0).collect::<Vec<_>>())?;
        let y = median(&mut chunk.iter().map(|p| p.1).collect::<Vec<_>>())?;
        match knots.last() {
            Some(&(px, _)) if x <= px => {}
            Some(&(_, py)) => knots.push((x, y.max(py))),
            None => knots.push((x, y)),
        }
    }
    (knots.len() >= 4).then_some(knots)
}

/// The curve through `knots` (increasing `x`) at `x`, and its slope there: linear between knots, the end segments
/// continued beyond them.
fn knot_eval(knots: &[(f64, f64)], x: f64) -> (f64, f64) {
    let i = knots.partition_point(|k| k.0 < x).max(1).min(knots.len().saturating_sub(1));
    let (Some(&(x0, y0)), Some(&(x1, y1))) = (knots.get(i.wrapping_sub(1)), knots.get(i)) else { return (x, 1.0) };
    let slope = if x1 > x0 { (y1 - y0) / (x1 - x0) } else { 0.0 };
    (y0 + slope * (x - x0), slope)
}

/// The embedded preview's long edge must be at least this share of the raw's (and 384 px): a DNG whose preview is a
/// small thumbnail is not fitted. The fit itself runs on a proxy of at most 384 px, so the 1024 px preview
/// Lightroom writes into its DNGs (15% of a 33 MP raw's long edge) is plenty.
pub(super) const MIN_PREVIEW_FRACTION: f64 = 0.1;

/// Lowest preview luminance and channel value a tone-only fit trains on (the other fits use 0.015 and 0.004): a dark
/// night scene has most of its pixels below 0.015, and a curve fitted without them overshoots its shadows.
pub(super) const DARK_FLOOR: (f32, f32) = (0.004, 0.001);

/// Share of the proxy by which the fitted curve may clip more highlights than the plain render and the camera's
/// preview (chosen on the CC0 and user DNGs from the clipped share of their 1440 px renders: it refuses the fits
/// that clip about 1% more than the camera does and passes the ones that clip like the camera).
pub(super) const CLIP_TOLERANCE: f64 = 0.001;

/// Log-luminance correlation (5 × 5 box blurred) the unshifted proxies must reach.
const MIN_STRUCTURE: f64 = 0.85;

/// Whether the sensor proxy and the camera JPEG proxy show the same picture in the same place: their log luminance
/// correlates by at least [`MIN_STRUCTURE`] unshifted, and no shift of more than one proxy pixel (within ±3)
/// correlates better.
pub(super) fn structure_agrees(sensor: &Rgb32f, reference: &Rgb32f) -> bool {
    let (w, h) = (sensor.width, sensor.height);
    if (w, h) != (reference.width, reference.height) || w < 16 || h < 16 || sensor.data.len() != w * h || reference.data.len() != w * h {
        return false;
    }
    let log = |img: &Rgb32f| -> Vec<f64> {
        let l: Vec<f64> =
            img.data.iter().map(|p| f64::from(luminance_2020(p.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 }))).max(1e-4).ln()).collect();
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let (mut sum, mut n) = (0.0, 0.0);
                for yy in y.saturating_sub(2)..(y + 3).min(h) {
                    for xx in x.saturating_sub(2)..(x + 3).min(w) {
                        if let Some(v) = l.get(yy * w + xx) {
                            sum += v;
                            n += 1.0;
                        }
                    }
                }
                sum / f64::max(n, 1.0)
            })
            .collect()
    };
    let (a, b) = (log(sensor), log(reference));
    const M: usize = 3;
    let corr = |dx: isize, dy: isize| -> f64 {
        let (mut n, mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for y in M..h - M {
            for x in M..w - M {
                let (xx, yy) = (x.saturating_add_signed(dx), y.saturating_add_signed(dy));
                let (Some(&u), Some(&v)) = (a.get(y * w + x), b.get(yy * w + xx)) else { continue };
                n += 1.0;
                sa += u;
                sb += v;
                saa += u * u;
                sbb += v * v;
                sab += u * v;
            }
        }
        let var = (saa - sa * sa / n) * (sbb - sb * sb / n);
        if n < 2.0 || var <= 1e-12 { 0.0 } else { (sab - sa * sb / n) / var.sqrt() }
    };
    let at_zero = corr(0, 0);
    let (mut best, mut shift) = (at_zero, (0isize, 0isize));
    for dy in -(M as isize)..=M as isize {
        for dx in -(M as isize)..=M as isize {
            let c = corr(dx, dy);
            if c > best + 1e-9 {
                (best, shift) = (c, (dx, dy));
            }
        }
    }
    at_zero >= MIN_STRUCTURE && shift.0.abs() <= 1 && shift.1.abs() <= 1
}

/// Same-size proxies of the sensor (white-balanced, baseline exposure, through `transform`'s
/// matrix: the generic camera ≈ sRGB model) and of the file's embedded camera JPEG (linear Rec.2020),
/// and per proxy pixel whether any sensor sample it covers is clipped.
pub(super) fn proxies(
    raw: &RawImage,
    bytes: &[u8],
    transform: &CameraTransform,
    size: usize,
    min_fraction: f64,
    lens: Option<&LensGeometry>,
) -> Option<(Rgb32f, Rgb32f, Vec<bool>)> {
    let edge = (2 * size).max(384) as u32;
    let base = proxy_base(raw, bytes, edge)?;
    let crop = raw.crop.clipped(raw.active_area.width, raw.active_area.height);
    // a preview below `min_fraction` of the crop's long edge (or under 384 px) is refused; 0 = any
    if min_fraction > 0.0 && (base.source_long as f64) < (min_fraction * crop.width.max(crop.height) as f64).max(384.0) {
        return None;
    }
    proxies_with(raw, base.reference.clone(), base.sensor.as_ref().map(|(k, s, c)| (*k, s.clone(), c.clone())), transform, size, lens)
}

/// What [`proxies`] pairs before any colour transform: the camera JPEG decoded to fit `edge` (working space), its
/// long edge as stored, and the sensor's binned proxy at `edge` with its clip mask (and the binning factor).
pub(super) struct ProxyBase {
    key: (usize, usize, u32, usize, usize),
    reference: Rgb32f,
    source_long: usize,
    sensor: Option<(usize, Rgb32f, Rgb32f)>,
}

thread_local! {
    /// The [`ProxyBase`] of the decode in a [`sharing_proxies`] scope on this thread.
    pub(super) static PROXY_BASE: std::cell::RefCell<Option<std::rc::Rc<ProxyBase>>> = const { std::cell::RefCell::new(None) };
    static SHARING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run one decode's colour fits (`f`: the starting colour, the JPEG vignetting) with the part of [`proxies`] that no
/// colour transform touches built once among them: they decode the same camera JPEG and bin the same sensor, each
/// then through its own transform, and building it twice was a quarter of a CR3's preview decode (~250 ms of 1.1 s on
/// an M1 Max). The fits come out as they did; outside a scope nothing is kept.
pub(crate) fn sharing_proxies<T>(f: impl FnOnce() -> T) -> T {
    struct End(bool);
    impl Drop for End {
        fn drop(&mut self) {
            SHARING.with(|s| s.set(self.0));
            if !self.0 {
                PROXY_BASE.with(|b| b.borrow_mut().take());
            }
        }
    }
    let _end = End(SHARING.with(|s| s.replace(true)));
    f()
}

/// What a raw's colour fits gave at its decode ([`decode_fits`]).
pub(super) struct Fits {
    xy: Xy,
    t: CameraTransform,
    look: Option<CameraLook>,
    matched: Option<CameraMatch>,
    lens: Option<lightcraft_develop::EmbeddedLens>,
    /// `raw.color` after [`starting_colour`] filled in the model's measured matrices (`None`: it did not).
    color: Option<ColorData>,
}

/// Raws' [`Fits`] by file ([`fingerprint`]) and decoded size (a linear raw binned while it is decoded is smaller and
/// fits on other samples), at most this many.
const FITS_KEPT: usize = 64;

/// Keyed by the file, the decoded size and the profile database generation (its camera and lens profiles feed the fits).
type FitsCache = lightcraft_preview::Lru<(lightcraft_preview::Hash128, usize, usize, u64), std::sync::Arc<Fits>>;

pub(super) fn fits_cache() -> &'static std::sync::Mutex<FitsCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<FitsCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(lightcraft_preview::Lru::new(FITS_KEPT)))
}

/// Forget the kept [`Fits`] (`Session::release_memory`).
pub(crate) fn forget_fits() {
    fits_cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}

/// A file told apart from others cheaply: its length, first 64 KiB (the tags) and 64 blocks of 4 KiB spread evenly
/// to its end (0.1 ms for a 23 MB CR3 on an M1 Max, where hashing it all takes 8 ms).
pub(super) fn fingerprint(bytes: &[u8]) -> lightcraft_preview::Hash128 {
    const BLOCK: usize = 4096;
    const BLOCKS: usize = 64;
    let mut h = lightcraft_preview::Hasher128::new();
    h.u64(bytes.len() as u64).update(bytes.get(..65536).unwrap_or(bytes));
    let span = bytes.len().saturating_sub(BLOCK) as u128;
    for i in 0..=BLOCKS as u128 {
        let at = usize::try_from(span * i / BLOCKS as u128).unwrap_or(0);
        h.update(bytes.get(at..at.saturating_add(BLOCK).min(bytes.len())).unwrap_or_default());
    }
    h.finish()
}

/// A raw decode's colour fits: [`starting_colour`] (which may fill the model's measured matrices into `raw.color`)
/// and, for a raw without lens data of its own (`lens`, the file's or the lens database's), [`jpeg_vignetting`], with
/// their proxies built once ([`sharing_proxies`]). Kept per file: a photo is decoded again at each level (thumbnail,
/// preview, full size for zoom and export, and after its decoded source is evicted), and these fits were ~45 % of a
/// CR3's preview decode. They depend only on the file, the decoded raw and the camera and lens profiles (read once per
/// process, and again when the profile database changes, which the key follows), so a kept result is what fitting
/// again gives. Returns the as-shot white, the camera transform, the
/// look, the Camera Matching profile and the lens correction.
pub(crate) fn decode_fits(
    raw: &mut RawImage,
    bytes: &[u8],
    lens_geometry: Option<&LensGeometry>,
    lens: Option<lightcraft_develop::EmbeddedLens>,
) -> (Xy, CameraTransform, Option<CameraLook>, Option<CameraMatch>, Option<lightcraft_develop::EmbeddedLens>) {
    let key = (fingerprint(bytes), raw.width, raw.height, crate::profile_db::generation());
    let kept = fits_cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key).cloned();
    let fits = match kept {
        Some(fits) => {
            if let Some(color) = &fits.color {
                raw.color = color.clone();
            }
            fits
        }
        None => {
            let had_matrix = lightcraft_raw::color::has_matrix(&raw.color);
            let fits = sharing_proxies(|| {
                let (xy, t, look, matched) = starting_colour(raw, bytes, lens_geometry, lens.as_ref());
                let lens = lens.or_else(|| jpeg_vignetting(raw, bytes, &t));
                let filled = !had_matrix && lightcraft_raw::color::has_matrix(&raw.color);
                Fits { xy, t, look, matched, lens, color: filled.then(|| raw.color.clone()) }
            });
            let fits = std::sync::Arc::new(fits);
            fits_cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, fits.clone(), 1);
            fits
        }
    };
    (fits.xy, fits.t, fits.look.clone(), fits.matched.clone(), fits.lens)
}

/// [`proxies`]' transform-free part for `raw` and its file `bytes` (see [`ProxyBase`]); shared within a
/// [`sharing_proxies`] scope.
fn proxy_base(raw: &RawImage, bytes: &[u8], edge: u32) -> Option<std::rc::Rc<ProxyBase>> {
    // (one decode's raw and bytes: the scope ends with the decode, so an address can't stand for another file)
    let key = (bytes.as_ptr() as usize, bytes.len(), edge, raw.width, raw.height);
    let sharing = SHARING.with(std::cell::Cell::get);
    if sharing && let Some(b) = PROXY_BASE.with(|b| b.borrow().clone()).filter(|b| b.key == key) {
        return Some(b);
    }
    let decoded = crate::files::decode_raw_preview(
        bytes,
        lightcraft_codecs::DecodeOptions { max_size: Some((edge, edge)), max_pixels: FIT_PREVIEW_MAX_PIXELS },
        None,
    );
    if decoded.is_none() && lightcraft_pipeline::profiling() {
        eprintln!("[profile] camera look: the embedded preview does not decode");
    }
    let decoded = decoded?;
    let source_long = decoded.source_width.max(decoded.source_height) as usize;
    let crop = raw.crop.clipped(raw.active_area.width, raw.active_area.height);
    let k = proxy_bin(&crop, edge);
    let sensor = sensor_proxy(raw, k, edge as usize).map(|s| {
        let clipped = clip_mask(&s);
        (k, s, clipped)
    });
    let base = std::rc::Rc::new(ProxyBase { key, reference: decoded.to_working(), source_long, sensor });
    if sharing {
        PROXY_BASE.with(|b| *b.borrow_mut() = Some(base.clone()));
    }
    Some(base)
}

/// The binning factor of the sensor proxy for a crop of `crop` at `edge` px: fixed and bounded, so the selected
/// look cannot depend on thumbnail/export resolution.
fn proxy_bin(crop: &lightcraft_raw::Rect, edge: u32) -> usize {
    (crop.width.max(crop.height).div_ceil(edge as usize).max(2)).div_ceil(2) * 2
}

/// [`proxies`] against `reference`, the camera JPEG or another render of the photo in the sensor's orientation (linear
/// Rec.2020).
pub(super) fn proxies_from(
    raw: &RawImage,
    reference: Rgb32f,
    transform: &CameraTransform,
    size: usize,
    lens: Option<&LensGeometry>,
) -> Option<(Rgb32f, Rgb32f, Vec<bool>)> {
    proxies_with(raw, reference, None, transform, size, lens)
}

/// [`proxies_from`], with the sensor proxy, its clip mask and binning factor already made when `sensor` has them.
fn proxies_with(
    raw: &RawImage,
    mut reference: Rgb32f,
    sensor: Option<(usize, Rgb32f, Rgb32f)>,
    transform: &CameraTransform,
    size: usize,
    lens: Option<&LensGeometry>,
) -> Option<(Rgb32f, Rgb32f, Vec<bool>)> {
    let edge = (2 * size).max(384) as u32;
    let (a, crop) = (raw.active_area, raw.crop.clipped(raw.active_area.width, raw.active_area.height));
    if crop.width == 0 || crop.height == 0 || reference.width == 0 || reference.height == 0 {
        return None;
    }
    let matches = |w: usize, h: usize, rw: usize, rh: usize| (rw as f64 / rh as f64 / (w as f64 / h as f64) - 1.0).abs() <= 0.02;
    if !matches(crop.width, crop.height, reference.width, reference.height) {
        // Panasonic previews show the whole active area while the default crop is the in-camera aspect ratio
        if !matches(a.width, a.height, reference.width, reference.height) {
            if lightcraft_pipeline::profiling() {
                eprintln!(
                    "[profile] camera look: preview {}×{} and sensor crop {}×{} (active area {}×{}) differ in shape",
                    reference.width, reference.height, crop.width, crop.height, a.width, a.height
                );
            }
            return None;
        }
        let (sx, sy) = (reference.width as f64 / a.width as f64, reference.height as f64 / a.height as f64);
        let (x, y) = ((crop.x as f64 * sx).round() as usize, (crop.y as f64 * sy).round() as usize);
        let (w, h) = ((crop.width as f64 * sx).round() as usize, (crop.height as f64 * sy).round() as usize);
        if w < 16 || h < 16 || x + w > reference.width || y + h > reference.height {
            return None;
        }
        reference = reference.into_crop(x, y, w, h);
    }
    // Fixed, bounded proxy: the selected look cannot depend on thumbnail/export resolution.
    let (k, mut sensor, mut clipped) = match sensor {
        Some(made) => made,
        None => {
            let k = proxy_bin(&crop, edge);
            let Some(s) = sensor_proxy(raw, k, edge as usize) else {
                if lightcraft_pipeline::profiling() {
                    eprintln!("[profile] camera look: no binned sensor proxy");
                }
                return None;
            };
            let c = clip_mask(&s);
            (k, s, c)
        }
    };
    // The camera may have corrected its JPEG with the lens warp the file carries (Nikon Z lenses always, at up to 9 %
    // zoom; other cameras when their correction setting is on): the sensor moved into that geometry is a candidate,
    // kept below when it lines up with the JPEG better than the uncorrected one.
    let warped = lens
        .filter(|_| raw.format != RawFormat::Cr3)
        .and_then(|l| Some((into_lens_geometry(&sensor, raw, k, l)?, into_lens_geometry(&clipped, raw, k, l)?)));
    let gain = 2f32.powf(transform.baseline_exposure as f32);
    let to_working = |p: [f32; 3]| transform.matrix.apply_f32(std::array::from_fn(|i| p[i] * transform.wb[i] * gain));
    if raw.format == RawFormat::Cr3 {
        // Orient the camera JPEG and the sensor identically before collecting pixel correspondences.
        sensor = sensor.into_oriented(raw.orientation);
        clipped = clipped.into_oriented(raw.orientation);
        reference = reference.into_oriented(raw.orientation);
        sensor.map_in_place(to_working);
        // Canon's JPEG can be cropped differently from the sensor. Estimate only that common
        // framing from edge directions, independently of the subsequent colour fit.
        (sensor, clipped) = align_cr3_framing(sensor, clipped, &reference);
    }
    let mut sensor = fit(&sensor, size, size, Filter::Box);
    let mut reference_small = reference_at(&reference, &sensor);
    if let Some((warped, warped_clipped)) = warped {
        let candidate = fit(&warped, size, size, Filter::Box);
        let candidate_reference = reference_at(&reference, &candidate);
        if alignment(&candidate, &candidate_reference) > alignment(&sensor, &reference_small) {
            (sensor, reference_small, clipped) = (candidate, candidate_reference, warped_clipped);
        }
    }
    let reference = reference_small;
    // A proxy pixel is clipped when any sample it averages is (outside the CR3 framing the mask is
    // NaN: those pixels are left out of the pairs anyway). Resized to exactly the proxy's size, like
    // the reference, so the mask pairs pixel for pixel.
    let clipped: Vec<bool> = resize(&clipped, sensor.width, sensor.height, Filter::Box).data.iter().map(|p| p[0] > 0.0).collect();
    if raw.format != RawFormat::Cr3 {
        sensor.map_in_place(to_working);
    }
    Some((sensor, reference, clipped))
}

/// The camera JPEG at exactly the sensor proxy's size, so the two pair pixel for pixel. Fitting it
/// into the proxy's box instead keeps its own aspect, which can round a pixel short: an ILCE-7RM4's
/// 1616×1080 preview, decoded at 384×257, fits a 192×128 proxy as 191×128, and `collect_pairs`
/// then rejected every file `lightcraft-cli calibrate` was given. `proxies` has already checked
/// that the aspects agree within 2 %.
pub(super) fn reference_at(reference: &Rgb32f, sensor: &Rgb32f) -> Rgb32f {
    resize(reference, sensor.width, sensor.height, Filter::Box)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Cr3Framing {
    pub(super) scale: f32,
    /// Translation as a fraction of the oriented frame's width and height.
    pub(super) offset: [f32; 2],
}

impl Cr3Framing {
    const IDENTITY: Self = Self { scale: 1.0, offset: [0.0; 2] };

    pub(super) fn source(self, x: f32, y: f32, w: usize, h: usize) -> (f32, f32) {
        let (w, h) = (w as f32, h as f32);
        ((x - w * 0.5) * self.scale + w * (0.5 + self.offset[0]), (y - h * 0.5) * self.scale + h * (0.5 + self.offset[1]))
    }
}

/// Register the JPEG's remaining framing on a fixed proxy. Edge directions survive different
/// white balance and monotone camera tone; no target RGB values train this three-parameter fit.
/// A separate third of the edges must confirm a substantial improvement and a close match.
/// A weak, flat or unrelated preview leaves the sensor framing unchanged.
pub(super) fn estimate_cr3_framing(sensor: &Rgb32f, reference: &Rgb32f) -> Option<Cr3Framing> {
    if sensor.width.checked_mul(sensor.height)? != sensor.data.len() || reference.width.checked_mul(reference.height)? != reference.data.len() {
        return None;
    }
    let sensor = fit(sensor, PROXY, PROXY, Filter::Box);
    let (w, h) = (sensor.width, sensor.height);
    if w < 16 || h < 16 {
        return None;
    }
    let reference = fit(reference, w, h, Filter::Box);
    if (reference.width, reference.height) != (w, h) {
        return None;
    }
    let mut gradients = Rgb32f::new(w, h);
    let mut edges = Vec::new();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let derivative = |image: &Rgb32f| {
                [
                    luminance_2020(image.data[y * w + x + 1]) - luminance_2020(image.data[y * w + x - 1]),
                    luminance_2020(image.data[(y + 1) * w + x]) - luminance_2020(image.data[(y - 1) * w + x]),
                ]
            };
            let d = derivative(&sensor);
            gradients.data[y * w + x] = [d[0], d[1], 0.0];
            let d = derivative(&reference);
            let magnitude = d[0].hypot(d[1]);
            let value = luminance_2020(reference.data[y * w + x]);
            if (0.02..0.9).contains(&value) && magnitude.is_finite() && magnitude > 0.03 {
                edges.push((x as f32 + 0.5, y as f32 + 0.5, d, magnitude, (x * 17 + y * 11) % 3 == 0));
            }
        }
    }
    if edges.len() < 192 {
        return None;
    }
    let score = |framing: Cr3Framing, held_out: bool| -> Option<f64> {
        let (mut agreement, mut weight, mut valid, mut total) = (0.0, 0.0, 0usize, 0usize);
        for &(x, y, target, magnitude, holdout) in &edges {
            if holdout != held_out {
                continue;
            }
            total += 1;
            let (sx, sy) = framing.source(x, y, w, h);
            if sx < 1.5 || sy < 1.5 || sx > w as f32 - 1.5 || sy > h as f32 - 1.5 {
                continue;
            }
            let sample = gradients.sample_bilinear(sx, sy);
            let length = sample[0].hypot(sample[1]);
            if !length.is_finite() || length <= 0.01 {
                continue;
            }
            let k = f64::from(magnitude.min(0.25));
            agreement += k * f64::from((sample[0] * target[0] + sample[1] * target[1]) / (length * magnitude));
            weight += k;
            valid += 1;
        }
        (valid >= 64 && valid * 5 >= total * 4 && weight > 0.0).then_some(agreement / weight)
    };
    let before = score(Cr3Framing::IDENTITY, false)?;
    let mut best = (before, Cr3Framing::IDENTITY);
    // At most 10% zoom and two proxy pixels of translation; a coarse search followed by one
    // local refinement prevents unbounded optimisation on the photograph's contents.
    for s in -10..=10 {
        for tx in -4..=4 {
            for ty in -4..=4 {
                let framing = Cr3Framing { scale: 1.0 + s as f32 * 0.01, offset: [tx as f32 * 0.5 / w as f32, ty as f32 * 0.5 / h as f32] };
                if let Some(value) = score(framing, false)
                    && value > best.0
                {
                    best = (value, framing);
                }
            }
        }
    }
    let coarse = best.1;
    for s in -5..=5 {
        for tx in -4..=4 {
            for ty in -4..=4 {
                let framing = Cr3Framing {
                    scale: coarse.scale + s as f32 * 0.002,
                    offset: [coarse.offset[0] + tx as f32 * 0.125 / w as f32, coarse.offset[1] + ty as f32 * 0.125 / h as f32],
                };
                if !(0.9..=1.1).contains(&framing.scale) || framing.offset[0].abs() > 2.0 / w as f32 || framing.offset[1].abs() > 2.0 / h as f32 {
                    continue;
                }
                if let Some(value) = score(framing, false)
                    && value > best.0
                {
                    best = (value, framing);
                }
            }
        }
    }
    let (held_before, held_after) = (score(Cr3Framing::IDENTITY, true)?, score(best.1, true)?);
    if best.0 < 0.9 || best.0 < before + 0.04 || held_after < 0.9 || held_after < held_before + 0.04 {
        return None;
    }
    if lightcraft_pipeline::profiling() {
        eprintln!("[profile] CR3 JPEG framing {:?}, held-out edge agreement {held_before:.4} -> {held_after:.4}", best.1);
    }
    Some(best.1)
}

/// The sensor proxy and its clip mask (same size), both moved to the camera JPEG's framing.
fn align_cr3_framing(sensor: Rgb32f, clipped: Rgb32f, reference: &Rgb32f) -> (Rgb32f, Rgb32f) {
    let Some(framing) = estimate_cr3_framing(&sensor, reference) else { return (sensor, clipped) };
    let apply = |image: &Rgb32f| {
        let (w, h) = (image.width, image.height);
        let mut aligned = Rgb32f::new(w, h);
        for (i, pixel) in aligned.data.iter_mut().enumerate() {
            let (x, y) = framing.source((i % w) as f32 + 0.5, (i / w) as f32 + 0.5, w, h);
            *pixel = if x >= 0.5 && y >= 0.5 && x <= w as f32 - 0.5 && y <= h as f32 - 0.5 { image.sample_bilinear(x, y) } else { [f32::NAN; 3] };
        }
        aligned
    };
    (apply(&sensor), apply(&clipped))
}

/// Colour training pairs of one raw for a camera profile: white-balanced camera RGB (with the
/// baseline exposure) → its camera JPEG (linear Rec.2020). `None` for formats with their own
/// colour matrices, other files or unusable previews.
pub(crate) fn profile_pairs(raw: &RawImage, bytes: &[u8], lens: Option<&LensGeometry>) -> Option<Vec<([f64; 3], [f64; 3])>> {
    if !file_local_look(raw.format) || lightcraft_raw::color::has_matrix(&raw.color) {
        return None;
    }
    let transform = lightcraft_raw::color::camera_transform(raw, lightcraft_raw::color::as_shot_white_xy(raw));
    let (sensor, reference, clipped) = proxies(raw, bytes, &transform, PROFILE_PROXY, 0.0, lens)?;
    let to_camera = transform.matrix.inverse()?;
    // clipped sensor pixels don't record the scene's colour (see `fit_look`)
    let sensor = without_clipped(&sensor, &clipped).unwrap_or(sensor);
    let (pairs, _) = collect_pairs(&sensor, &reference, 0.05, None, false)?;
    Some(pairs.into_iter().map(|(x, y)| (to_camera.apply(x), y)).collect())
}

/// A camera profile's colour model (matrix from white-balanced camera RGB, hue/saturation table)
/// fitted to pairs pooled from many photos.
pub(crate) fn fit_profile(pairs: &[([f64; 3], [f64; 3])]) -> Option<(Mat3, Option<HsvTable>)> {
    let matrix = fit_matrix(pairs)?;
    Some((matrix, fit_hue_sat(pairs, &matrix)))
}

/// A raw's lens-correction warp, as its camera applied it to the embedded JPEG: the first `WarpRectilinear` of
/// `OpcodeList3` (its green plane; the in-file corrections LightCraft reads have no tangential terms). `None` without
/// one, or for a DNG 1.6 `WarpRectilinear2` chain (phone DNGs, which carry colour matrices and are not fitted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LensGeometry {
    /// `kr0..kr3`: source radius = r * (kr0 + kr1 r^2 + kr2 r^4 + kr3 r^6), r over the farthest corner distance.
    pub(super) k: [f64; 4],
    /// The optical centre, relative to the active area (as the opcode has it).
    pub(super) center: [f64; 2],
}

pub(crate) fn lens_geometry(list3: &[lightcraft_raw::Opcode]) -> Option<LensGeometry> {
    use lightcraft_raw::Opcode;
    let ops: Vec<&Opcode> = lightcraft_raw::opcodes::applied_ops(list3).collect();
    if ops.iter().any(|op| matches!(op, Opcode::WarpRectilinear2 { .. })) {
        return None;
    }
    ops.into_iter().find_map(|op| match op {
        Opcode::WarpRectilinear { planes, center } => {
            let p = planes.get(1).or(planes.first())?;
            let k = [p[0], p[1], p[2], p[3]];
            (k.iter().chain(center).all(|v| v.is_finite()) && (0.0..=1.0).contains(&center[0]) && (0.0..=1.0).contains(&center[1]))
                .then_some(LensGeometry { k, center: *center })
        }
        _ => None,
    })
}

/// How well two same-size proxies line up: the correlation of their high-passed log luminance over the pixels both
/// have (a tone curve or colour change leaves it alone, a geometric offset lowers it). `-1` without enough pixels.
pub(super) fn alignment(a: &Rgb32f, b: &Rgb32f) -> f64 {
    let (w, h) = (a.width, a.height);
    if (b.width, b.height) != (w, h) || w < 3 || h < 3 {
        return -1.0;
    }
    let lum = |img: &Rgb32f| img.data.iter().map(|p| (luminance_2020(*p).max(1e-4) as f64).ln()).collect::<Vec<f64>>();
    let high = |l: &[f64]| -> Vec<f64> {
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                    return f64::NAN;
                }
                let around = [i - w - 1, i - w, i - w + 1, i - 1, i + 1, i + w - 1, i + w, i + w + 1];
                l[i] - around.iter().map(|&j| l[j]).sum::<f64>() / 8.0
            })
            .collect()
    };
    let (ha, hb) = (high(&lum(a)), high(&lum(b)));
    let pairs: Vec<(f64, f64)> = ha.into_iter().zip(hb).filter(|(x, y)| x.is_finite() && y.is_finite()).collect();
    if pairs.len() < 64 {
        return -1.0;
    }
    let n = pairs.len() as f64;
    let (ma, mb) = (pairs.iter().map(|p| p.0).sum::<f64>() / n, pairs.iter().map(|p| p.1).sum::<f64>() / n);
    let cov: f64 = pairs.iter().map(|p| (p.0 - ma) * (p.1 - mb)).sum();
    let (va, vb): (f64, f64) = (pairs.iter().map(|p| (p.0 - ma).powi(2)).sum(), pairs.iter().map(|p| (p.1 - mb).powi(2)).sum());
    let r = cov / (va * vb).sqrt().max(1e-12);
    if r.is_finite() { r } else { -1.0 }
}

/// A `k`-binned sensor proxy of `raw` ([`sensor_proxy`]: the default crop, or the active area when the crop is
/// smaller than a block, at `1/k`) moved into the camera JPEG's geometry: each pixel samples the proxy where `lens`
/// maps it, and is NaN where that falls outside the proxy (the pairs leave it out). `None` for a proxy of another
/// size (the demosaiced fallback of linear raws).
pub(super) fn into_lens_geometry(img: &Rgb32f, raw: &RawImage, k: usize, lens: &LensGeometry) -> Option<Rgb32f> {
    let a = raw.active_area;
    let c = raw.crop.clipped(a.width, a.height);
    let c = if c.width < k || c.height < k { lightcraft_raw::Rect::new(0, 0, a.width, a.height) } else { c };
    let (w, h) = (img.width, img.height);
    if k == 0 || w != c.width / k || h != c.height / k || w == 0 || h == 0 || a.width < 2 || a.height < 2 {
        return None;
    }
    let (aw, ah) = (a.width as f64 - 1.0, a.height as f64 - 1.0);
    let (cx, cy) = (lens.center[0] * aw, lens.center[1] * ah);
    let m = [(0.0, 0.0), (aw, 0.0), (0.0, ah), (aw, ah)].iter().map(|&(x, y)| (x - cx).hypot(y - cy)).fold(0.0, f64::max).max(1e-9);
    let [k0, k1, k2, k3] = lens.k;
    let kf = k as f64;
    let (x0, y0) = (c.x as f64, c.y as f64);
    let mut out = Rgb32f::new(w, h);
    for (i, px) in out.data.iter_mut().enumerate() {
        // the proxy pixel's centre in active-area pixel-index coordinates, mapped to the sensor
        let (ox, oy) = (x0 + ((i % w) as f64 + 0.5) * kf - 0.5, y0 + ((i / w) as f64 + 0.5) * kf - 0.5);
        let (dx, dy) = ((ox - cx) / m, (oy - cy) / m);
        let r2 = dx * dx + dy * dy;
        let f = k0 + r2 * (k1 + r2 * (k2 + r2 * k3));
        let (sx, sy) = ((cx + m * dx * f + 0.5 - x0) / kf, (cy + m * dy * f + 0.5 - y0) / kf);
        *px = if sx >= 0.5 && sy >= 0.5 && sx <= w as f64 - 0.5 && sy <= h as f64 - 0.5 {
            img.sample_bilinear(sx as f32, sy as f32)
        } else {
            [f32::NAN; 3]
        };
    }
    Some(out)
}

pub(super) fn sensor_proxy(raw: &RawImage, k: usize, edge: usize) -> Option<Rgb32f> {
    // Linear (three-sample) data needs no demosaicing: developed and fitted to `edge`, not binned by `k`
    // (`develop_binned` bins it too, for previews, but a small image would then halve below `edge`).
    // Lens opcodes cannot be applied to a binned mosaic: they are defined at full resolution.
    if (raw.cpp == 3 && raw.cfa.is_none()) || (raw.format == RawFormat::Dng && !raw.opcodes.list3.is_empty()) {
        return Some(fit(&raw.develop(lightcraft_raw::Method::Bilinear).ok()?, edge, edge, Filter::Box));
    }
    raw.develop_binned(k, SENSOR_CLIP).ok()?
}
