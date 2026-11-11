//! Film effects (`docs/film-effects.md`): the stack of film marks over a photo (`film.layers` in its develop settings).
//! The frame and the switches are plain settings (`develop.merge {settings: {film: {frame, frame_tone, border, markings,
//! vary}}}`), each layer's sliders indexed controls (`filmLayer.<i>.<field>` with `develop.set`). Each command here is one
//! undo step.

use std::sync::atomic::{AtomicU64, Ordering};

use lightcraft_develop::{Film, FilmKind, FilmLayer, MAX_FILM_LAYERS};
use serde_json::{Value, json};

use super::{CommandSpec, bad, bool_or, cmd, has_active};
use crate::{Result, Session};

/// A seed no other layer made now is likely to have: the clock and a counter, mixed (splitmix64).
fn fresh_seed() -> u32 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mut z = t ^ COUNTER.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (z ^ (z >> 31)) as u32
}

/// The `seed` a caller gave (an agent or a test that wants a known pattern), else a fresh one.
fn seed_param(p: &Value, c: &str) -> Result<u32> {
    match p.get("seed").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().and_then(|v| u32::try_from(v).ok()).ok_or_else(|| bad(c, "`seed` must be a non-negative integer that fits in u32")),
        None => Ok(fresh_seed()),
    }
}

fn layer_id(p: &Value, c: &str) -> Result<u32> {
    p.get("id").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).ok_or_else(|| bad(c, "`id` must be a film effect's id"))
}

fn find(film: &Film, id: u32, c: &str) -> Result<usize> {
    film.layers.iter().position(|l| l.id == id).ok_or_else(|| bad(c, format!("no film effect {id}")))
}

/// Edit the active photo's film effects as one undo step named `label`. `f` answers the layer to show selected, if any.
fn film_edit(s: &mut Session, c: &str, label: &str, f: impl FnOnce(&mut Film) -> Result<Option<u32>>) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    let selected = f(&mut d.film)?;
    let index = selected.and_then(|sel| d.film.layers.iter().position(|l| l.id == sel));
    s.set_develop(id, d, label)?;
    Ok(json!({"id": selected, "index": index}))
}

fn room(film: &Film, c: &str) -> Result<()> {
    if film.layers.len() >= MAX_FILM_LAYERS { Err(bad(c, format!("a photo holds at most {MAX_FILM_LAYERS} film effects"))) } else { Ok(()) }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "film.add",
            "Add Film Effect",
            [],
            None,
            "{kind: leak|streak|burn|dust|scratches|negative, seed?: u32} — a new layer on top of the stack with the kind's starting values; answers {id, index}",
            has_active,
            |s, p| {
                let c = "film.add";
                let kind: FilmKind = serde_json::from_value(p.get("kind").cloned().ok_or_else(|| bad(c, "missing `kind`"))?)
                    .map_err(|_| bad(c, "`kind` must be leak, streak, burn, dust, scratches or negative"))?;
                let seed = seed_param(p, c)?;
                film_edit(s, c, "Add Film Effect", |film| {
                    room(film, c)?;
                    let id = film.next_id();
                    film.layers.push(FilmLayer::new(id, kind, seed));
                    Ok(Some(id))
                })
            }
        ),
        cmd!("film.delete", "Delete Film Effect", [], None, "{id}", has_active, |s, p| {
            let c = "film.delete";
            let lid = layer_id(p, c)?;
            film_edit(s, c, "Delete Film Effect", |film| {
                let i = find(film, lid, c)?;
                film.layers.remove(i);
                Ok(film.layers.get(i.saturating_sub(1)).or(film.layers.first()).map(|l| l.id))
            })
        }),
        cmd!("film.visible", "Show/Hide Film Effect", [], None, "{id, visible?: bool} — toggles when `visible` is left out", has_active, |s, p| {
            let c = "film.visible";
            let lid = layer_id(p, c)?;
            film_edit(s, c, "Show/Hide Film Effect", |film| {
                let i = find(film, lid, c)?;
                let l = &mut film.layers[i];
                l.visible = bool_or(p, "visible", !l.visible);
                Ok(Some(lid))
            })
        }),
        cmd!(
            "film.duplicate",
            "Duplicate Film Effect",
            [],
            None,
            "{id} — the copy goes right above the original with its own random pattern; answers {id, index}",
            has_active,
            |s, p| {
                let c = "film.duplicate";
                let lid = layer_id(p, c)?;
                let seed = seed_param(p, c)?;
                film_edit(s, c, "Duplicate Film Effect", |film| {
                    room(film, c)?;
                    let i = find(film, lid, c)?;
                    let next = film.next_id();
                    let copy = FilmLayer { id: next, seed, ..film.layers[i].clone() };
                    film.layers.insert(i + 1, copy);
                    Ok(Some(next))
                })
            }
        ),
        cmd!(
            "film.move",
            "Reorder Film Effects",
            [],
            None,
            "{id, to?: index (0 = bottom), delta?: ±n (+1 = one up)} — layers draw bottom to top",
            has_active,
            |s, p| {
                let c = "film.move";
                let lid = layer_id(p, c)?;
                film_edit(s, c, "Reorder Film Effects", |film| {
                    let i = find(film, lid, c)?;
                    let last = film.layers.len().saturating_sub(1) as i64;
                    let to = match (p.get("to").and_then(Value::as_i64), p.get("delta").and_then(Value::as_i64)) {
                        (Some(t), _) => t,
                        // (an agent's delta can be anything: saturate rather than overflow)
                        (None, Some(d)) => (i as i64).saturating_add(d),
                        (None, None) => return Err(bad(c, "give `to` (index) or `delta`")),
                    }
                    .clamp(0, last) as usize;
                    let l = film.layers.remove(i);
                    film.layers.insert(to, l);
                    Ok(Some(lid))
                })
            }
        ),
        cmd!("film.shuffle", "Shuffle Film Effect", [], None, "{id, seed?: u32} — a new random pattern for the layer", has_active, |s, p| {
            let c = "film.shuffle";
            let lid = layer_id(p, c)?;
            let seed = seed_param(p, c)?;
            film_edit(s, c, "Shuffle Film Effect", |film| {
                let i = find(film, lid, c)?;
                film.layers[i].seed = seed;
                Ok(Some(lid))
            })
        }),
        cmd!(
            "film.list",
            "List Film Effects",
            [],
            None,
            "{} — the active photo's film settings: {frame, frame_tone, border, markings, vary, layers}",
            has_active,
            |s, _| {
                let id = s.active().ok_or_else(|| bad("film.list", "no active photo"))?;
                let d = s.develop_of(id).unwrap_or_default();
                serde_json::to_value(&d.film).map_err(|e| bad("film.list", e.to_string()))
            }
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_seeds_differ() {
        let a: Vec<u32> = (0..64).map(|_| fresh_seed()).collect();
        let mut b = a.clone();
        b.sort_unstable();
        b.dedup();
        assert_eq!(b.len(), a.len());
    }

    #[test]
    fn kind_names_parse() {
        for k in FilmKind::ALL {
            let v = serde_json::to_value(k).unwrap();
            assert_eq!(serde_json::from_value::<FilmKind>(v).unwrap(), k);
        }
    }
}
