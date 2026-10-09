//! The world's land as a triangle mesh (Natural Earth 110 m "land", public domain), for the map
//! when no tiles are available. `data/land.bin`, written by `cargo xtask geodata`.
//!
//! File layout (after inflating, little-endian): `"LCL1"`, `u32 n` vertices of `i16 lon·100, i16
//! lat·100`, `u32 m` indices (`u16`, three per triangle).

use std::sync::OnceLock;

use crate::codec::{DataError, Reader, inflate};

static EMBEDDED: &[u8] = include_bytes!("../data/land.bin");

/// Land triangles; positions are `(lat, lon)` in degrees.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Land {
    /// `(lat, lon)` of each vertex.
    pub vertices: Vec<(f32, f32)>,
    /// Three indices per triangle into `vertices`; every index is in range.
    pub indices: Vec<u16>,
}

impl Land {
    /// The land built into the app, parsed on first use.
    pub fn global() -> &'static Land {
        static L: OnceLock<Land> = OnceLock::new();
        L.get_or_init(|| Land::from_bytes(EMBEDDED).unwrap_or_default())
    }

    pub fn from_bytes(deflated: &[u8]) -> Result<Land, DataError> {
        let data = inflate(deflated)?;
        let mut r = Reader::new(&data);
        if r.take(4)? != b"LCL1" {
            return Err(DataError("not a land file"));
        }
        let n = r.count(4)?;
        let mut vertices = Vec::with_capacity(n);
        for _ in 0..n {
            let lon = f32::from(r.i16()?) / 100.0;
            let lat = f32::from(r.i16()?) / 100.0;
            vertices.push((lat, lon));
        }
        let m = r.count(2)?;
        if m % 3 != 0 {
            return Err(DataError("not whole triangles"));
        }
        let mut indices = Vec::with_capacity(m);
        for _ in 0..m {
            let i = r.u16()?;
            if usize::from(i) >= n {
                return Err(DataError("index out of range"));
            }
            indices.push(i);
        }
        Ok(Land { vertices, indices })
    }

    pub fn triangles(&self) -> impl Iterator<Item = [(f32, f32); 3]> + '_ {
        self.indices.as_chunks::<3>().0.iter().filter_map(|t| {
            let v = |i: u16| self.vertices.get(usize::from(i)).copied();
            Some([v(t[0])?, v(t[1])?, v(t[2])?])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Even-odd test of whether a position is on land, by brute force over the triangles.
    fn on_land(l: &Land, lat: f32, lon: f32) -> bool {
        l.triangles().any(|[a, b, c]| {
            let s = |p: (f32, f32), q: (f32, f32)| (p.1 - lon) * (q.0 - lat) - (q.1 - lon) * (p.0 - lat);
            let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
            let degenerate = d1 == 0.0 && d2 == 0.0 && d3 == 0.0;
            !degenerate && !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
        })
    }

    #[test]
    fn the_embedded_land_is_the_world() {
        let l = Land::global();
        assert!(l.triangles().count() > 1000, "{}", l.triangles().count());
        assert!(l.triangles().all(|[a, b, c]| (b.0 - a.0) * (c.1 - a.1) != (c.0 - a.0) * (b.1 - a.1)), "no zero-area triangles");
        assert!(on_land(l, 40.4, -3.7), "Madrid is on land");
        assert!(on_land(l, 35.0, 105.0), "China");
        assert!(on_land(l, -25.0, 133.0), "Australia");
        assert!(!on_land(l, 0.0, -140.0), "the Pacific is not");
        assert!(!on_land(l, 30.0, -40.0), "the Atlantic is not");
    }

    #[test]
    fn damaged_files_are_errors() {
        assert!(Land::from_bytes(&[]).is_err());
        assert!(Land::from_bytes(&[0xff; 8]).is_err());
    }
}
