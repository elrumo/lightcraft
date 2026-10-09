//! The map and places: where the visible photos are, and where one photo was taken.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, f64_or};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "map.points", "Photos on the Map", [], None,
            "{zoom?: 0..19 (default 3), cell?: marker size in px (default 56), bbox?: [south, west, north, east], ids?: list the photo ids of markers up to this size (default 25)} → {located, unlocated, markers: [{lat, lon, count, front, ids?, bounds}]} — the visible photos with a GPS position, grouped as the map groups them at that zoom",
            always, |s, p| {
            let zoom = f64_or(p, "zoom", 3.0);
            let cell = f64_or(p, "cell", 56.0);
            if !zoom.is_finite() || !(0.0..=22.0).contains(&zoom) || !cell.is_finite() || cell < 4.0 {
                return Err(bad("map.points", "zoom must be 0..22 and cell at least 4"));
            }
            let bbox = match p.get("bbox") {
                None | Some(Value::Null) => None,
                Some(b) => {
                    let v: Vec<f64> = b.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                    if v.len() != 4 || v.iter().any(|x| !x.is_finite()) {
                        return Err(bad("map.points", "bbox is [south, west, north, east]"));
                    }
                    Some((v[0], v[1], v[2], v[3]))
                }
            };
            let with_ids = p.get("ids").and_then(Value::as_u64).unwrap_or(25) as usize;
            let ids = s.visible_cloned();
            let pts = crate::map::points(&s.catalog, &ids);
            let inside = |lat: f64, lon: f64| match bbox {
                None => true,
                Some((so, w, n, e)) => (so..=n).contains(&lat) && if w <= e { (w..=e).contains(&lon) } else { lon >= w || lon <= e },
            };
            let markers: Vec<Value> = crate::map::markers(&pts, zoom, cell)
                .iter()
                .map(|c| crate::map::cluster_json(c, &pts, with_ids))
                .filter(|m| inside(m["lat"].as_f64().unwrap_or(f64::NAN), m["lon"].as_f64().unwrap_or(f64::NAN)))
                .collect();
            Ok(json!({"located": pts.len(), "unlocated": ids.len() - pts.len(), "markers": markers}))
        }),
        cmd!(query "map.bounds", "Map Bounds of the Photos", [], None,
            "{} → {located, south, west, north, east} — the box around the visible photos that have a GPS position (null when none)",
            always, |s, _| {
            let ids = s.visible_cloned();
            let pts = crate::map::points(&s.catalog, &ids);
            Ok(match crate::map::bounds(&pts) {
                Some(((so, w), (n, e))) => json!({"located": pts.len(), "south": so, "west": w, "north": n, "east": e}),
                None => json!({"located": 0, "south": null, "west": null, "north": null, "east": null}),
            })
        }),
        cmd!(query "map.place", "Find a Place", [], None,
            "{query} → [{label, kind: city|region|country, south, west, north, east}] — what a place name means (offline gazetteer), best match first, with the map box that shows it",
            always, |_, p| {
            let q = super::str_param(p, "query").ok_or_else(|| bad("map.place", "missing `query`"))?;
            let g = lightcraft_geo::Gazetteer::global();
            let mut hits = g.lookup_text(q);
            hits.sort_by_key(|h| std::cmp::Reverse(g.importance(*h)));
            Ok(Value::Array(
                hits.into_iter()
                    .take(12)
                    .filter_map(|h| {
                        let ((so, w), (n, e)) = g.view_of(h)?;
                        let kind = match h.kind {
                            lightcraft_geo::Kind::City => "city",
                            lightcraft_geo::Kind::Region => "region",
                            lightcraft_geo::Kind::Country => "country",
                        };
                        Some(json!({"label": g.label(h)?, "kind": kind, "south": so, "west": w, "north": n, "east": e}))
                    })
                    .collect(),
            ))
        }),
        cmd!(query "photo.place", "Where a Photo Was Taken", [], None,
            "{id?} → {gps: [lat, lon] | null, place: {display, city, region, country, fromGps} | null} — the GPS position and the place it is in (offline gazetteer), else the photo's own place fields",
            always, |s, p| {
            let id = p.get("id").and_then(Value::as_u64).map(lightcraft_catalog::PhotoId).or(s.active()).ok_or_else(|| bad("photo.place", "no photo"))?;
            let ph = s.catalog.photo(id).ok_or_else(|| bad("photo.place", "no such photo"))?;
            Ok(json!({
                "gps": ph.meta.gps.map(|(la, lo)| json!([la, lo])),
                "place": crate::map::place_of(ph).map(|pl| json!({"display": pl.display, "city": pl.city, "region": pl.region, "country": pl.country, "fromGps": pl.from_gps})),
            }))
        }),
    ]
}
