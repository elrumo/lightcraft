//! The per-pixel stage: everything after the spatial planes are ready, in one parallel pass.

use lightcraft_color::Map3;
use lightcraft_color::Mat3;
use lightcraft_color::luminance_2020;
use lightcraft_color::spline::{Lut1, MonotoneCurve};
use lightcraft_color::transfer::{linear_to_srgb, srgb_to_linear};
use lightcraft_develop::{DevelopSettings, LocalAdjustments, Process, ToneCurve, VignetteStyle};
use lightcraft_geom::Point;
use lightcraft_raster::Rgba8;

use crate::colorops::ColorOps;
use crate::geometry::Frame;
use crate::local::log_lum;
use crate::output::{DeepImage, DeepSamples, OutputDepth, OutputSpace, OutputTrc};
use crate::tone::ToneMap;
use crate::{Prepared, SourceInfo, for_rows};

/// Capture sharpening's change of log2 luminance (process V3) for `det`, log luminance less its
/// blur ([`crate::local::Capture`], weighted): `sp` the amount (and a mask's Sharpness), `detail`
/// the Detail slider (0..1: what strong edges get of it, where halos would show; fine texture gets
/// all), `mask` the Masking slider (0..1: only edges stronger than it). The GPU kernel mirrors it.
#[inline]
pub(crate) fn capture_delta(sp: f32, det: f32, detail: f32, mask: f32) -> f32 {
    let a = det.abs();
    sp * 1.3 * det.clamp(-0.8, 0.8) * sharpen_mask_weight(mask, a) * capture_keep(detail, a)
}

/// What a strong edge (`a`, the size of its detail in EV) keeps of capture sharpening under Detail
/// `detail` (0..1): all of it for fine texture, only `detail` of it where halos would show.
#[inline]
pub(crate) fn capture_keep(detail: f32, a: f32) -> f32 {
    detail + (1.0 - detail) * (1.0 - smooth(0.15, 1.0, a))
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How much of the sharpening a pixel gets (0..1) under Masking `masking` (0..1), from its fine
/// detail `det` (log luminance minus its texture blur): 1 everywhere at 0, only stronger edges
/// as it rises. The GPU kernel (`finish.wgsl`) computes the same; Alt-drag on Masking shows it
/// ([`crate::Overlay::SharpenMask`]).
#[inline]
pub fn sharpen_mask_weight(masking: f32, det: f32) -> f32 {
    if masking > 0.0 { smooth(masking * 0.25, masking * 0.25 + 0.15, det.abs()) } else { 1.0 }
}

/// Parametric region curve (encoded domain) composed with the master point curve.
fn curve_luts(c: &ToneCurve) -> Option<[Lut1; 3]> {
    let parametric = c.highlights != 0.0 || c.lights != 0.0 || c.darks != 0.0 || c.shadows != 0.0;
    let master = !ToneCurve::point_curve_is_identity(&c.master);
    let chans = [&c.red, &c.green, &c.blue].map(|p| !ToneCurve::point_curve_is_identity(p));
    if !parametric && !master && !chans.iter().any(|b| *b) {
        return None;
    }
    const N: usize = 1024;
    let mut base = crate::curve::ParametricCurve::new(c).to_lut();
    let to_pts = |p: &[Point]| p.iter().map(|q| (q.x, q.y)).collect::<Vec<_>>();
    if master {
        base = MonotoneCurve::new(&to_pts(&c.master)).to_lut(N).compose(&base);
    }
    let per = [&c.red, &c.green, &c.blue];
    Some(std::array::from_fn(|i| {
        if chans.get(i).copied().unwrap_or(false) {
            let pts = per.get(i).map(|&p| to_pts(p)).unwrap_or_default();
            MonotoneCurve::new(&pts).to_lut(N).compose(&base)
        } else {
            base.clone()
        }
    }))
}

/// A LUT profile's table is made for sRGB. For an output with other primaries (Display P3, a monitor's own, Adobe RGB,
/// ProPhoto, Rec. 2020) the colour goes to sRGB's primaries through these matrices (linear output → linear sRGB, and
/// back), takes the lookup there and returns, so a look is the same in a preview and in any export. `None` for sRGB
/// itself or a display whose primaries are sRGB's to within a thousandth (and for primaries that can't be inverted).
pub fn lut_space(to_out: &[[f32; 3]; 3]) -> Option<([[f32; 3]; 3], [[f32; 3]; 3])> {
    let srgb = crate::output::OutputSpace::Srgb.from_working();
    if to_out.iter().flatten().zip(srgb.iter().flatten()).all(|(a, b)| (a - b).abs() < 1e-3) {
        return None;
    }
    let m = |a: &[[f32; 3]; 3]| Mat3(a.map(|r| r.map(f64::from)));
    let (out, s) = (m(to_out), m(&srgb));
    // linear output → Rec.2020 → sRGB, and back
    Some((s.mul(&out.inverse()?).to_f32(), out.mul(&s.inverse()?).to_f32()))
}

/// The LUT profile's lookup of the display-encoded colour `e` (output primaries, sRGB curve), blended by its amount `k`:
/// the table sees SDR values (an HDR render keeps what is above SDR white), in sRGB's primaries when `space` says so.
/// The GPU kernel (`finish.wgsl`, `lut_through`) computes the same.
#[inline]
fn lut_pixel(l: &crate::lut::Lut3d, k: f32, e: [f32; 3], space: Option<&([[f32; 3]; 3], [[f32; 3]; 3])>, hdr: bool) -> [f32; 3] {
    let through = |e: [f32; 3]| {
        let c = e.map3(|v| v.clamp(0.0, 1.0));
        let m = l.apply(c);
        // (an SDR render pulls towards the table's colour from where it is, an HDR one from the clamped colour)
        let from = if hdr { c } else { e };
        [e[0] + (m[0] - from[0]) * k, e[1] + (m[1] - from[1]) * k, e[2] + (m[2] - from[2]) * k]
    };
    match space {
        None => through(e),
        Some((to_s, from_s)) => {
            let s = mul3(to_s, e.map3(srgb_to_linear)).map3(linear_to_srgb);
            mul3(from_s, through(s).map3(srgb_to_linear)).map3(linear_to_srgb)
        }
    }
}

/// Vignette (post-crop) parameters.
#[derive(Clone, Copy, Debug)]
pub struct Vig {
    pub amount: f32,
    pub start: f32,
    pub width: f32,
    pub aspect_mix: f32,
    pub power: f32,
    pub highlights: f32,
    pub style: VignetteStyle,
}

fn vignette(s: &DevelopSettings) -> Option<Vig> {
    let v = &s.vignette;
    (v.amount != 0.0).then(|| {
        let r = (v.roundness / 100.0) as f32;
        Vig {
            amount: (v.amount / 100.0) as f32,
            start: 0.15 + (v.midpoint / 100.0) as f32 * 0.95,
            width: 0.05 + (v.feather / 100.0) as f32 * 1.1,
            aspect_mix: ((r + 1.0) / 2.0).clamp(0.0, 1.0),
            power: if r >= 0.0 { 2.0 } else { 2.0 + (-r) * 6.0 },
            highlights: (v.highlights / 100.0) as f32,
            style: v.style,
        }
    })
}

/// Hash constants of [`grain_node`] (shared with the GPU kernel).
pub const GRAIN_HASH: [u32; 4] = [0x8da6_b343, 0xd816_3841, 0xcb1a_b31f, 0x5bd1_e995];

/// Grain lattice spacing in pixels of a 6000 px long edge at Grain Size 0 and per Size point: Lightroom's grain at
/// Size 25 on a 6000 px export has its power spectrum flat to 0.3 cycles/px and falling steeply after, which is white
/// noise on a 1.5 px lattice reconstructed with a cubic (`grain_weights`). Only Size 25 was measured; the rest of the
/// range is extrapolated linearly.
const GRAIN_CELL_BASE: f32 = 1.0;
const GRAIN_CELL_PER_SIZE: f32 = 0.02;
/// The long edge (px) the lattice spacings above are given for; a render's lattice scales with its long edge.
const GRAIN_REF_LONG: f32 = 6000.0;
/// Lattice units per output pixel (1 / the Size 25 spacing) up to which the grain keeps its full strength; above it a
/// pixel averages several lattice cells and the grain loses strength, as Lightroom's full-resolution grain does when
/// it is downsized.
const GRAIN_FULL_RES: f32 = 2.0 / 3.0;

/// Grain of an amount (0..1), Size and Roughness (0..1) for a render of `px_per_long` output pixels per long
/// edge: the strength (std of the noise at mid grey), the lattice spacing in output pixels, and the weight of
/// a coarser second layer (Roughness above 50 clumps the grain).
pub fn grain_params(amount: f32, size: f32, roughness: f32, px_per_long: f32) -> (f32, f32, f32) {
    let cell = (GRAIN_CELL_BASE + GRAIN_CELL_PER_SIZE * size * 100.0) * (0.75 + 0.5 * roughness.min(0.5)) * px_per_long / GRAIN_REF_LONG;
    let cell = cell.max(1e-4);
    // averaging `f` lattice cells per pixel leaves 1/sqrt(1 + 0.9 f²) of the strength (measured on the field)
    let f = 1.0 / cell;
    let att = ((1.0 + 0.9 * GRAIN_FULL_RES * GRAIN_FULL_RES) / (1.0 + 0.9 * f * f)).sqrt().min(1.0);
    (amount * att, cell, ((roughness - 0.5) * 0.9).max(0.0))
}

/// Std of the grain added to an encoded channel value `v` (0..1) at Grain 100, in encoded units: Lightroom's
/// measured profile (0.066 at the extremes of the range, 0.084 at mid grey).
#[inline]
pub fn grain_strength(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    0.0575 + 0.106 * v * (1.0 - v)
}

/// One lattice node's value: unit variance, triangular distribution, from a hash of its cell and the seed.
#[inline]
fn grain_node(i: i32, j: i32, seed: u32) -> f32 {
    let mut v = (i as u32).wrapping_mul(GRAIN_HASH[0]) ^ (j as u32).wrapping_mul(GRAIN_HASH[1]) ^ seed.wrapping_mul(GRAIN_HASH[2]);
    v ^= v >> 16;
    v = v.wrapping_mul(GRAIN_HASH[3]);
    v ^= v >> 13;
    v = v.wrapping_mul(0x85eb_ca6b);
    v ^= v >> 16;
    (((v & 0xffff) + (v >> 16)) as f32 * (1.0 / 32768.0) - 2.0) * 1.224_744_9
}

/// Weights of the four lattice nodes (one before, the cell's two, one after) at fraction `t` of the cell: the
/// Mitchell-Netravali cubic with B = 0.1, C = 0.45, which sums to 1 and has the flat-then-steep spectrum of
/// Lightroom's grain.
#[inline]
fn grain_weights(t: f32) -> [f32; 4] {
    let near = |x: f32| 0.966_667 + x * x * (1.4 * x - 2.35);
    let far = |x: f32| 1.933_333 + x * (-3.8 + x * (2.35 - 0.466_667 * x));
    [far(1.0 + t), near(t), near(1.0 - t), far(2.0 - t)]
}

/// Grain at lattice coordinates `(x, y)`: white noise on the lattice, reconstructed by [`grain_weights`] and
/// scaled to unit variance at every position.
#[inline]
fn grain_field(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (wx, wy) = (grain_weights(x - x0), grain_weights(y - y0));
    let (i, j) = (x0 as i32, y0 as i32);
    let mut sum = 0.0;
    for (dj, wj) in wy.iter().enumerate() {
        for (di, wi) in wx.iter().enumerate() {
            sum += wi * wj * grain_node(i.wrapping_add(di as i32 - 1), j.wrapping_add(dj as i32 - 1), seed);
        }
    }
    let norm = wx.iter().map(|w| w * w).sum::<f32>() * wy.iter().map(|w| w * w).sum::<f32>();
    sum / norm.sqrt().max(1e-6)
}

/// The grain at lattice coordinates `(x, y)`: [`grain_field`] with `coarse` of a second layer three times
/// coarser (variance kept at 1).
#[inline]
fn grain_noise(x: f32, y: f32, coarse: f32, seed: u32) -> f32 {
    let g = grain_field(x, y, seed);
    if coarse > 0.0 { g * (1.0 - coarse * coarse).max(0.0).sqrt() + grain_field(x / 3.0, y / 3.0, seed ^ 0x55) * coarse } else { g }
}

/// Number of per-mask terms in [`mask_terms`].
pub const MASK_TERMS: usize = 21;

/// Number of alpha-weighted terms at the start of [`mask_terms`].
pub const MASK_SUMS: usize = 17;

/// Positive dehaze from process V4 (#1237; upstream's "V3") of scene-linear `c`: `dz` the strength (0..1, with a
/// local adjustment's added), `d` the dark channel over the airlight (0..1), `air` the airlight.
/// The strength eases in (`dz·(0.4 + 0.6·dz)`: +10 acts at about half V1's), the transmission is
/// floored at 0.15, and the veil removed never exceeds 90 % of the pixel's darkest channel, so no
/// pixel is pushed to black (V1 clipped shadows next to bright sky from +10). The GPU mirrors it in
/// `finish.wgsl`. Sources: He, Sun, Tang, "Single Image Haze Removal Using Dark Channel Prior"
/// (CVPR 2009), with own easing and veil limit.
#[inline]
pub fn dehaze_v4_pixel(c: [f32; 3], dz: f32, d: f32, air: f32) -> [f32; 3] {
    let k = dz.clamp(0.0, 1.0);
    let k = k * (0.4 + 0.6 * k);
    let t = (1.0 - 0.95 * k * d).max(0.15);
    let veil = (air * (1.0 - t)).min(0.9 * c[0].min(c[1]).min(c[2]).max(0.0));
    c.map(|v| ((v - veil) / t).max(0.0))
}

/// Negative dehaze (haze added) pulls scene-linear `c` towards the airlight `target` by `k`, in the space of
/// `c^(1 / NEG_DEHAZE_GAMMA)`: in linear light a few percent of airlight lifted the darks by tens of L\*, where
/// Lightroom's -16 on the Phoenix 200 preset keeps its blacks and fogs the mids and highlights (measured on its export
/// of an ILCE-6000 raw). The GPU mirrors it in `finish.wgsl`.
#[inline]
pub fn add_haze(c: [f32; 3], target: f32, k: f32) -> [f32; 3] {
    let t = target.max(0.0).powf(1.0 / NEG_DEHAZE_GAMMA);
    c.map(|v| (v.max(0.0).powf(1.0 / NEG_DEHAZE_GAMMA) * (1.0 - k) + t * k).powf(NEG_DEHAZE_GAMMA))
}

/// The gamma of [`add_haze`]'s blend.
pub const NEG_DEHAZE_GAMMA: f32 = 1.4;
/// Strength of negative dehaze: the share of the airlight blended in at Dehaze -100 over a fully hazy pixel.
pub const NEG_DEHAZE_STRENGTH: f32 = 0.8;
/// Positive Highlights gain (log2 luminance per unit of the slider) over the Shadows' 1.7: Lightroom's +49 lifts bright
/// areas about 1.5 times as much (fitted to its export of the Phoenix 200 preset, docs/lightroom-preset-fit.md).
pub const HIGHLIGHTS_UP: f32 = 2.5;

/// A mask's local adjustments as the per-pixel stage uses them (alpha-weighted sums), in order:
/// exposure, temp, tint, contrast, highlights, shadows, whites, blacks, texture, clarity, dehaze,
/// saturation, hue, sharpness, noise, moiré, defringe, then the colour overlay: on (0/1), cos and
/// sin of its OkLCh hue, and its strength.
pub fn mask_terms(j: &LocalAdjustments) -> [f32; MASK_TERMS] {
    let (on, cos, sin, amt) = if j.color_sat > 0.0 {
        let hue = crate::colorops::oklch_hue_of_srgb_hue(j.color_hue);
        (1.0, hue.cos(), hue.sin(), (j.color_sat / 100.0) as f32)
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };
    [
        j.exposure as f32,
        (j.temp / 100.0) as f32,
        (j.tint / 100.0) as f32,
        (j.contrast / 100.0) as f32,
        (j.highlights / 100.0) as f32,
        (j.shadows / 100.0) as f32,
        (j.whites / 100.0) as f32,
        (j.blacks / 100.0) as f32,
        (j.texture / 100.0) as f32,
        (j.clarity / 100.0) as f32,
        (j.dehaze / 100.0) as f32,
        (j.saturation / 100.0) as f32,
        (j.hue / 100.0) as f32 * 0.6,
        (j.sharpness / 100.0) as f32,
        (j.noise / 100.0) as f32,
        (j.moire / 100.0) as f32,
        (j.defringe / 100.0) as f32,
        on,
        cos,
        sin,
        amt,
    ]
}

/// Everything the per-pixel stage computes once per render: the CPU loop below and the GPU kernel
/// (`lightcraft-gpu`) both read their parameters from here, so the two cannot drift apart.
pub struct FinishParams {
    /// A LUT profile and its amount (0..2), applied to the display-encoded colour.
    pub lut: Option<(std::sync::Arc<crate::lut::Lut3d>, f32)>,
    pub tone: ToneMap,
    pub ops: ColorOps,
    /// Calibration: primaries matrix (row-major, linear Rec.2020) and shadows tint (−1..1).
    pub calib: Option<[[f32; 3]; 3]>,
    pub shadow_tint: f32,
    /// Tone curves (parametric ∘ point, per channel) on encoded values, 1024 entries each.
    pub curves: Option<[Lut1; 3]>,
    /// Refine Saturation as 0..1 (1 = the curves' own saturation).
    pub refine_sat: f32,
    pub vig: Option<Vig>,
    /// Linear Rec.2020 → linear output RGB, the output's luminance weights (gamut mapping) and its
    /// encoding curve (see [`crate::output`]).
    pub to_out: [[f32; 3]; 3],
    pub out_luma: [f32; 3],
    pub out_trc: OutputTrc,
    /// Soft proofing (CPU only; the GPU path declines proof renders).
    pub proof: Option<crate::output::ProofParams>,
    pub hl: f32,
    pub sh: f32,
    pub clar: f32,
    pub tex: f32,
    pub dehaze: f32,
    /// Dehaze's process-V4 response ([`dehaze_v4_pixel`]) instead of V1's.
    pub dehaze_v4: bool,
    /// Refinements made while the fork had no process versions, pinned to V5.
    pub fork_refinements: bool,
    pub sharpen: f32,
    pub sharpen_mask: f32,
    /// Sharpening reads the texture band (before process V3); from V3 it reads the capture plane.
    pub band_sharpen: bool,
    /// Capture sharpening's parameters (from V3, when the render has its plane: the GPU kernel's).
    pub capture: Option<crate::local::Capture>,
    /// Airlight after and before exposure, exposure gain and EV (see [`crate::Prepared`]).
    pub air: f32,
    pub air_pre: f32,
    pub gain: f32,
    pub ev: f32,
    /// Grain: strength, lattice spacing (output px), coarse layer weight, seed (see [`grain_params`]).
    pub grain: Option<(f32, f32, f32, u32)>,
    /// Output px → normalized oriented coordinates.
    pub out_to_norm: lightcraft_geom::Affine,
    pub ow: f64,
    pub oh: f64,
    pub w: usize,
    pub h: usize,
    pub px_per_long: f64,
    /// The window's offset and the whole output's size (pixels): `(0, 0, w, h)` unless windowed.
    pub view: [f32; 4],
    /// Peak linear output relative to SDR white: 1 for SDR renders; `2^max_ev` for an HDR render
    /// ([`OutputDepth::F32Hdr`] with HDR on), which keeps values above 1 instead of clipping them.
    pub peak: f32,
}

impl FinishParams {
    /// Parameters for a `w × h` render into `space`; `ev`/`air_pre` as in [`crate::Prepared`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        s: &DevelopSettings,
        frame: &Frame,
        info: &SourceInfo,
        w: usize,
        h: usize,
        px_per_long: f64,
        air_pre: f32,
        space: OutputSpace,
    ) -> FinishParams {
        let effects = s.section_enabled("effects");
        let (clar, tex, dehaze) = if effects {
            ((s.effects.clarity / 100.0) as f32, (s.effects.texture / 100.0) as f32, (s.effects.dehaze / 100.0) as f32)
        } else {
            (0.0, 0.0, 0.0)
        };
        let ev = s.light.exposure as f32;
        let gain = 2f32.powf(ev);
        let grain = (s.grain.amount > 0.0 && effects).then(|| {
            let (amt, cell, coarse) =
                grain_params((s.grain.amount / 100.0) as f32, (s.grain.size / 100.0) as f32, (s.grain.roughness / 100.0) as f32, px_per_long as f32);
            (amt, cell, coarse, s.grain.seed)
        });
        let calibration = s.section_enabled("calibration");
        let peak = s.hdr.peak();
        FinishParams {
            peak,
            calib: if calibration { crate::colorops::calibration_matrix(&s.calibration) } else { None },
            shadow_tint: if calibration { (s.calibration.shadows_tint / 100.0) as f32 } else { 0.0 },
            tone: base_tone(s, info, peak),
            lut: crate::lut::get(&s.profile.id).map(|l| (l, (s.profile.amount / 100.0).clamp(0.0, 2.0) as f32)),
            ops: ColorOps::new(s),
            curves: curve_luts(&s.curve),
            refine_sat: (s.curve.refine_saturation / 100.0).clamp(0.0, 1.0) as f32,
            vig: if effects { vignette(s) } else { None },
            to_out: space.from_working(),
            out_luma: space.luma(),
            out_trc: space.trc(),
            proof: None,
            hl: (s.light.highlights / 100.0) as f32,
            sh: (s.light.shadows / 100.0) as f32,
            clar,
            tex,
            dehaze,
            dehaze_v4: matches!(s.process.process(), Process::V4 | Process::V5),
            fork_refinements: s.process.process() == Process::V5,
            sharpen: (s.detail.sharpen_amount / 150.0) as f32,
            sharpen_mask: (s.detail.sharpen_masking / 100.0) as f32,
            band_sharpen: !crate::local::capture_process(s),
            capture: None,
            air: air_pre * gain,
            air_pre,
            gain,
            ev,
            grain,
            out_to_norm: frame.out_to_norm(w, h),
            ow: frame.ow,
            oh: frame.oh,
            w,
            h,
            px_per_long,
            view: frame.view.map_or([0.0, 0.0, w as f32, h as f32], |v| [v.x as f32, v.y as f32, v.full_w as f32, v.full_h as f32]),
        }
    }

    /// Render into a display's primaries (sRGB curve) instead of the output space, soft proof
    /// included (see [`crate::DisplaySpace`]).
    pub fn for_display(&mut self, d: &crate::DisplaySpace, proof: Option<crate::Proof>) {
        self.to_out = d.from_working;
        self.out_luma = d.luma;
        self.out_trc = OutputTrc::Srgb;
        self.proof = proof.map(|pr| pr.params_display(d));
    }
}

/// The base tone map (scene → display luminance, with Contrast, Whites and Blacks) of `s`'s
/// rendering process. This is where a new tone model branches: a retuned default tone curve (issue
/// #146) is a new [`Process`] with its own arm here, and V1's arm stays as it is, so photos edited
/// under V1 keep their look (`docs/process-versions.md`). CPU and GPU both take it from here.
/// `peak` is the HDR render's peak relative to SDR white ([`lightcraft_develop::Hdr::peak`]); at 1.0 (SDR)
/// every arm is the SDR tone map. An HDR render keeps the camera's look through the midtones and
/// lifts its highlights past white ([`ToneMap::camera_hdr`]), so it matches the SDR render below them.
pub fn base_tone(s: &DevelopSettings, info: &SourceInfo, peak: f32) -> ToneMap {
    let l = &s.light;
    match s.process.process() {
        // V2 changes the Highlights/Shadows base (`local::tone_base`), V3 sharpening, V4 dehaze: not the tone map
        Process::V1 | Process::V2 | Process::V3 | Process::V4 | Process::V5 => match info.camera_tone.as_ref().filter(|_| info.raw) {
            Some(curve) => ToneMap::camera_hdr(curve, l.contrast, l.whites, l.blacks, peak),
            None if info.raw => ToneMap::hdr(l.contrast, l.whites, l.blacks, peak),
            None => ToneMap::display_hdr(l.contrast, l.whites, l.blacks, peak),
        },
    }
}

pub(crate) fn finish(
    p: &Prepared,
    s: &DevelopSettings,
    frame: &Frame,
    info: &SourceInfo,
    space: OutputSpace,
    display: Option<&crate::DisplaySpace>,
    proof: Option<crate::Proof>,
) -> Rgba8 {
    let (w, h) = (p.img.width, p.img.height);
    let mut fp = FinishParams::new(s, frame, info, w, h, p.px_per_long, p.air, space);
    fp.proof = proof.map(|pr| pr.params(space));
    if let Some(d) = display {
        fp.for_display(d, proof);
    }
    let trc = fp.out_trc;
    let data = finish_with(p, &fp, false, |e| match trc {
        OutputTrc::Srgb => [enc(e[0]), enc(e[1]), enc(e[2]), 255],
        t => {
            let x = e.map(|v| enc(t.encode(lightcraft_color::transfer::srgb_to_linear(v.clamp(0.0, 1.0)))));
            [x[0], x[1], x[2], 255]
        }
    });
    Rgba8 { width: w, height: h, data }
}

/// [`finish`] into 16-bit display-encoded or 32-bit float linear samples (output primaries).
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_deep(
    p: &Prepared,
    s: &DevelopSettings,
    frame: &Frame,
    info: &SourceInfo,
    space: OutputSpace,
    depth: OutputDepth,
    proof: Option<crate::Proof>,
) -> DeepImage {
    use lightcraft_color::transfer::srgb_to_linear;
    let (w, h) = (p.img.width, p.img.height);
    let mut fp = FinishParams::new(s, frame, info, w, h, p.px_per_long, p.air, space);
    fp.proof = proof.map(|pr| pr.params(space));
    let trc = fp.out_trc;
    let samples = match depth {
        OutputDepth::F32Linear => {
            let v = finish_with(p, &fp, true, |e| e.map3(|v| srgb_to_linear(v.clamp(0.0, 1.0))));
            DeepSamples::F32(v.into_flattened())
        }
        OutputDepth::F32Hdr => {
            // the sRGB curve extended past 1 (it is unbounded), so values up to `peak` survive
            let peak = fp.peak;
            let v = finish_with(p, &fp, true, |e| e.map3(|v| srgb_to_linear(v.max(0.0)).clamp(0.0, peak)));
            DeepSamples::F32(v.into_flattened())
        }
        _ => {
            let q = |v: f32| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            let v = finish_with(p, &fp, true, |e| match trc {
                OutputTrc::Srgb => e.map(q),
                t => e.map(|v| q(t.encode(srgb_to_linear(v.clamp(0.0, 1.0))))),
            });
            DeepSamples::U16(v.into_flattened())
        }
    };
    DeepImage { width: w, height: h, space, samples }
}

/// The per-pixel stage: every output pixel's colour in the output primaries, encoded with the sRGB
/// curve (tone curves and grain applied; not clamped), handed to `store` for the final encoding.
/// `exact`: encode with the exact sRGB curve instead of the (8/10-bit accurate) table.
pub(crate) fn finish_with<T: Copy + Default + Send>(
    p: &Prepared,
    fp: &FinishParams,
    exact: bool,
    store: impl Fn([f32; 3]) -> T + Sync + Send,
) -> Vec<T> {
    let (w, h) = (p.img.width, p.img.height);
    let p_lut = fp.lut.clone();
    let lut_sp = p_lut.as_ref().and_then(|_| lut_space(&fp.to_out));
    let FinishParams {
        tone,
        ops,
        curves,
        vig,
        to_out,
        out_luma,
        hl,
        sh,
        clar,
        tex,
        dehaze,
        dehaze_v4,
        fork_refinements,
        sharpen,
        sharpen_mask,
        band_sharpen,
        air,
        air_pre,
        gain,
        ev,
        grain,
        ..
    } = fp;
    let band_sharpen = *band_sharpen;
    let dehaze_v4 = *dehaze_v4;
    let fork_refinements = *fork_refinements;
    let (hl, sh, clar, tex, dehaze, sharpen, sharpen_mask, air, air_pre, gain, ev) =
        (*hl, *sh, *clar, *tex, *dehaze, *sharpen, *sharpen_mask, *air, *air_pre, *gain, *ev);
    let terms: Vec<[f32; MASK_TERMS]> = p.masks.iter().map(|m| mask_terms(&m.adjust)).collect();
    let out_to_norm = fp.out_to_norm;
    let long = fp.ow.max(fp.oh);
    let [vx, vy, vw, vh] = fp.view;
    // the vignette belongs to the whole output frame, not to the window being drawn
    let aspect = vw / vh;

    let srgb = srgb_lut();
    let peak = fp.peak;
    let hdr = peak > 1.0;
    let mut out = vec![T::default(); w * h];
    for_rows(&mut out, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let raw = p.img.data[i];
            let mut c = if gain == 1.0 { raw } else { raw.map3(|v| v * gain) };
            let l_pre = p.log_l.data[i];
            let l0 = l_pre + ev;

            // --- local (mask) contributions: alpha-weighted sums of the masks' terms
            let mut l = [0.0f32; MASK_SUMS];
            let mut tint_col: Option<([f32; 3], f32)> = None;
            for (m, t) in p.masks.iter().zip(&terms) {
                let a = m.alpha.data[i];
                if a <= 0.0 {
                    continue;
                }
                for k in 0..MASK_SUMS {
                    l[k] += a * t[k];
                }
                if t[MASK_SUMS] > 0.0 {
                    tint_col = Some(([t[MASK_SUMS + 1], t[MASK_SUMS + 2], 0.0], a * t[MASK_SUMS + 3]));
                }
            }
            let [l_exp, l_temp, l_tint, l_con, l_hl, l_sh, l_wh, l_bl, l_tex, l_clar, l_dehaze, l_sat, l_hue, l_sharp, l_noise, l_moire, l_defringe] =
                l;

            // --- local Moiré (and the colour part of Noise): chromaticity towards its blur
            let mt = (l_moire + 0.5 * l_noise.max(0.0)).clamp(-1.0, 1.0);
            if mt != 0.0
                && let Some(cb) = &p.chroma_blur
            {
                let (y, ch0, cb) = (luminance_2020(c), crate::masks::chromaticity(raw), cb.data[i]);
                c = std::array::from_fn(|k| ((ch0[k] + (cb[k] - ch0[k]) * mt) * y).max(0.0));
            }
            // --- local Defringe: desaturate purple / green fringes along edges
            let df = l_defringe.clamp(0.0, 1.0);
            if df > 0.0
                && let Some(b) = &p.texture_blur
            {
                let k = df * defringe_weight(c, p.log_l.data[i] - b.data[i]);
                if k > 0.0 {
                    let y = luminance_2020(c);
                    c = c.map3(|v| v + (y - v) * k);
                }
            }

            // --- dehaze (scene linear)
            let dz = dehaze + l_dehaze;
            if dz != 0.0
                && let Some(dark) = &p.dark
            {
                // The dark channel is a blur, not a patch minimum: near bright neighbours it credits
                // dark pixels with veil they don't hold (and a windowed render borrows the whole
                // frame's airlight), so the subtraction went past zero and crushed them to black
                // (#2504). Never attribute more veil than the pixel's own minimum channel.
                let c_min = c[0].min(c[1]).min(c[2]);
                let raw_d = if air_pre > 0.0 { dark.data[i] / air_pre } else { 0.0 };
                let d = if fork_refinements { raw_d.min(if air > 0.0 { c_min / air } else { 0.0 }) } else { raw_d }.clamp(0.0, 1.0);
                if dz > 0.0 && dehaze_v4 {
                    c = dehaze_v4_pixel(c, dz, d, air);
                } else if dz > 0.0 {
                    let t = (1.0 - 0.95 * dz.min(1.0) * d).max(0.12);
                    c = c.map3(|v| ((v - air * (1.0 - t)) / t).max(0.0));
                } else {
                    let k = (-dz).min(1.0) * NEG_DEHAZE_STRENGTH * (0.35 + 0.65 * d);
                    c = if fork_refinements { add_haze(c, air * 0.9, k) } else { c.map3(|v| v + (air * 0.9 - v) * k) };
                }
            }

            // --- local exposure / temp / tint
            if l_exp != 0.0 {
                let g = l_exp.exp2();
                c = c.map3(|v| v * g);
            }
            if l_temp != 0.0 || l_tint != 0.0 {
                let y0 = luminance_2020(c);
                c = [c[0] * (1.0 + 0.3 * l_temp), c[1] * (1.0 - 0.22 * l_tint), c[2] * (1.0 - 0.3 * l_temp).max(0.0)];
                let y1 = luminance_2020(c).max(1e-9);
                c = c.map3(|v| v * y0 / y1);
            }

            // --- local tone in log luminance
            let l1 = if dz != 0.0 || l_exp != 0.0 { log_lum(c) } else { l0 };
            let shift = l1 - l0;
            let base = p.base.data[i] + ev + shift;
            let mut delta = 0.0f32;
            let (hh, ss) = (hl + l_hl, sh + l_sh);
            if hh != 0.0 || ss != 0.0 {
                let ws = 1.0 - smooth(-4.8, 0.3, base);
                let wh = smooth(-1.0, 2.8, base);
                delta += ss * 1.7 * ws * ws.sqrt() + hh * if hh > 0.0 && fork_refinements { HIGHLIGHTS_UP } else { 1.7 } * wh;
            }
            if l_wh != 0.0 {
                delta += l_wh * 0.8 * smooth(0.5, 3.0, l1);
            }
            if l_bl != 0.0 {
                delta += l_bl * 0.8 * (1.0 - smooth(-6.0, -1.5, l1));
            }
            if l_con != 0.0 {
                delta += l_con * 0.14 * l1.clamp(-6.0, 4.0);
            }
            let cl = clar + l_clar;
            if cl != 0.0
                && let Some(b) = &p.clarity_blur
            {
                let det = (l_pre - b.data[i]).clamp(-2.5, 2.5);
                let mid = (-(base / 3.2).powi(2)).exp();
                delta += cl * 0.85 * det * (0.35 + 0.65 * mid);
            }
            let tx = tex + l_tex;
            let sp = l_sharp * 0.6 + sharpen;
            // (before V3 sharpening reads the texture band; from V3 its own plane, `Capture`)
            let sp_band = if band_sharpen { sp } else { 0.0 };
            if (tx != 0.0 || sp_band != 0.0)
                && let Some(b) = &p.texture_blur
            {
                let det = l_pre - b.data[i];
                let tame = 1.0 - 0.6 * smooth(0.4, 1.6, det.abs());
                delta += tx * 1.1 * det.clamp(-1.0, 1.0) * tame;
                if sp_band != 0.0 {
                    let m = sharpen_mask_weight(sharpen_mask, det);
                    delta += sp_band * 1.3 * det.clamp(-0.8, 0.8) * m;
                }
            }
            if sp != 0.0
                && let Some((cap, b)) = &p.sharpen
            {
                delta += capture_delta(sp, (l_pre - b.data[i]) * cap.w, cap.detail, sharpen_mask);
            }
            // local Noise: smooth (or, negative, boost) small-amplitude detail, keep edges
            if l_noise != 0.0
                && let Some(b) = &p.texture_blur
            {
                let det = l_pre - b.data[i];
                delta -= l_noise.clamp(-1.0, 1.0) * 0.9 * det * (1.0 - smooth(0.1, 0.5, det.abs()));
            }
            if delta != 0.0 {
                let g = delta.exp2();
                c = c.map3(|v| v * g);
            }

            // --- calibration (scene linear, before the tone map)
            if fp.calib.is_some() || fp.shadow_tint != 0.0 {
                c = crate::colorops::calibrate(c, fp.calib.as_ref(), fp.shadow_tint);
            }

            // --- tone map on luminance, highlight desaturation
            let yl = luminance_2020(c);
            let o = tone.apply(yl);
            let mut d = if yl > 1e-9 { c.map3(|v| v * o / yl) } else { [0.0; 3] };
            let k = tone.chroma_scale(o);
            if k != 1.0 {
                d = d.map3(|v| o + (v - o) * k);
            }
            let mx = d[0].max(d[1]).max(d[2]);
            if mx > peak {
                let t = ((mx - peak) / (mx - o).max(1e-6)).clamp(0.0, 1.0);
                d = d.map3(|v| v + (o - v) * t);
            }

            // --- colour
            d = ops.apply(d, l_sat, l_hue);
            if let Some((dir, amt)) = tint_col {
                let lab = lightcraft_color::perceptual::oklab_from_2020(d);
                d = lightcraft_color::perceptual::oklab_to_2020([lab[0], lab[1] + dir[0] * 0.08 * amt, lab[2] + dir[1] * 0.08 * amt]);
            }

            // --- vignette (display linear, post-crop)
            if let Some(v) = vig {
                let u = (vx + x as f32 + 0.5) / vw * 2.0 - 1.0;
                let vv = (vy + y as f32 + 0.5) / vh * 2.0 - 1.0;
                let sx = 1.0 + (aspect - 1.0) * v.aspect_mix;
                let sy = 1.0 + (1.0 / aspect - 1.0) * v.aspect_mix;
                let (ax, ay) = ((u * sx.max(1.0) / sx.max(sy)).abs(), (vv * sy.max(1.0) / sx.max(sy)).abs());
                let dist = (ax.powf(v.power) + ay.powf(v.power)).powf(1.0 / v.power);
                let t = smooth(v.start, v.start + v.width, dist);
                if t > 0.0 {
                    let lum = luminance_2020(d).clamp(0.0, 1.0);
                    if v.amount < 0.0 {
                        let mut f = 1.0 + v.amount * t;
                        if v.style == VignetteStyle::HighlightPriority {
                            f += (1.0 - f) * v.highlights * smooth(0.4, 1.0, lum);
                        }
                        if v.style == VignetteStyle::PaintOverlay {
                            d = d.map3(|c| c * (1.0 - (-v.amount) * t) + 0.0);
                        } else {
                            d = d.map3(|c| c * f);
                        }
                    } else if hdr {
                        // lighten towards SDR white; never pull HDR highlights down to it
                        d = d.map3(|c| c + (1.0 - c).max(0.0) * v.amount * t * 0.85);
                    } else {
                        d = d.map3(|c| c + (1.0 - c) * v.amount * t * 0.85);
                    }
                }
            }

            // --- gamut map to the output space (desaturate towards luminance until in range);
            // soft proofing maps into the proof space first and shows that in the output space
            let mut warn = None;
            let mut r = match &fp.proof {
                Some(pp) if pp.table.is_some() => {
                    let (q, dist) = pp.table.as_ref().map_or((d, 0.0), |t| t.sample(d));
                    if pp.dest_warning && dist > crate::output::PROOF_GAMUT_TOLERANCE {
                        warn = Some(crate::output::PROOF_DEST_WARNING);
                    }
                    mul3(to_out, q)
                }
                Some(pp) => {
                    let q0 = mul3(&pp.to_proof, d);
                    let (q, t) = gamut_map_to(q0, pp.luma, peak);
                    if pp.dest_warning && out_of_gamut(q0, t) {
                        warn = Some(crate::output::PROOF_DEST_WARNING);
                    }
                    mul3(&pp.proof_to_out, q)
                }
                None => mul3(to_out, d),
            };
            let (mapped, t) = gamut_map_to(r, *out_luma, peak);
            if fp.proof.as_ref().is_some_and(|pp| pp.display_warning) && warn.is_none() && out_of_gamut(r, t) {
                warn = Some(crate::output::PROOF_DISPLAY_WARNING);
            }
            r = mapped;

            // --- encode, curves, grain
            let mut e = if hdr {
                // HDR: the (unbounded) sRGB curve, so values above SDR white encode above 1
                r.map3(|v| {
                    if v > 1.0 {
                        linear_to_srgb(v)
                    } else if exact {
                        linear_to_srgb(v.max(0.0))
                    } else {
                        encode_srgb(srgb, v)
                    }
                })
            } else if exact {
                r.map3(|v| linear_to_srgb(v.clamp(0.0, 1.0)))
            } else {
                r.map3(|v| encode_srgb(srgb, v))
            };
            if let Some(l) = curves {
                let e0 = e;
                e = if hdr {
                    // above SDR white the curves continue with slope 1 from their end point
                    std::array::from_fn(|k| if e[k] > 1.0 { l[k].eval(1.0) + (e[k] - 1.0) } else { l[k].eval(e[k]) })
                } else {
                    [l[0].eval(e[0]), l[1].eval(e[1]), l[2].eval(e[2])]
                };
                if fp.refine_sat < 1.0 {
                    e = refine_saturation(e0, e, fp.refine_sat);
                }
            }
            if let Some((amt, cell, coarse, seed)) = *grain {
                let n = out_to_norm.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
                let (gx, gy) = ((n.x * fp.ow) as f32 / long as f32, (n.y * fp.oh) as f32 / long as f32);
                let sc = fp.px_per_long as f32 / cell;
                let g = amt * grain_noise(gx * sc, gy * sc, coarse, seed);
                e = e.map3(|v| v + g * grain_strength(v));
            }
            let e = match &p_lut {
                Some((l, k)) => lut_pixel(l, *k, e, lut_sp.as_ref(), hdr),
                None => e,
            };
            *px = store(warn.unwrap_or(e));
        }
    });
    out
}

/// A colour needing more desaturation than this (scale < `GAMUT_WARN`) to fit is out of gamut for
/// the gamut warnings (tolerates rounding at the gamut boundary).
const GAMUT_WARN: f32 = 0.995;

/// For the gamut warnings: `r` needed desaturating by `t` to fit, and isn't simply a neutral
/// beyond white or below black (that's clipping, which the clipping warnings show).
#[inline]
fn out_of_gamut(r: [f32; 3], t: f32) -> bool {
    let (lo, hi) = (r[0].min(r[1]).min(r[2]), r[0].max(r[1]).max(r[2]));
    t < GAMUT_WARN && (hi - lo) > 0.02 * hi.abs().max(1e-3) && hi - lo > 1e-3 && (lo < -1e-3 || (hi > 1.0 && lo < 1.0))
}

#[inline]
fn mul3(m: &[[f32; 3]; 3], d: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * d[0] + m[0][1] * d[1] + m[0][2] * d[2],
        m[1][0] * d[0] + m[1][1] * d[1] + m[1][2] * d[2],
        m[2][0] * d[0] + m[2][1] * d[1] + m[2][2] * d[2],
    ]
}

/// Desaturate linear `r` towards its luminance (weights `luma`) until every channel is in 0..1;
/// returns the mapped colour and the chroma scale used (1 = already in gamut).
#[inline]
pub fn gamut_map(r: [f32; 3], luma: [f32; 3]) -> ([f32; 3], f32) {
    gamut_map_to(r, luma, 1.0)
}

/// [`gamut_map`] into `0..peak` (an HDR render's range; `peak` = 1 is [`gamut_map`]).
#[inline]
pub fn gamut_map_to(r: [f32; 3], luma: [f32; 3], peak: f32) -> ([f32; 3], f32) {
    let yy = (luma[0] * r[0] + luma[1] * r[1] + luma[2] * r[2]).clamp(0.0, peak);
    let mut t = 1.0f32;
    for c in r {
        if c < 0.0 {
            t = t.min(yy / (yy - c).max(1e-9));
        } else if c > peak {
            t = t.min((peak - yy) / (c - yy).max(1e-9));
        }
    }
    if t < 1.0 { (r.map3(|c| yy + (c - yy) * t), t) } else { (r, 1.0) }
}

/// Local Defringe weight of a scene-linear colour `c` whose log luminance differs by `det` from
/// its fine blur: high on strong edges with a purple or green cast.
#[inline]
pub fn defringe_weight(c: [f32; 3], det: f32) -> f32 {
    let y = luminance_2020(c).max(1e-6);
    let purple = (c[0].min(c[2]) - c[1]) / y;
    let green = (c[1] - c[0].max(c[2])) / y;
    smooth(0.04, 0.3, det.abs()) * smooth(0.02, 0.2, purple).max(smooth(0.02, 0.2, green))
}

/// Refine Saturation: scale the curved colour's chroma (around its luma, encoded values) so its
/// saturation (chroma / luma) moves from the curve's towards the pre-curve one: the ratio is
/// `(s_curve / s_before)^refine`, so 1 keeps the curve, 0 restores the original saturation.
#[inline]
pub fn refine_saturation(before: [f32; 3], after: [f32; 3], refine: f32) -> [f32; 3] {
    let luma = |e: [f32; 3]| 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
    let chroma = |e: [f32; 3]| e[0].max(e[1]).max(e[2]) - e[0].min(e[1]).min(e[2]);
    let (y0, y1) = (luma(before), luma(after));
    let s0 = chroma(before) / y0.max(1e-4);
    let s1 = chroma(after) / y1.max(1e-4);
    if s1 <= 1e-6 || s0 <= 1e-6 {
        return after;
    }
    let k = (s0 / s1).powf(1.0 - refine).clamp(0.0, 4.0);
    after.map3(|v| y1 + (v - y1) * k)
}

pub const SRGB_LUT_N: usize = 4096;

/// Linear → sRGB-encoded table (`SRGB_LUT_N + 1` entries over 0..1), interpolated linearly.
pub fn srgb_lut() -> &'static [f32; SRGB_LUT_N + 1] {
    static LUT: std::sync::OnceLock<Box<[f32; SRGB_LUT_N + 1]>> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = Box::new([0.0f32; SRGB_LUT_N + 1]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = linear_to_srgb(i as f32 / SRGB_LUT_N as f32);
        }
        t
    })
}

/// Linear → sRGB-encoded (clamped to 0..1) by an interpolated table: within 2e-5 of the exact
/// curve (≪ one 8-bit or 10-bit step), several times faster than `powf`.
#[inline]
fn encode_srgb(lut: &[f32; SRGB_LUT_N + 1], v: f32) -> f32 {
    let f = v.clamp(0.0, 1.0) * SRGB_LUT_N as f32;
    let i = (f as usize).min(SRGB_LUT_N - 1);
    let t = f - i as f32;
    lut[i] + (lut[i + 1] - lut[i]) * t
}

#[inline]
fn enc(v: f32) -> u8 {
    // `v` is already sRGB-encoded; round to 8 bits.
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lightroom's grain at 100 / 25 / 50 on a 6000 px export (measured on its export of the grain test raw):
    /// noise of unit variance whose neighbouring pixels correlate at 0.52 and whose pixels two apart don't.
    #[test]
    fn grain_has_the_statistics_of_lightroom_grain() {
        let (amt, cell, coarse) = grain_params(1.0, 0.25, 0.5, 6000.0);
        assert!((amt - 1.0).abs() < 1e-6 && (cell - 1.5).abs() < 1e-6 && coarse == 0.0, "{amt} {cell} {coarse}");
        let n = 256;
        let f: Vec<f32> = (0..n * n).map(|i| grain_noise(((i % n) as f32 + 0.5) / cell, ((i / n) as f32 + 0.5) / cell, coarse, 7)).collect();
        let mean = f.iter().sum::<f32>() / f.len() as f32;
        let var = f.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / f.len() as f32;
        let corr = |dx: usize| {
            let (mut s, mut c) = (0.0, 0);
            for y in 0..n {
                for x in 0..n - dx {
                    s += (f[y * n + x] - mean) * (f[y * n + x + dx] - mean);
                    c += 1;
                }
            }
            s / c as f32 / var
        };
        assert!(mean.abs() < 0.05 && (var.sqrt() - 1.0).abs() < 0.05, "mean {mean} std {}", var.sqrt());
        assert!((corr(1) - 0.52).abs() < 0.06, "lag 1: {}", corr(1));
        assert!(corr(2).abs() < 0.08 && corr(3).abs() < 0.1, "lags 2, 3: {} {}", corr(2), corr(3));
        // strongest at mid grey, 0.066 at the ends; a downsized render keeps less of it, a coarser Size more per cell
        assert!((grain_strength(0.5) - 0.084).abs() < 1e-3 && (grain_strength(0.0) - 0.0575).abs() < 1e-6);
        assert!(grain_params(1.0, 0.25, 0.5, 1500.0).0 < 0.6 && grain_params(1.0, 0.25, 0.5, 600.0).0 > 0.0);
        // a hostile coordinate or seed doesn't panic
        let _ = (grain_noise(f32::MAX, f32::NAN, 0.5, u32::MAX), grain_noise(f32::INFINITY, -1e30, 0.0, 0));
    }

    #[test]
    fn refine_saturation_restores_the_pre_curve_saturation() {
        let before = [0.5, 0.3, 0.2];
        let after = [0.7, 0.35, 0.15]; // a contrasty curve: more saturated
        assert_eq!(refine_saturation(before, after, 1.0), after);
        let r = refine_saturation(before, after, 0.0);
        let sat = |e: [f32; 3]| (e[0] - e[2]) / (0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2]);
        assert!((sat(r) - sat(before)).abs() < 1e-4, "{r:?}");
        let half = refine_saturation(before, after, 0.5);
        assert!(sat(half) > sat(before) && sat(half) < sat(after));
        // luma is kept
        let y = |e: [f32; 3]| 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
        assert!((y(r) - y(after)).abs() < 1e-5);
    }

    #[test]
    fn a_lut_sees_srgb_primaries_whatever_the_output() {
        use crate::output::OutputSpace;
        // a table far from the identity: red and blue swapped, green lifted
        let n = 9;
        let mut data = Vec::new();
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let f = |v: usize| v as f32 / (n - 1) as f32;
                    data.push([f(b), f(g) * 0.8 + 0.1, f(r)]);
                }
            }
        }
        let lut = crate::lut::Lut3d { size: n, data, domain_min: [0.0; 3], domain_max: [1.0; 3], title: None };
        assert!(lut_space(&OutputSpace::Srgb.from_working()).is_none(), "sRGB needs no conversion");
        for space in [OutputSpace::DisplayP3, OutputSpace::AdobeRgb, OutputSpace::ProPhoto, OutputSpace::Rec2020] {
            let sp = lut_space(&space.from_working()).unwrap();
            for c in [[0.2f32, 0.5, 0.7], [0.8, 0.3, 0.2], [0.5, 0.5, 0.5], [0.9, 0.7, 0.1], [0.05, 0.4, 0.2]] {
                // the colour as this output holds it, through the lookup, and back in sRGB's numbers
                let held = mul3(&sp.1, c.map3(srgb_to_linear)).map3(linear_to_srgb);
                let out = lut_pixel(&lut, 1.0, held, Some(&sp), false);
                let back = mul3(&sp.0, out.map3(srgb_to_linear)).map3(linear_to_srgb);
                let want = lut.apply(c);
                assert!((0..3).all(|k| (back[k] - want[k]).abs() < 2e-3), "{space:?} {c:?}: {back:?} vs {want:?}");
                // and half the amount is half way, in the same primaries
                let half = lut_pixel(&lut, 0.5, held, Some(&sp), false);
                let half = mul3(&sp.0, half.map3(srgb_to_linear)).map3(linear_to_srgb);
                assert!((0..3).all(|k| (half[k] - (c[k] + want[k]) / 2.0).abs() < 2e-3), "{space:?} {c:?} at half: {half:?}");
            }
        }
    }

    #[test]
    fn srgb_table_matches_the_exact_curve() {
        let lut = srgb_lut();
        let mut worst = 0.0f32;
        for i in 0..=200_000 {
            // dense near black, where the curve bends most
            let v = (i as f32 / 200_000.0).powi(3);
            worst = worst.max((encode_srgb(lut, v) - linear_to_srgb(v)).abs());
        }
        assert!(worst < 2e-5, "{worst}");
        assert_eq!(encode_srgb(lut, -1.0), 0.0);
        assert!((encode_srgb(lut, 2.0) - 1.0).abs() < 1e-6);
    }
}
