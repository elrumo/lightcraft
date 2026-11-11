//! The parametric tone curve model shared by the pipeline renderer and the UI curve editor.
//!
//! Lightroom-style tone curve parametric model: four regions (shadows, darks, lights, highlights)
//! separated by three split points. Each region contributes a bump fitted to Lightroom's response, summed
//! and forced monotone.

use lightcraft_color::spline::Lut1;
use lightcraft_develop::ToneCurve;

/// The four shape controls of the parametric curve, in graph order: region 0 is the darkest.
pub const PARAM_AMOUNTS: [&str; 4] = ["curve.shadows", "curve.darks", "curve.lights", "curve.highlights"];

/// Resolution the parametric curve is sampled at, matching the renderer's LUT.
pub const PARAM_LUT: usize = 1024;

/// Each region's response, fitted to Lightroom's exports of two raws (Sony ILCE-6000, Nikon D70) with one slider at a
/// time (±50, ±100) and with moved splits (`docs/lightroom-preset-fit.md`): a smoothstep bump rising from `a` to its peak
/// at `p` and falling to `b`, positions on the split grid (0 = black, 1..3 = the splits, 4 = white, fractions between),
/// lifting `gain` per unit of slider (separate for raising and lowering) times `(width / 0.5)^k`. Lights reaches far
/// below its own window and grows with it; Darks lifts from black.
/// `(a, p, b, k, gain_up, gain_down)`, shadows first.
const REGION_SHAPES: [(f32, f32, f32, f32, f32, f32); 4] = [
    (0.0, 0.5, 2.125, 0.0, 0.112, 0.135),
    (0.0, 0.875, 4.0, 0.0, 0.212, 0.258),
    (0.625, 3.25, 4.0, 1.5, 0.152, 0.110),
    (2.0, 3.375, 4.0, 0.0, 0.149, 0.137),
];

fn smooth01(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How far region `region` at slider `amount` lifts the curve at `x` (0 for a zero amount).
fn region_lift(splits: &[f64; 3], region: usize, amount: f64, x: f64) -> f64 {
    let Some(&(ua, up, ub, k, gain_up, gain_down)) = REGION_SHAPES.get(region) else { return 0.0 };
    if amount == 0.0 || !amount.is_finite() {
        return 0.0;
    }
    let grid = [0.0, splits[0], splits[1], splits[2], 1.0];
    // a position on the split grid: 0 = black, 1..3 = the splits, 4 = white
    let at = |u: f32| {
        let i = (u.max(0.0) as usize).min(3);
        let (lo, hi) = (grid.get(i).copied().unwrap_or(0.0), grid.get(i + 1).copied().unwrap_or(1.0));
        lo + (u as f64 - i as f64) * (hi - lo)
    };
    let (a, p, b) = (at(ua), at(up), at(ub));
    let rise = if x > a && x <= p && p > a {
        smooth01((x - a) / (p - a))
    } else if x > p && x < b && b > p {
        smooth01((b - x) / (b - p))
    } else {
        0.0
    };
    let gain = if amount > 0.0 { gain_up } else { gain_down } as f64 * ((b - a) / 0.5).max(0.0).powf(k as f64);
    amount / 100.0 * gain * rise
}

/// The parametric (region) curve of a [`ToneCurve`]: the four region amounts, each a bump placed on
/// the splits ([`REGION_SHAPES`]), summed and then forced monotone.
///
/// This is the renderer's curve, not an approximation of it — both `curve_luts` in the pipeline
/// and the curve editor in the UI use this struct so they can never disagree.
#[derive(Clone, Debug)]
pub struct ParametricCurve {
    /// The curve, sampled and already forced monotone.
    lut: [f32; PARAM_LUT],
    /// The three region boundaries as fractions of the graph (0..1).
    pub splits: [f64; 3],
    /// The four region amounts, in graph order (0 = shadows, 3 = highlights).
    pub amounts: [f64; 4],
}

impl ParametricCurve {
    pub fn new(c: &ToneCurve) -> ParametricCurve {
        let s0 = if c.split_shadows.is_nan() { 0.25 } else { (c.split_shadows / 100.0).clamp(0.0, 1.0) as f32 };
        let s1 = if c.split_mid.is_nan() { 0.50_f32.max(s0) } else { (c.split_mid / 100.0).clamp(s0 as f64, 1.0) as f32 };
        let s2 = if c.split_highlights.is_nan() { 0.75_f32.max(s1) } else { (c.split_highlights / 100.0).clamp(s1 as f64, 1.0) as f32 };
        let clean_amt = |a: f64| if a.is_nan() { 0.0 } else { a.clamp(-100.0, 100.0) };
        let (a0, a1, a2, a3) = (clean_amt(c.shadows), clean_amt(c.darks), clean_amt(c.lights), clean_amt(c.highlights));
        let splits = [s0 as f64, s1 as f64, s2 as f64];
        let amounts = [a0, a1, a2, a3];
        let mut lut = [0.0_f32; PARAM_LUT];
        let mut prev = 0.0_f32;
        for (i, entry) in lut.iter_mut().enumerate() {
            let x = i as f64 / (PARAM_LUT - 1) as f64;
            let d: f64 = amounts.iter().enumerate().map(|(r, &amt)| region_lift(&splits, r, amt, x)).sum();
            let y = (x + d).clamp(0.0, 1.0) as f32;
            prev = prev.max(y);
            *entry = prev;
        }
        ParametricCurve { lut, splits, amounts }
    }

    /// Convert the evaluated parametric curve into a 1024-entry [`Lut1`].
    pub fn to_lut(&self) -> Lut1 {
        Lut1 { v: self.lut.to_vec() }
    }

    /// The curve at `x` (0 = black, 1 = white), read off the table (so it never overshoots the ends).
    pub fn at(&self, x: f64) -> f64 {
        let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) as f32 };
        let at = x * (PARAM_LUT - 1) as f32;
        let i = at.floor().clamp(0.0, (PARAM_LUT - 1) as f32) as usize;
        let cur = self.lut.get(i).copied().unwrap_or(0.0);
        let next = self.lut.get(i + 1).copied().unwrap_or(cur);
        let f = at - i as f32;
        (cur + (next - cur) * f) as f64
    }

    /// Where region `region`'s handle sits on the input axis: the middle of its own window, so a
    /// handle never has to cross a split to be grabbed.
    pub fn region_x(&self, region: usize) -> f64 {
        let (a, b) = self.region_window(region);
        (a + b) / 2.0
    }

    /// The window (two splits, as fractions of the graph) region `region` owns.
    pub fn region_window(&self, region: usize) -> (f64, f64) {
        let s0 = self.split_x(0);
        let s1 = self.split_x(1);
        let s2 = self.split_x(2);
        match region {
            0 => (0.0, s0),
            1 => (s0, s1),
            2 => (s1, s2),
            _ => (s2, 1.0),
        }
    }

    /// The region that owns graph position `x`: the one whose own window contains it. The four
    /// windows tile 0..1, so this is always defined and a drag anywhere on the curve moves exactly
    /// one slider — the region under the pointer.
    pub fn region_at(&self, x: f64) -> usize {
        let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        let s0 = self.split_x(0);
        let s1 = self.split_x(1);
        let s2 = self.split_x(2);
        if x <= s0 {
            0
        } else if x <= s1 {
            1
        } else if x <= s2 {
            2
        } else {
            3
        }
    }

    /// How much output level region `region`'s bump gains at `x` per unit of its slider, on the side
    /// (raising or lowering) its slider is on now. A drag converts the pointer's vertical motion into a
    /// slider change through it.
    pub fn level_per_unit(&self, region: usize, x: f64) -> f64 {
        let side = if self.amounts.get(region).copied().unwrap_or(0.0) < 0.0 { -1.0 } else { 1.0 };
        region_lift(&self.splits, region, side, x.clamp(0.0, 1.0)) * side
    }

    /// The baseline level at `x` from the identity diagonal plus the contributions of all regions
    /// *except* `region`. Used when inverting the model for `region` while holding the other three
    /// regions fixed.
    pub fn base_level_without(&self, region: usize, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        let others: f64 = (0..4).filter(|&i| i != region).map(|i| region_lift(&self.splits, i, self.amounts.get(i).copied().unwrap_or(0.0), x)).sum();
        x + others
    }

    /// The curve level region `region` sits at when its slider holds `amount`, at graph position `x`,
    /// holding the other regions fixed at their current settings.
    pub fn handle_level_at(&self, region: usize, amount: f64, x: f64) -> f64 {
        (self.base_level_without(region, x) + region_lift(&self.splits, region, amount, x.clamp(0.0, 1.0))).clamp(0.0, 1.0)
    }

    /// The amount that puts region `region`'s curve at `level` at `x`, with the other three
    /// regions held fixed — the inverse of [`Self::handle_level_at`]. A level the region cannot
    /// reach saturates rather than jumping.
    pub fn amount_at_level(&self, region: usize, level: f64, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        let delta = level.clamp(0.0, 1.0) - self.base_level_without(region, x);
        let side = if delta < 0.0 { -1.0 } else { 1.0 };
        let per_unit = region_lift(&self.splits, region, side, x) * side;
        if per_unit.abs() <= f64::EPSILON {
            return self.amounts.get(region).copied().unwrap_or(0.0);
        }
        (delta / per_unit).clamp(-100.0, 100.0).round()
    }

    /// The graph position split `split` (0..=2) sits at, 0..1.
    pub fn split_x(&self, split: usize) -> f64 {
        self.splits.get(split).copied().unwrap_or(0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_or_inverted_splits_do_not_panic_or_poison_curve() {
        let c = ToneCurve {
            split_shadows: 80.0,
            split_mid: 20.0,
            split_highlights: f64::NAN,
            shadows: f64::NAN,
            lights: f64::INFINITY,
            ..Default::default()
        };
        let model = ParametricCurve::new(&c);
        assert!(model.at(0.5).is_finite());
        assert!(model.at(f64::NAN).is_finite());
        assert!(model.region_at(f64::NAN) < 4);
        assert!((model.splits[0] - 0.8).abs() < 1e-6);
        assert!((model.splits[1] - 0.8).abs() < 1e-6);
        assert!((model.splits[2] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn horizontal_inversion_at_x_0_51_with_darks_60_preserves_zero_lights() {
        let c = ToneCurve { darks: 60.0, lights: 0.0, ..Default::default() };
        let model = ParametricCurve::new(&c);
        assert_eq!(model.region_at(0.51), 2, "x=0.51 is inside the Lights region");
        let level = model.at(0.51);
        assert!(level > 0.53, "Darks +60 lifts x = 0.51: {level}");
        // With other regions held fixed, inverting at that level must return Lights = 0, not Darks' lift read as Lights
        let lights = model.amount_at_level(2, level, 0.51);
        assert_eq!(lights, 0.0, "zero vertical delta preserves Lights=0: {lights}");
    }

    #[test]
    fn a_curve_level_and_its_amount_are_inverses_with_other_regions_present() {
        let c = ToneCurve { shadows: 20.0, darks: -30.0, lights: 40.0, highlights: -10.0, ..Default::default() };
        let model = ParametricCurve::new(&c);
        for region in 0..4 {
            let centre = model.region_x(region);
            let (_, upper) = model.region_window(region);
            for x in [centre, centre + (upper - centre) * 0.5] {
                for amount in [-100.0, -40.0, 0.0, 30.0, 100.0] {
                    let level = model.handle_level_at(region, amount, x);
                    // (a level pinned to the bottom or top of the graph can't tell how far past it the slider went)
                    if level <= 0.0 || level >= 1.0 {
                        continue;
                    }
                    let back = model.amount_at_level(region, level, x);
                    assert!((back - amount).abs() <= 1.0, "region {region} at {x}: {amount} -> {level} -> {back}");
                }
            }
        }
    }

    #[test]
    fn regions_lift_where_lightroom_s_do() {
        // Lightroom's exports of an ILCE-6000 and a D70 raw with one slider moved, read as a curve on 0..255 (lift at x)
        let lift = |c: ToneCurve, x: f64| (ParametricCurve::new(&c).at(x) - x) * 255.0;
        let cases = [
            (ToneCurve { shadows: 50.0, ..Default::default() }, 0.15, 14.0),
            (ToneCurve { shadows: -100.0, ..Default::default() }, 0.18, -37.0),
            (ToneCurve { darks: 50.0, ..Default::default() }, 0.40, 23.0),
            (ToneCurve { darks: -50.0, ..Default::default() }, 0.20, -31.0),
            (ToneCurve { lights: 50.0, ..Default::default() }, 0.70, 35.5),
            (ToneCurve { lights: 50.0, split_shadows: 15.0, split_mid: 35.0, split_highlights: 60.0, ..Default::default() }, 0.65, 50.0),
            (ToneCurve { highlights: 50.0, ..Default::default() }, 0.82, 20.0),
            (ToneCurve { highlights: 50.0, ..Default::default() }, 0.40, -1.0),
            (ToneCurve { highlights: 100.0, ..Default::default() }, 0.80, 37.0),
        ];
        for (c, x, want) in cases {
            let got = lift(c.clone(), x);
            assert!((got - want).abs() < 6.0, "{c:?} at {x}: {got:.1} against Lightroom's {want}");
        }
    }
}
