//! Curves for tone mapping: cubic Hermite splines (monotone, Fritsch–Carlson, or natural) and lookup tables.
//!
//! Monotone interpolation never overshoots between control points, so a point curve can't invert
//! tones by accident (a common complaint with natural cubic splines).

use serde::{Deserialize, Serialize};

/// A curve through control points in `0..1 × 0..1` (x strictly increasing after normalization).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MonotoneCurve {
    xs: Vec<f64>,
    ys: Vec<f64>,
    ms: Vec<f64>,
}

impl MonotoneCurve {
    /// Build from points; sorts by x, merges duplicates. Fewer than two points → identity.
    pub fn new(points: &[(f64, f64)]) -> MonotoneCurve {
        let mut p: Vec<(f64, f64)> = points.iter().copied().filter(|(x, y)| x.is_finite() && y.is_finite()).collect();
        p.sort_by(|a, b| a.0.total_cmp(&b.0));
        p.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);
        if p.len() < 2 {
            p = vec![(0.0, 0.0), (1.0, 1.0)];
        }
        let n = p.len();
        let xs: Vec<f64> = p.iter().map(|q| q.0).collect();
        let ys: Vec<f64> = p.iter().map(|q| q.1).collect();
        let d: Vec<f64> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i])).collect();
        let mut ms = vec![0.0; n];
        ms[0] = d[0];
        ms[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            ms[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
        }
        // Fritsch–Carlson limiter where the data is monotone within a segment.
        for i in 0..n - 1 {
            if d[i] == 0.0 {
                ms[i] = 0.0;
                ms[i + 1] = 0.0;
                continue;
            }
            let a = ms[i] / d[i];
            let b = ms[i + 1] / d[i];
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                ms[i] = t * a * d[i];
                ms[i + 1] = t * b * d[i];
            }
        }
        MonotoneCurve { xs, ys, ms }
    }

    /// A tone curve's point curve as Lightroom draws it: the natural cubic spline through the points (second
    /// derivative 0 at both ends), which can overshoot between points where the monotone curve can't. Fitted to
    /// Lightroom's exports with a point curve against the same photo without one, the spline matches to about 1/255
    /// and [`MonotoneCurve::new`] is flatter (`docs/lightroom-preset-fit.md`). Same normalization as `new`.
    pub fn natural(points: &[(f64, f64)]) -> MonotoneCurve {
        let mut c = MonotoneCurve::new(points);
        let (xs, ys) = (&c.xs, &c.ys);
        let n = xs.len();
        if n < 3 {
            return c;
        }
        let h: Vec<f64> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        let d: Vec<f64> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / h[i]).collect();
        // second derivatives at the inner points (tridiagonal, Thomas algorithm); 0 at the ends
        let mut m2 = vec![0.0; n];
        let (mut cp, mut dp) = (vec![0.0; n], vec![0.0; n]);
        for i in 1..n - 1 {
            let (a, b, c, r) = (h[i - 1], 2.0 * (h[i - 1] + h[i]), h[i], 6.0 * (d[i] - d[i - 1]));
            let den = b - a * cp[i - 1];
            cp[i] = c / den;
            dp[i] = (r - a * dp[i - 1]) / den;
        }
        for i in (1..n - 1).rev() {
            m2[i] = dp[i] - cp[i] * m2[i + 1];
        }
        let mut ms: Vec<f64> = (0..n - 1).map(|i| d[i] - h[i] * (2.0 * m2[i] + m2[i + 1]) / 6.0).collect();
        ms.push(d[n - 2] + h[n - 2] * (m2[n - 2] + 2.0 * m2[n - 1]) / 6.0);
        c.ms = ms;
        c
    }

    pub fn identity() -> MonotoneCurve {
        MonotoneCurve::new(&[(0.0, 0.0), (1.0, 1.0)])
    }

    pub fn eval(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0] + self.ms[0] * (x - self.xs[0]).min(0.0) * 0.0;
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        let i = match self.xs.binary_search_by(|v| v.total_cmp(&x)) {
            Ok(i) => return self.ys[i],
            Err(i) => i - 1,
        };
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[i]
            + (t3 - 2.0 * t2 + t) * h * self.ms[i]
            + (-2.0 * t3 + 3.0 * t2) * self.ys[i + 1]
            + (t3 - t2) * h * self.ms[i + 1]
    }

    /// Sample into a LUT of `n` entries over `0..1`.
    pub fn to_lut(&self, n: usize) -> Lut1 {
        Lut1 { v: (0..n).map(|i| self.eval(i as f64 / (n - 1) as f64) as f32).collect() }
    }

    pub fn points(&self) -> Vec<(f64, f64)> {
        self.xs.iter().copied().zip(self.ys.iter().copied()).collect()
    }
}

/// A 1-D lookup table over `0..1` with linear interpolation; inputs outside clamp.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut1 {
    pub v: Vec<f32>,
}

impl Lut1 {
    pub fn identity(n: usize) -> Lut1 {
        Lut1 { v: (0..n).map(|i| i as f32 / (n - 1) as f32).collect() }
    }
    pub fn from_fn(n: usize, f: impl Fn(f32) -> f32) -> Lut1 {
        Lut1 { v: (0..n).map(|i| f(i as f32 / (n - 1) as f32)).collect() }
    }
    #[inline]
    pub fn eval(&self, x: f32) -> f32 {
        let n = self.v.len();
        let f = x.clamp(0.0, 1.0) * (n - 1) as f32;
        let i = (f as usize).min(n - 2);
        let t = f - i as f32;
        self.v[i] + (self.v[i + 1] - self.v[i]) * t
    }
    /// `self ∘ inner` (apply `inner` first).
    pub fn compose(&self, inner: &Lut1) -> Lut1 {
        Lut1 { v: inner.v.iter().map(|&x| self.eval(x)).collect() }
    }
    pub fn is_identity(&self) -> bool {
        let n = self.v.len();
        self.v.iter().enumerate().all(|(i, &y)| (y - i as f32 / (n - 1) as f32).abs() < 1e-6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_spline_is_the_cubic_through_the_points() {
        let pts = [(0.0, 0.0), (0.5, 0.6), (1.0, 1.0)];
        let c = MonotoneCurve::natural(&pts);
        for (x, y) in pts {
            assert!((c.eval(x) - y).abs() < 1e-12);
        }
        // natural end conditions: the second derivative vanishes at both ends
        let e = 1e-4;
        let y2 = |x: f64| (c.eval(x + e) - 2.0 * c.eval(x) + c.eval(x - e)) / (e * e);
        assert!(y2(e).abs() < 0.05 && y2(1.0 - e).abs() < 0.05, "{} {}", y2(e), y2(1.0 - e));
        // continuous second derivative at the inner point
        assert!((y2(0.5 - 2.0 * e) - y2(0.5 + 2.0 * e)).abs() < 0.05);
        // two points stay a straight line
        let line = MonotoneCurve::natural(&[(0.0, 0.1), (1.0, 0.9)]);
        assert!((line.eval(0.5) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn identity_curve() {
        let c = MonotoneCurve::identity();
        for i in 0..=10 {
            let x = i as f64 / 10.0;
            assert!((c.eval(x) - x).abs() < 1e-12);
        }
        assert!(c.to_lut(256).is_identity());
    }

    #[test]
    fn passes_through_points_and_is_monotone() {
        let pts = [(0.0, 0.0), (0.25, 0.15), (0.5, 0.5), (0.75, 0.88), (1.0, 1.0)];
        let c = MonotoneCurve::new(&pts);
        for (x, y) in pts {
            assert!((c.eval(x) - y).abs() < 1e-12);
        }
        let mut prev = -1.0;
        for i in 0..=1000 {
            let y = c.eval(i as f64 / 1000.0);
            assert!(y >= prev - 1e-12);
            prev = y;
        }
    }

    #[test]
    fn no_overshoot_on_flat_segment() {
        let c = MonotoneCurve::new(&[(0.0, 0.0), (0.4, 0.5), (0.6, 0.5), (1.0, 1.0)]);
        for i in 400..=600 {
            let y = c.eval(i as f64 / 1000.0);
            assert!((y - 0.5).abs() < 1e-9, "{y}");
        }
    }

    #[test]
    fn lut_compose() {
        let a = Lut1::from_fn(1024, |x| x * x);
        let b = Lut1::from_fn(1024, |x| x.sqrt());
        let c = a.compose(&b);
        assert!((c.eval(0.3) - 0.3).abs() < 2e-3);
    }

    proptest::proptest! {
        #[test]
        fn monotone_for_increasing_data(ys in proptest::collection::vec(0.0f64..1.0, 3..8)) {
            let mut ys = ys; ys.sort_by(|a, b| a.total_cmp(b));
            let n = ys.len();
            let pts: Vec<(f64, f64)> = ys.iter().enumerate().map(|(i, y)| (i as f64 / (n - 1) as f64, *y)).collect();
            let c = MonotoneCurve::new(&pts);
            let mut prev = f64::MIN;
            for i in 0..=200 { let y = c.eval(i as f64 / 200.0); proptest::prop_assert!(y >= prev - 1e-9); prev = y; }
        }
    }
}
