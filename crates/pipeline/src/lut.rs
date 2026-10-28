//! 3D LUT profiles: `.cube` files (the plain-text Cube LUT format: `LUT_3D_SIZE`, optional
//! `DOMAIN_MIN` / `DOMAIN_MAX`, then size³ "r g b" lines with red changing fastest), applied to
//! the display-encoded output colour and blended by the profile amount. Profiles are registered
//! here by id (`lut:…`) and found by the finishing stage. The GPU takes tables up to
//! [`GPU_MAX_SIZE`] points a side (the table travels in the kernel's auxiliary buffer); a bigger one
//! renders on the CPU.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

/// The biggest table (points a side) the GPU renders: 65³ is 3.2 MB of floats.
pub const GPU_MAX_SIZE: usize = 65;

/// A 3D colour lookup table.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut3d {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub title: Option<String>,
}

impl Lut3d {
    /// Parse a `.cube` file.
    pub fn parse_cube(text: &str) -> Result<Lut3d, String> {
        let text = text.trim_start_matches('\u{feff}');
        let (mut size, mut title) = (0usize, None);
        let (mut min, mut max) = ([0.0f32; 3], [1.0f32; 3]);
        let mut data = Vec::new();
        let three = |rest: &str| -> Option<[f32; 3]> {
            let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        };
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            if let Some(r) = l.strip_prefix("TITLE") {
                title = Some(r.trim().trim_matches('"').to_string());
            } else if let Some(r) = l.strip_prefix("LUT_3D_SIZE") {
                size = r.trim().parse().map_err(|_| format!("bad LUT_3D_SIZE `{}`", r.trim()))?;
                if !(2..=256).contains(&size) {
                    return Err(format!("LUT_3D_SIZE {size} out of range"));
                }
            } else if l.starts_with("LUT_1D_SIZE") {
                return Err("1D LUTs are not supported (only 3D .cube files)".into());
            } else if let Some(r) = l.strip_prefix("DOMAIN_MIN") {
                min = three(r).ok_or("bad DOMAIN_MIN")?;
            } else if let Some(r) = l.strip_prefix("DOMAIN_MAX") {
                max = three(r).ok_or("bad DOMAIN_MAX")?;
            } else if l.starts_with(|c: char| c.is_ascii_alphabetic()) {
                // other keywords (LUT_IN_VIDEO_RANGE…) are ignored
            } else {
                data.push(three(l).ok_or_else(|| format!("bad line `{l}`"))?);
            }
        }
        if size == 0 {
            return Err("not a 3D .cube LUT (no LUT_3D_SIZE)".into());
        }
        if data.len() != size * size * size {
            return Err(format!("expected {} entries, found {}", size * size * size, data.len()));
        }
        Ok(Lut3d { size, data, domain_min: min, domain_max: max, title })
    }

    /// Parse an Autodesk / Lustre `.3dl` file: an optional first line of input mesh points (taken as evenly spaced), then
    /// size³ lines of whole-number "r g b" with **blue changing fastest** (the opposite of `.cube`). The output depth
    /// (10, 12 or 16 bits, or 0..1 floats) is read from the largest value in the table.
    pub fn parse_3dl(text: &str) -> Result<Lut3d, String> {
        const SIZE_MAX: usize = 129;
        let text = text.trim_start_matches('\u{feff}');
        let mut mesh: Option<usize> = None;
        let mut rows: Vec<[f32; 3]> = Vec::new();
        for line in text.lines() {
            let l = line.trim();
            // (keywords such as `3DMESH`, `Mesh 4 12` or `gamma 1.0` in the header carry nothing we need)
            if l.is_empty()
                || l.starts_with(['#', '/'])
                || l.starts_with(|c: char| c.is_ascii_alphabetic())
                || l.to_ascii_uppercase().starts_with("3DMESH")
            {
                continue;
            }
            let v: Vec<f32> = l
                .split_whitespace()
                .map(|x| x.parse::<f32>().ok().filter(|f| f.is_finite() && *f >= 0.0).ok_or_else(|| format!("bad line `{l}`")))
                .collect::<Result<_, _>>()?;
            match v.as_slice() {
                [r, g, b] => {
                    if rows.len() >= SIZE_MAX * SIZE_MAX * SIZE_MAX {
                        return Err(format!("a .3dl LUT of more than {SIZE_MAX} points a side is not supported"));
                    }
                    rows.push([*r, *g, *b]);
                }
                // (the mesh line: how many points a side)
                _ if rows.is_empty() && mesh.is_none() && v.len() > 3 => mesh = Some(v.len()),
                _ => return Err(format!("bad line `{l}`")),
            }
        }
        let size = (rows.len() as f64).cbrt().round() as usize;
        if size < 2 || size * size * size != rows.len() {
            return Err(format!("not a .3dl LUT: {} entries are not a cube", rows.len()));
        }
        if mesh.is_some_and(|m| m != size) {
            return Err(format!("not a .3dl LUT: its mesh has {} points and its table {size} a side", mesh.unwrap_or(0)));
        }
        let max = rows.iter().flatten().fold(0.0f32, |a, &b| a.max(b));
        let scale = match max {
            m if m <= 1.0 => 1.0,
            m if m <= 1023.0 => 1023.0,
            m if m <= 4095.0 => 4095.0,
            m if m <= 65535.0 => 65535.0,
            m => return Err(format!("a .3dl value of {m} is past 16 bits")),
        };
        let mut data = vec![[0.0f32; 3]; rows.len()];
        for r in 0..size {
            for g in 0..size {
                for b in 0..size {
                    if let (Some(dst), Some(src)) = (data.get_mut(r + size * (g + size * b)), rows.get(b + size * (g + size * r))) {
                        *dst = [src[0] / scale, src[1] / scale, src[2] / scale];
                    }
                }
            }
        }
        Ok(Lut3d { size, data, domain_min: [0.0; 3], domain_max: [1.0; 3], title: None })
    }

    /// A Hald CLUT image's pixels (`pixels`, row by row, each 0..1) as a table. The image is `level³` pixels a side and holds
    /// `level²` points a side, red changing fastest from the first pixel, as `.cube` does.
    pub fn from_hald(pixels: &[[f32; 3]], width: usize) -> Result<Lut3d, String> {
        let level = (width as f64).cbrt().round() as usize;
        let size = level * level;
        if level < 2 || level * level * level != width || pixels.len() != width.saturating_mul(width) {
            return Err(format!("not a Hald CLUT image: its side must be a cube (64, 125, 216, 343, 512…), not {width}"));
        }
        Ok(Lut3d { size, data: pixels.to_vec(), domain_min: [0.0; 3], domain_max: [1.0; 3], title: None })
    }

    /// The same table sampled at `size` points a side (trilinear): a smaller file for a table that is finer than it needs.
    pub fn resampled(&self, size: usize) -> Lut3d {
        let size = size.clamp(2, 256);
        let mut data = Vec::with_capacity(size * size * size);
        let at = |i: usize, k: usize| self.domain_min[k] + (self.domain_max[k] - self.domain_min[k]) * (i as f32 / (size - 1) as f32);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push(self.apply([at(r, 0), at(g, 1), at(b, 2)]));
                }
            }
        }
        Lut3d { size, data, ..self.clone() }
    }

    /// The table as `.cube` text (6 decimals).
    pub fn to_cube(&self) -> String {
        use std::fmt::Write;
        let mut s = String::with_capacity(64 + self.data.len() * 27);
        if let Some(t) = self.title.as_deref().map(|t| t.replace(['"', '\n', '\r'], " ")).filter(|t| !t.trim().is_empty()) {
            let _ = writeln!(s, "TITLE \"{}\"", t.trim());
        }
        let _ = writeln!(s, "LUT_3D_SIZE {}", self.size);
        if self.domain_min != [0.0; 3] || self.domain_max != [1.0; 3] {
            let _ = writeln!(s, "DOMAIN_MIN {} {} {}", self.domain_min[0], self.domain_min[1], self.domain_min[2]);
            let _ = writeln!(s, "DOMAIN_MAX {} {} {}", self.domain_max[0], self.domain_max[1], self.domain_max[2]);
        }
        for c in &self.data {
            let _ = writeln!(s, "{:.6} {:.6} {:.6}", c[0], c[1], c[2]);
        }
        s
    }

    /// Look up `c` (trilinear).
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let s = (n - 1) as f32;
        let mut idx = [0usize; 3];
        let mut frac = [0.0f32; 3];
        for k in 0..3 {
            let span = (self.domain_max[k] - self.domain_min[k]).max(1e-6);
            let x = (((c[k] - self.domain_min[k]) / span).clamp(0.0, 1.0)) * s;
            let i = (x.floor() as usize).min(n - 2);
            idx[k] = i;
            frac[k] = x - i as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.data[r + n * (g + n * b)];
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let (r, g, b) = (idx[0], idx[1], idx[2]);
        let c00 = lerp(at(r, g, b), at(r + 1, g, b), frac[0]);
        let c10 = lerp(at(r, g + 1, b), at(r + 1, g + 1, b), frac[0]);
        let c01 = lerp(at(r, g, b + 1), at(r + 1, g, b + 1), frac[0]);
        let c11 = lerp(at(r, g + 1, b + 1), at(r + 1, g + 1, b + 1), frac[0]);
        lerp(lerp(c00, c10, frac[1]), lerp(c01, c11, frac[1]), frac[2])
    }
}

/// What is registered under a profile id: a table, or the file one is read from the first time a render asks.
enum Entry {
    Ready(Arc<Lut3d>),
    File(PathBuf),
    /// A file that was read and is not a table: no LUT, and not read again on every render.
    Failed,
}

fn registry() -> &'static RwLock<HashMap<String, Entry>> {
    static R: OnceLock<RwLock<HashMap<String, Entry>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Make a LUT available to renders under profile id `id` (`lut:…`).
pub fn register(id: &str, lut: Lut3d) {
    registry().write().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), Entry::Ready(Arc::new(lut)));
}

/// Make the `.cube` file at `path` available under `id`, read when a render first needs it: an app with a pack of
/// big tables doesn't parse them all at every start.
pub fn register_file(id: &str, path: impl Into<PathBuf>) {
    registry().write().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), Entry::File(path.into()));
}

/// The LUT registered for profile `id`.
pub fn get(id: &str) -> Option<Arc<Lut3d>> {
    if !id.starts_with("lut:") {
        return None;
    }
    let path = match registry().read().unwrap_or_else(|e| e.into_inner()).get(id)? {
        Entry::Ready(l) => return Some(l.clone()),
        Entry::Failed => return None,
        Entry::File(p) => p.clone(),
    };
    // (read outside the lock: two renders asking at once may both read it, which only costs time)
    let Ok(text) = std::fs::read_to_string(&path) else {
        // (a file that can't be read now, on a volume not mounted yet or half copied, is asked for again next time)
        return None;
    };
    let lut = Lut3d::parse_cube(&text).ok().map(Arc::new);
    let mut w = registry().write().unwrap_or_else(|e| e.into_inner());
    if matches!(w.get(id), Some(Entry::File(p)) if *p == path) {
        w.insert(id.to_string(), lut.clone().map_or(Entry::Failed, Entry::Ready));
    }
    lut
}

/// The size and title a `.cube` file's first lines (`head`) declare, without reading its table.
pub fn cube_header(head: &str) -> Result<(usize, Option<String>), String> {
    let mut title = None;
    for line in head.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("TITLE") {
            title = Some(r.trim().trim_matches('"').to_string());
        } else if let Some(r) = l.strip_prefix("LUT_3D_SIZE") {
            let n: usize = r.trim().parse().map_err(|_| format!("bad LUT_3D_SIZE `{}`", r.trim()))?;
            return if (2..=256).contains(&n) { Ok((n, title)) } else { Err(format!("LUT_3D_SIZE {n} out of range")) };
        } else if l.starts_with("LUT_1D_SIZE") {
            return Err("1D LUTs are not supported (only 3D .cube files)".into());
        }
    }
    Err("not a 3D .cube LUT (no LUT_3D_SIZE)".into())
}

/// Forget a LUT profile.
pub fn unregister(id: &str) {
    registry().write().unwrap_or_else(|e| e.into_inner()).remove(id);
}

/// Profile ids that name a LUT (`lut:…`).
pub fn is_lut_profile(id: &str) -> bool {
    id.starts_with("lut:")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(f: impl Fn([f32; 3]) -> [f32; 3], n: usize) -> String {
        let mut s = format!("TITLE \"test\"\nLUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let c = f([r as f32 / (n - 1) as f32, g as f32 / (n - 1) as f32, b as f32 / (n - 1) as f32]);
                    s.push_str(&format!("{} {} {}\n", c[0], c[1], c[2]));
                }
            }
        }
        s
    }

    #[test]
    fn a_registered_file_is_read_when_first_asked_for() {
        let dir = std::env::temp_dir().join(format!("lc-lut-lazy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.cube"), cube(|c| [c[2], c[1], c[0]], 5)).unwrap();
        std::fs::write(dir.join("bad.cube"), "LUT_3D_SIZE 4\n0 0 0\n").unwrap();
        register_file("lut:lazy-test", dir.join("a.cube"));
        register_file("lut:lazy-bad", dir.join("bad.cube"));
        register_file("lut:lazy-gone", dir.join("gone.cube"));
        let l = get("lut:lazy-test").unwrap();
        assert!((l.apply([0.1, 0.5, 0.8])[0] - 0.8).abs() < 1e-5);
        std::fs::remove_file(dir.join("a.cube")).unwrap();
        assert!(get("lut:lazy-test").is_some(), "read once");
        assert!(get("lut:lazy-bad").is_none() && get("lut:lazy-gone").is_none());
        assert!(get("lut:lazy-nothing").is_none());
        for id in ["lut:lazy-test", "lut:lazy-bad", "lut:lazy-gone"] {
            unregister(id);
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(cube_header("TITLE \"Look\"\nLUT_3D_SIZE 33\n0 0 0").unwrap(), (33, Some("Look".to_string())));
        assert!(cube_header("LUT_1D_SIZE 4").is_err() && cube_header("0 0 0").is_err() && cube_header("LUT_3D_SIZE 999").is_err());
    }

    #[test]
    fn identity_and_swap() {
        let id = Lut3d::parse_cube(&cube(|c| c, 17)).unwrap();
        assert_eq!(id.title.as_deref(), Some("test"));
        let v = id.apply([0.2, 0.55, 0.9]);
        assert!(v.iter().zip([0.2, 0.55, 0.9]).all(|(a, b)| (a - b).abs() < 1e-5), "{v:?}");
        let swap = Lut3d::parse_cube(&cube(|c| [c[2], c[1], c[0]], 9)).unwrap();
        let v = swap.apply([0.1, 0.5, 0.8]);
        assert!((v[0] - 0.8).abs() < 1e-5 && (v[2] - 0.1).abs() < 1e-5, "{v:?}");
        assert!(Lut3d::parse_cube("LUT_1D_SIZE 4\n0 0 0").is_err());
        assert!(Lut3d::parse_cube("LUT_3D_SIZE 2\n0 0 0\n").is_err(), "too few entries");
    }

    /// A `.3dl` of `n` points a side with the values `f` gives, blue changing fastest, at `max` for white.
    fn three_dl(f: impl Fn([f32; 3]) -> [f32; 3], n: usize, max: f32, mesh: bool) -> String {
        let mut s = String::new();
        if mesh {
            s.push_str(&(0..n).map(|i| format!("{}", (i as f32 * 1023.0 / (n - 1) as f32).round())).collect::<Vec<_>>().join(" "));
            s.push('\n');
        }
        for r in 0..n {
            for g in 0..n {
                for b in 0..n {
                    let c = f([r as f32 / (n - 1) as f32, g as f32 / (n - 1) as f32, b as f32 / (n - 1) as f32]);
                    s.push_str(&format!("{} {} {}\n", (c[0] * max).round(), (c[1] * max).round(), (c[2] * max).round()));
                }
            }
        }
        s
    }

    fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
    }

    #[test]
    fn a_3dl_has_blue_fastest_and_its_depth_is_read_from_the_table() {
        let probe = [0.1, 0.5, 0.8];
        // 10 bits with a mesh line, 12 bits without one, and a file with a header keyword
        for (max, mesh) in [(1023.0, true), (4095.0, false)] {
            let swap = Lut3d::parse_3dl(&three_dl(|c| [c[2], c[1], c[0]], 9, max, mesh)).unwrap();
            assert_eq!(swap.size, 9);
            assert!(close(swap.apply(probe), [0.8, 0.5, 0.1], 2e-3), "{max}: {:?}", swap.apply(probe));
        }
        let text = format!("3DMESH\nMesh 3 10\n{}", three_dl(|c| [c[0] * 0.5, c[1], c[2]], 5, 1023.0, true));
        assert!(close(Lut3d::parse_3dl(&text).unwrap().apply([1.0, 1.0, 1.0]), [0.5, 1.0, 1.0], 2e-3));
        // 16 bits, and the colours a table never reaches the top of
        assert!(close(Lut3d::parse_3dl(&three_dl(|c| c, 5, 65535.0, false)).unwrap().apply([1.0, 0.0, 0.5]), [1.0, 0.0, 0.5], 1e-3));
    }

    #[test]
    fn a_3dl_that_is_not_a_cube_is_refused() {
        assert!(Lut3d::parse_3dl("").is_err());
        assert!(Lut3d::parse_3dl("0 0 0\n1 1 1\n").is_err(), "two entries");
        assert!(Lut3d::parse_3dl(&three_dl(|c| c, 4, 1023.0, true).replace("1023 1023 1023", "1023 x 1023")).is_err());
        // a mesh that says 5 points over a table of 4
        let wrong = format!("0 256 512 768 1023\n{}", three_dl(|c| c, 4, 1023.0, false));
        assert!(Lut3d::parse_3dl(&wrong).is_err());
        assert!(Lut3d::parse_3dl("0 0 0\n99999999 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n").is_err(), "past 16 bits");
    }

    #[test]
    fn a_hald_image_is_a_table_and_a_big_one_can_be_made_smaller() {
        // level 3: 27 px a side, 9 points a side, red changing fastest
        let n = 9;
        let px: Vec<[f32; 3]> = (0..n * n * n)
            .map(|i| {
                let (r, g, b) = (i % n, (i / n) % n, i / (n * n));
                [b as f32 / 8.0, g as f32 / 8.0, r as f32 / 8.0]
            })
            .collect();
        let swap = Lut3d::from_hald(&px, 27).unwrap();
        assert_eq!(swap.size, 9);
        assert!(close(swap.apply([0.1, 0.5, 0.8]), [0.8, 0.5, 0.1], 1e-5));
        for bad in [26usize, 64] {
            assert!(Lut3d::from_hald(&px, bad).is_err(), "{bad}");
        }
        let small = swap.resampled(5);
        assert_eq!(small.size, 5);
        assert!(close(small.apply([0.25, 0.5, 1.0]), [1.0, 0.5, 0.25], 1e-5));
    }

    #[test]
    fn a_table_written_as_cube_reads_back() {
        let mut lut = Lut3d::parse_cube(&cube(|c| [c[1] * 0.9, c[2], c[0] * 0.3 + 0.1], 7)).unwrap();
        lut.title = Some("a \"quoted\"\nname".into());
        let back = Lut3d::parse_cube(&lut.to_cube()).unwrap();
        assert_eq!((back.size, back.title.as_deref()), (7, Some("a  quoted  name")));
        assert!(close(back.apply([0.3, 0.6, 0.9]), lut.apply([0.3, 0.6, 0.9]), 2e-6));
        lut.domain_max = [2.0; 3];
        assert_eq!(Lut3d::parse_cube(&lut.to_cube()).unwrap().domain_max, [2.0; 3]);
    }
}
