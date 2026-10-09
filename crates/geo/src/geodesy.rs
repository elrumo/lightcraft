//! Distances on the globe and Web Mercator projection (the maths behind map tiles).

/// Mean Earth radius, kilometres.
pub const EARTH_RADIUS_KM: f64 = 6371.0088;

/// Latitude limit of Web Mercator (the square world map ends here).
pub const MAX_MERCATOR_LAT: f64 = 85.051_128_779_806_59;

/// Whether `(lat, lon)` is a real position in degrees.
pub fn valid(lat: f64, lon: f64) -> bool {
    lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)
}

/// Great-circle distance in kilometres (haversine).
pub fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * a.sqrt().clamp(0.0, 1.0).asin()
}

/// A point on the unit-width world square: `x` 0 at 180°W to 1 at 180°E, `y` 0 at the north limit
/// to 1 at the south limit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct World {
    pub x: f64,
    pub y: f64,
}

/// Project a position onto the world square (latitude is clamped to the Mercator limit).
pub fn project(lat: f64, lon: f64) -> World {
    let lat = lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT).to_radians();
    World { x: (lon + 180.0) / 360.0, y: (1.0 - lat.tan().asinh() / std::f64::consts::PI) / 2.0 }
}

/// The position (`lat`, `lon`) of a world-square point; `x` outside 0..1 wraps around the globe.
pub fn unproject(w: World) -> (f64, f64) {
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * w.y.clamp(0.0, 1.0))).sinh().atan().to_degrees();
    let lon = (w.x - w.x.floor()) * 360.0 - 180.0;
    (lat, lon)
}

/// Size in pixels of the world square at a (possibly fractional) zoom level, for 256-pixel tiles.
pub fn world_pixels(zoom: f64) -> f64 {
    256.0 * zoom.exp2()
}

/// The smallest zoom at which a box of `lat`/`lon` extents fits in `width` × `height` pixels.
pub fn zoom_to_fit(min: (f64, f64), max: (f64, f64), width: f64, height: f64) -> f64 {
    let (a, b) = (project(max.0, min.1), project(min.0, max.1));
    let (dx, dy) = ((b.x - a.x).abs().max(1e-9), (b.y - a.y).abs().max(1e-9));
    let z = (width / (256.0 * dx)).log2().min((height / (256.0 * dy)).log2());
    if z.is_finite() { z.clamp(0.0, 19.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn madrid_to_london() {
        let d = distance_km(40.4168, -3.7038, 51.5074, -0.1278);
        assert!((d - 1264.0).abs() < 8.0, "{d}");
        assert_eq!(distance_km(10.0, 20.0, 10.0, 20.0), 0.0);
    }

    #[test]
    fn antipodes_are_half_a_circumference() {
        let d = distance_km(0.0, 0.0, 0.0, 180.0);
        assert!((d - std::f64::consts::PI * EARTH_RADIUS_KM).abs() < 1e-6);
    }

    #[test]
    fn projection_round_trips() {
        for (lat, lon) in [(0.0, 0.0), (40.4, -3.7), (-33.9, 151.2), (84.0, 179.0), (-84.0, -179.0)] {
            let (la, lo) = unproject(project(lat, lon));
            assert!((la - lat).abs() < 1e-9 && (lo - lon).abs() < 1e-9, "{lat},{lon} -> {la},{lo}");
        }
        let c = project(0.0, 0.0);
        assert!((c.x - 0.5).abs() < 1e-12 && (c.y - 0.5).abs() < 1e-12);
        assert!(project(90.0, 0.0).y >= 0.0, "clamped to the Mercator limit");
    }

    #[test]
    fn validity() {
        assert!(valid(0.0, 0.0) && valid(-90.0, 180.0));
        assert!(!valid(91.0, 0.0) && !valid(0.0, f64::NAN) && !valid(f64::INFINITY, 0.0));
    }

    #[test]
    fn zoom_fit() {
        let z = zoom_to_fit((-85.0, -180.0), (85.0, 180.0), 1024.0, 1024.0);
        assert!((z - 2.0).abs() < 0.01, "{z}");
        assert_eq!(zoom_to_fit((1.0, 1.0), (1.0, 1.0), 800.0, 600.0), 19.0);
    }
}
