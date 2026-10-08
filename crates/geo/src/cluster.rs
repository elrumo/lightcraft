//! Grouping photo positions into map markers: positions that would overlap on screen at the
//! current zoom become one marker with a count.

use std::collections::HashMap;

use crate::geodesy::{World, project, valid};

/// A photo's position on the map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// The caller's id for the photo.
    pub id: u64,
    pub lat: f64,
    pub lon: f64,
    /// Which photo stands for a group: the heaviest (rating, picked…); ties go to the lowest index.
    pub weight: i32,
}

/// One marker.
#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    /// Centre of its photos on the world square.
    pub at: World,
    /// Indices into the points it was made from.
    pub members: Vec<u32>,
    /// The member that stands for the group.
    pub front: u32,
    /// Bounding box of the members: `(min, max)` corners as world points.
    pub bounds: (World, World),
}

impl Cluster {
    pub fn count(&self) -> usize {
        self.members.len()
    }
}

/// Group `points` for a view at `zoom`: photos within about `cell_px` pixels of each other join.
/// Points with impossible positions are left out. The order is stable (left to right, top to bottom).
pub fn cluster(points: &[Point], zoom: f64, cell_px: f64) -> Vec<Cluster> {
    if !zoom.is_finite() || !cell_px.is_finite() || cell_px <= 0.0 {
        return vec![];
    }
    let scale = crate::geodesy::world_pixels(zoom.clamp(0.0, 22.0)) / cell_px;
    let mut cells: HashMap<(i64, i64), Cluster> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        if !valid(p.lat, p.lon) {
            continue;
        }
        let w = project(p.lat, p.lon);
        let key = ((w.x * scale).floor() as i64, (w.y * scale).floor() as i64);
        let i = i as u32;
        match cells.get_mut(&key) {
            None => {
                cells.insert(key, Cluster { at: w, members: vec![i], front: i, bounds: (w, w) });
            }
            Some(c) => {
                let n = c.members.len() as f64;
                c.at = World { x: (c.at.x * n + w.x) / (n + 1.0), y: (c.at.y * n + w.y) / (n + 1.0) };
                c.bounds =
                    (World { x: c.bounds.0.x.min(w.x), y: c.bounds.0.y.min(w.y) }, World { x: c.bounds.1.x.max(w.x), y: c.bounds.1.y.max(w.y) });
                if points.get(i as usize).map(|p| p.weight) > points.get(c.front as usize).map(|p| p.weight) {
                    c.front = i;
                }
                c.members.push(i);
            }
        }
    }
    let mut keyed: Vec<((i64, i64), Cluster)> = cells.into_iter().collect();
    keyed.sort_by_key(|(k, _)| (k.1, k.0));
    keyed.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(id: u64, lat: f64, lon: f64, weight: i32) -> Point {
        Point { id, lat, lon, weight }
    }

    #[test]
    fn nearby_photos_group_and_far_ones_do_not() {
        let pts = [pt(1, 40.4169, -3.7035, 0), pt(2, 40.4170, -3.7036, 3), pt(3, 40.4171, -3.7034, 1), pt(4, 48.8584, 2.2945, 0)];
        let world = cluster(&pts, 3.0, 60.0);
        assert_eq!(world.len(), 2, "Madrid's three are one marker, Paris another");
        let madrid = world.iter().find(|c| c.count() == 3).unwrap();
        assert_eq!(madrid.front, 1, "the best-rated photo stands for the group");
        let street = cluster(&pts, 19.0, 60.0);
        assert!(street.len() >= 3, "{}", street.len());
        assert_eq!(street.iter().map(Cluster::count).sum::<usize>(), 4, "every photo is in exactly one marker");
    }

    #[test]
    fn every_valid_photo_lands_in_one_marker() {
        let pts: Vec<Point> = (0..500).map(|i| pt(i, -60.0 + (i as f64 * 0.37) % 120.0, -170.0 + (i as f64 * 1.9) % 340.0, (i % 5) as i32)).collect();
        for z in [0.0, 2.0, 5.5, 12.0, 19.0] {
            let c = cluster(&pts, z, 56.0);
            assert_eq!(c.iter().map(Cluster::count).sum::<usize>(), 500, "zoom {z}");
        }
        assert!(cluster(&pts, 0.0, 56.0).len() < cluster(&pts, 8.0, 56.0).len(), "zooming in splits markers");
    }

    #[test]
    fn bad_positions_and_parameters() {
        let pts = [pt(1, f64::NAN, 0.0, 0), pt(2, 91.0, 0.0, 0), pt(3, 0.0, 181.0, 0), pt(4, 10.0, 10.0, 0)];
        assert_eq!(cluster(&pts, 4.0, 56.0).len(), 1);
        assert!(cluster(&pts, f64::NAN, 56.0).is_empty());
        assert!(cluster(&pts, 4.0, 0.0).is_empty());
        assert!(cluster(&pts, 4.0, f64::INFINITY).is_empty());
        assert!(cluster(&[], 4.0, 56.0).is_empty());
        let _ = cluster(&pts, 1e9, 56.0);
        let _ = cluster(&pts, -1e9, 56.0);
    }

    #[test]
    fn the_order_is_stable_and_bounds_hold() {
        let pts = [pt(1, 10.0, 10.0, 0), pt(2, 50.0, -100.0, 0), pt(3, 10.0, 10.0001, 0)];
        let a = cluster(&pts, 6.0, 56.0);
        assert_eq!(a, cluster(&pts, 6.0, 56.0));
        for c in &a {
            assert!(c.bounds.0.x <= c.at.x + 1e-12 && c.at.x <= c.bounds.1.x + 1e-12);
            assert!(c.bounds.0.y <= c.at.y + 1e-12 && c.at.y <= c.bounds.1.y + 1e-12);
        }
        assert!(a[0].at.y < a[1].at.y, "north first");
    }
}
