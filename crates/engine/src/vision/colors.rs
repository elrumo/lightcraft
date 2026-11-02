//! The colours of photos on this device: "red", "blue sky" and the like find the photos that are mostly that colour.
//!
//! No model and no download: [`lightcraft_vision::colors::extract`] reads a small copy of the neutral rendering that
//! indexing makes anyway, so it runs in the same pass as the description (`describe_photos`), and for photos that are
//! already described (or when no description model is here) in a silent stage of its own ([`color_photos`]) that
//! `vision.index` always includes. The palettes are derived data in the library's `search/` folder
//! (`colors-<engine>.bin`), never in the catalog. A server keeps its own (`apps/lightcraft-server/src/colors.rs`) and
//! `library.search` with the server as source asks it; this device's index is for local searches.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use lightcraft_vision::colors::{self, ColorIndex};
use lightcraft_vision::{Hit, Key};
use serde_json::{Value, json};

use super::{IndexJob, Shared};
use crate::{Session, guard, memory};

/// Where the palettes live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ColorSpec {
    /// `None`: in memory only (a library that isn't a folder).
    pub path: Option<PathBuf>,
}

/// What a search by colour looks for.
#[derive(Clone, Debug)]
pub(super) struct ColorWhere {
    pub spec: ColorSpec,
    /// Indices into [`colors::BUCKETS`]: a photo needs all of them.
    pub buckets: Vec<usize>,
    /// The query names nothing but colours.
    pub only: bool,
}

/// Opens the palettes at `spec` (another algorithm's, or damaged, is derived data: started over).
fn open_index(spec: &ColorSpec) -> Result<ColorIndex, String> {
    use lightcraft_vision::Error;
    let Some(path) = &spec.path else { return Ok(ColorIndex::memory()) };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("can't create {}: {e}", parent.display()))?;
    }
    match ColorIndex::open(path, colors::ENGINE) {
        Ok(ix) => Ok(ix),
        Err(Error::Mismatch { .. } | Error::Format(_)) => {
            log::info!("colours {} are another version's or damaged: starting over", path.display());
            let _ = std::fs::remove_file(path);
            ColorIndex::open(path, colors::ENGINE).map_err(|e| e.to_string())
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Runs `f` on the palettes at `spec`, opening them first (or again, when they are other ones).
pub(super) fn with_colors<R>(shared: &Shared, spec: &ColorSpec, f: impl FnOnce(&mut ColorIndex) -> R) -> Result<R, String> {
    let mut slot = shared.colors.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.as_ref().is_none_or(|(p, _)| *p != spec.path) {
        let ix = open_index(spec)?;
        *slot = Some((spec.path.clone(), ix));
    }
    let (_, ix) = slot.as_mut().ok_or("the colours are unavailable")?;
    let r = f(ix);
    shared.colors_len.store(ix.len(), Ordering::Relaxed);
    shared.colors_opened.store(true, Ordering::Relaxed);
    Ok(r)
}

/// Keeps the palette of a photo rendered for another purpose (the description). A failed write is only logged: the
/// next run makes it again.
pub(super) fn keep(shared: &Shared, spec: &ColorSpec, key: Key, img: &lightcraft_raster::Rgba8) {
    let kept = guard::catch("reading colours", || colors::extract(img))
        .and_then(|palette| with_colors(shared, spec, |ix| ix.insert(key, palette).map_err(|e| e.to_string()))?);
    if let Err(e) = kept {
        log::debug!("colours: can't keep a palette: {e}");
    }
}

/// Works out the palettes of `work`'s photos, one at a time. Silent: it neither counts in the run's totals nor fails it.
pub(super) fn color_photos(shared: &Arc<Shared>, spec: &ColorSpec, work: Vec<(Key, crate::media::RenderJob)>, job: &IndexJob) {
    for (key, render) in work {
        if job.cancel.load(Ordering::Relaxed) {
            break;
        }
        match memory::in_background(|| render.run().rendered) {
            Ok(r) => keep(shared, spec, key, &r.image),
            Err(e) => log::debug!("colours: can't render a photo: {e}"),
        }
        shared.touch();
    }
}

/// Whether the photo `key` has the colours of `at`.
pub(super) fn has(shared: &Shared, at: &ColorWhere, key: &Key) -> bool {
    with_colors(shared, &at.spec, |ix| ix.matches(key, &at.buckets).is_some()).unwrap_or(false)
}

/// `hits` (the words' and the description's, best first) with the photos that have the colours of `at` in front: first
/// every such photo, most of the colour first, when the query names nothing else; else the hits that have them, in
/// their order. The others follow. At most `limit` in all.
pub(super) fn promote(shared: &Shared, at: &ColorWhere, hits: Vec<Hit>, limit: usize) -> Result<Vec<Hit>, String> {
    with_colors(shared, &at.spec, |ix| {
        let mut out: Vec<Hit> = if at.only {
            ix.search(&at.buckets).into_iter().take(limit).map(|(key, score)| Hit { key, score }).collect()
        } else {
            hits.iter().filter(|h| ix.matches(&h.key, &at.buckets).is_some()).copied().collect()
        };
        let seen: std::collections::HashSet<Key> = out.iter().map(|h| h.key).collect();
        out.extend(hits.into_iter().filter(|h| !seen.contains(&h.key)));
        out.truncate(limit);
        out
    })
}

impl Session {
    /// The palettes file: in the library's `search/` folder, else none (memory only).
    pub(super) fn vision_colors_spec(&self) -> ColorSpec {
        let path = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("search").join(format!("colors-{}.bin", colors::ENGINE)));
        ColorSpec { path }
    }

    /// How many photos have a palette here.
    pub fn vision_colors_len(&self) -> usize {
        with_colors(&self.vision.shared, &self.vision_colors_spec(), |ix| ix.len()).unwrap_or(0)
    }

    /// Where to look when `query` names colours and this device has palettes: `None` for a query without a colour word, or
    /// when no photo has a palette yet (then the whole query goes to the description model as it always did).
    pub(super) fn vision_color_where(&self, query: &colors::ColorQuery) -> Option<ColorWhere> {
        if query.buckets.is_empty() {
            return None;
        }
        let spec = self.vision_colors_spec();
        (with_colors(&self.vision.shared, &spec, |ix| ix.len()).ok()? > 0).then(|| ColorWhere {
            spec,
            buckets: query.buckets.clone(),
            only: query.rest.is_empty(),
        })
    }

    /// The colour names a search understands, for a front end to suggest (`library.colorNames`): `[{name, rgb: [r, g, b]}]`
    /// for the twelve colours a photo is counted into, and `words`, every word that stands for one (English first).
    pub fn vision_color_names() -> Value {
        // (a representative colour of each, for a swatch next to the name)
        const RGB: [[u8; 3]; 12] = [
            [220, 40, 40],
            [240, 140, 30],
            [245, 215, 40],
            [60, 170, 70],
            [30, 170, 170],
            [40, 90, 220],
            [140, 60, 190],
            [235, 120, 180],
            [125, 80, 45],
            [20, 20, 20],
            [245, 245, 245],
            [128, 128, 128],
        ];
        let names: Vec<Value> = colors::BUCKETS.iter().zip(RGB).map(|(n, rgb)| json!({"name": n, "rgb": rgb})).collect();
        json!({"colors": names, "words": colors::words().into_iter().map(|(w, b)| json!({"word": w, "color": colors::name(b)})).collect::<Vec<_>>()})
    }
}
