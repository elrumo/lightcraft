//! The map: where the visible photos were taken, grouped into markers, and a photo's place in words.
//!
//! The UI draws the markers itself (it keeps the viewport); this module is what both it and the
//! `map.*` commands (CLI, MCP, control channel) ask: *which photos have a position*, *how do they
//! group at this zoom*, *where is this photo*, *what route do the photos suggest*.

use lightcraft_catalog::{Catalog, Flag, Photo, PhotoId};
use lightcraft_geo::cluster::{Cluster, Point, cluster};
use lightcraft_geo::geodesy::unproject;
use lightcraft_geo::route::{Fix, Routes};
use lightcraft_geo::{Gazetteer, PlaceName};
use lightcraft_meta::DateTime;
use serde_json::{Value, json};

/// Which photo stands for a group of photos on one marker: the best rated, picks first.
pub fn weight(p: &Photo) -> i32 {
    i32::from(p.rating) * 2
        + match p.flag {
            Flag::Pick => 1,
            Flag::Reject => -20,
            Flag::None => 0,
        }
}

/// The positions of those of `ids` that have one. The point's `id` is the photo's id.
pub fn points(cat: &Catalog, ids: &[PhotoId]) -> Vec<Point> {
    ids.iter()
        .filter_map(|id| {
            let p = cat.photo(*id)?;
            let (lat, lon) = p.meta.gps?;
            Some(Point { id: id.0, lat, lon, weight: weight(p) })
        })
        .collect()
}

/// The box around `points`: `((south, west), (north, east))`, `None` for no points.
pub fn bounds(points: &[Point]) -> Option<((f64, f64), (f64, f64))> {
    points.iter().filter(|p| lightcraft_geo::geodesy::valid(p.lat, p.lon)).fold(None, |b, p| {
        Some(match b {
            None => ((p.lat, p.lon), (p.lat, p.lon)),
            Some(((s, w), (n, e))) => ((s.min(p.lat), w.min(p.lon)), (n.max(p.lat), e.max(p.lon))),
        })
    })
}

/// Where a photo was taken, in words.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceInfo {
    /// "Madrid, Spain".
    pub display: String,
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    /// Worked out from the GPS position (rather than read from the photo's place fields).
    pub from_gps: bool,
}

/// A photo's place: its GPS position looked up in the gazetteer, else the place fields written in
/// the file or by hand (city, state, country), `None` when it has neither.
pub fn place_of(p: &Photo) -> Option<PlaceInfo> {
    if let Some((lat, lon)) = p.meta.gps
        && let Some(n) = Gazetteer::global().name_of(lat, lon)
    {
        return Some(from_name(&n));
    }
    let m = &p.meta;
    let given: Vec<&str> = [&m.location, &m.city, &m.state, &m.country].into_iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if given.is_empty() {
        return None;
    }
    let opt = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    Some(PlaceInfo { display: given.join(", "), city: opt(&m.city), region: opt(&m.state), country: opt(&m.country), from_gps: false })
}

fn from_name(n: &PlaceName<'_>) -> PlaceInfo {
    PlaceInfo {
        display: n.display(),
        city: n.city.map(str::to_string),
        region: n.region.map(str::to_string),
        country: n.country.map(str::to_string),
        from_gps: true,
    }
}

/// A marker as JSON for `map.points`.
pub fn cluster_json(c: &Cluster, pts: &[Point], with_ids: usize) -> Value {
    let (lat, lon) = unproject(c.at);
    let front = pts.get(c.front as usize).map(|p| p.id);
    // up to four of its photos, for a marker that pictures them
    let mut v = json!({"lat": lat, "lon": lon, "count": c.count(), "front": front, "pictured": c.pictured(pts, 4)});
    if c.count() <= with_ids {
        v["ids"] = json!(c.members.iter().filter_map(|i| pts.get(*i as usize).map(|p| p.id)).collect::<Vec<_>>());
    }
    let (a, b) = (unproject(c.bounds.0), unproject(c.bounds.1));
    v["bounds"] = json!({"south": a.0.min(b.0), "west": a.1.min(b.1), "north": a.0.max(b.0), "east": a.1.max(b.1)});
    v
}

/// Group `points` for a map at `zoom` with markers about `cell_px` pixels wide.
pub fn markers(points: &[Point], zoom: f64, cell_px: f64) -> Vec<Cluster> {
    cluster(points, zoom, cell_px)
}

/// The photos of a route estimate: where and when, and what was left out.
#[derive(Debug, Default)]
pub struct RouteFixes {
    pub fixes: Vec<Fix>,
    /// Ids that are no photo of this library.
    pub unknown: usize,
    /// Photos without a GPS position.
    pub no_gps: usize,
    /// Photos with a position and no capture time, or only a date.
    pub no_time: usize,
    /// Fixes whose capture time had no zone, so one was taken from the longitude.
    pub zone_estimated: usize,
}

/// The position and the moment of each of `ids` (a photo listed twice counts once). A capture time
/// is the camera's clock: with a recorded offset it is exact, without one the zone of the
/// photo's longitude (to the hour) stands in, which orders and paces photos across zones well
/// enough for a route that is an estimate anyway.
pub fn route_fixes(cat: &Catalog, ids: &[PhotoId]) -> RouteFixes {
    let mut out = RouteFixes::default();
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if !seen.insert(id.0) {
            continue;
        }
        let Some(p) = cat.photo(*id) else {
            out.unknown += 1;
            continue;
        };
        let Some((lat, lon)) = p.meta.gps else {
            out.no_gps += 1;
            continue;
        };
        // a date alone is too coarse to place a photo between two others
        let Some(dt) = p.captured.as_deref().filter(|c| c.contains('T')).and_then(DateTime::parse_iso) else {
            out.no_time += 1;
            continue;
        };
        let mut time = dt.unix_seconds() as f64 + f64::from(dt.millis) / 1000.0;
        if dt.offset_minutes.is_none() {
            time -= (lon / 15.0).round().clamp(-12.0, 14.0) * 3600.0;
            out.zone_estimated += 1;
        }
        out.fixes.push(Fix { id: id.0, time, lat, lon });
    }
    out
}

/// A moment as the `…Z` text the other commands use.
fn iso(t: f64) -> String {
    format!("{}Z", lightcraft_catalog::dates::civil(t.floor() as i64))
}

/// The answer of `map.route`: the routes with their stops, legs and lines.
pub fn route_json(found: &RouteFixes, routes: &Routes) -> Value {
    let routes_json: Vec<Value> = routes
        .routes
        .iter()
        .map(|r| {
            let stops: Vec<Value> = r
                .stops
                .iter()
                .map(|s| json!({"lat": s.lat, "lon": s.lon, "arrive": s.arrive, "leave": s.leave, "count": s.photos.len(), "photos": s.photos}))
                .collect();
            let legs: Vec<Value> = r
                .legs
                .iter()
                .map(|l| {
                    json!({
                        "from": l.from, "to": l.to,
                        "distanceKm": l.distance_km, "durationS": l.duration_s, "speedKmh": l.speed_kmh,
                        "mode": l.mode.as_str(), "arc": l.arc,
                        "lines": l.lines.iter().map(|line| line.iter().map(|(la, lo)| json!([la, lo])).collect::<Vec<_>>()).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({
                "start": r.start(), "end": r.end(), "startIso": iso(r.start()), "endIso": iso(r.end()),
                "distanceKm": r.distance_km(), "stops": stops, "legs": legs,
            })
        })
        .collect();
    json!({
        "estimated": true,
        "photos": found.fixes.len().saturating_sub(routes.skipped),
        "zoneEstimated": found.zone_estimated,
        "skipped": {"noGps": found.no_gps, "noTime": found.no_time, "unknown": found.unknown, "invalid": routes.skipped},
        "routes": routes_json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: u64, gps: Option<(f64, f64)>) -> Photo {
        let mut p = Photo::new(PhotoId(id), lightcraft_catalog::Source::Demo { scene: 0 }, "a.jpg", "JPEG", 100, 100, "2026-01-01T00:00:00");
        p.meta.gps = gps;
        p
    }

    #[test]
    fn a_photos_place_comes_from_its_gps_or_its_fields() {
        let p = photo(1, Some((40.4169, -3.7035)));
        let place = place_of(&p).unwrap();
        assert!(place.from_gps && place.display.ends_with("Spain") && place.city.as_deref() == Some("Madrid"), "{place:?}");
        let mut q = photo(2, None);
        assert!(place_of(&q).is_none());
        q.meta.city = "Lisboa".into();
        q.meta.country = "Portugal".into();
        let place = place_of(&q).unwrap();
        assert_eq!((place.display.as_str(), place.from_gps), ("Lisboa, Portugal", false));
        // open sea: the position is known, the place is not; fall back to what the file says
        let mut sea = photo(3, Some((0.0, -140.0)));
        assert!(place_of(&sea).is_none());
        sea.meta.country = "Pacific".into();
        assert_eq!(place_of(&sea).unwrap().display, "Pacific");
    }

    #[test]
    fn heavier_photos_front_markers() {
        let mut a = photo(1, None);
        let mut b = photo(2, None);
        a.rating = 5;
        b.flag = Flag::Pick;
        b.rating = 2;
        assert!(weight(&a) > weight(&b));
        b.flag = Flag::Reject;
        assert!(weight(&b) < weight(&photo(3, None)));
    }

    #[test]
    fn bounds_of_points() {
        assert_eq!(bounds(&[]), None);
        let pts = [
            Point { id: 1, lat: 10.0, lon: 20.0, weight: 0 },
            Point { id: 2, lat: -5.0, lon: 30.0, weight: 0 },
            Point { id: 3, lat: f64::NAN, lon: 0.0, weight: 0 },
        ];
        assert_eq!(bounds(&pts), Some(((-5.0, 20.0), (10.0, 30.0))));
    }
}
