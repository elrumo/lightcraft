//! Search by description: the model (`vision.model.*`), indexing the library (`vision.index*`) and
//! the search itself (`library.search`). See [`crate::vision`].

use std::sync::atomic::Ordering;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::vision::{DEFAULT_LIMIT, LICENSE_NAME, LICENSE_URL, MODEL_BYTES};

fn ids_param(p: &Value) -> Option<Vec<PhotoId>> {
    p.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(PhotoId).collect())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "vision.model.status",
            "Search Model Status",
            [],
            None,
            "{} — whether this build can search by description, whether the model is installed (and where), loaded, busy, and how many photos are indexed → {available, installed, dir, loaded, loading, busy, searching, indexed (null until the index has been opened), photos, sizeBytes, license, licenseUrl, mirrors, download: {running, done, total, file, error, finished}, index: {total, done, failed, running, cancelled, error} | null}",
            always,
            |s, _| Ok(s.vision_status())
        ),
        cmd!(
            query "vision.model.download",
            "Download Search Model",
            [],
            None,
            "{acknowledged: true} — download the search model (SigLIP 2, about 1.5 GB, Google's Apache License 2.0, not LightCraft's) in the background, from the configured mirrors; only after the user agreed to it. Watch vision.model.status; vision.model.cancel stops it (it resumes later) → {started, installed, downloading}",
            always,
            |s, p| {
                let c = "vision.model.download";
                if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
                    return Err(bad(
                        c,
                        format!(
                            "the search model is a {:.1} GB download under the {LICENSE_NAME} ({LICENSE_URL}): pass `acknowledged: true` once the user has agreed to download it",
                            MODEL_BYTES as f64 / 1e9
                        ),
                    ));
                }
                let started = s.vision.start_download().map_err(|e| bad(c, e))?;
                Ok(json!({"started": started, "installed": s.vision.installed(), "downloading": s.vision.download_status()["running"]}))
            }
        ),
        cmd!(
            query "vision.model.cancel",
            "Cancel Search Model Download",
            [],
            None,
            "{} — stop the search model download (what has arrived is kept, and a new download resumes from it) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.vision.cancel_download()}))
        ),
        cmd!(
            query "vision.index",
            "Index Photos for Search",
            [],
            None,
            "{ids?: [photo ids], wait?: bool} — compute the search vector of every library photo that has none yet (or of `ids`), from an unedited rendering, in the background unless `wait`; needs the model (vision.model.status) → {total, done, failed, running, cancelled, error}. Watch vision.indexProgress; vision.indexCancel stops it",
            always,
            |s, p| {
                let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
                s.vision_index(ids_param(p), wait).map_err(|e| bad("vision.index", e))
            }
        ),
        cmd!(
            query "vision.indexProgress",
            "Search Indexing Progress",
            [],
            None,
            "{} → {total, done, failed, running, cancelled, error} | null (no run yet)",
            always,
            |s, _| Ok(s.vision.job().map_or(Value::Null, |j| j.json()))
        ),
        cmd!(
            query "vision.share",
            "Send Search Data to the Server",
            [],
            None,
            "{wait?: bool} — send the sync server the search vectors this device computed that it doesn't have (so it and the user's other devices needn't compute them), in pieces; the server keeps only photos of this library, for the same model. Needs a server that can search (vision.model.status → server) and indexed photos → {running, sent, error}; watch vision.model.status → share",
            always,
            |s, p| {
                let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
                s.vision_share(wait).map_err(|e| bad("vision.share", e))
            }
        ),
        cmd!(
            "vision.setShare",
            "Share Search Data with the Server",
            [],
            None,
            "{on: bool} — whether this library sends its search vectors to the sync server automatically whenever new photos are indexed (off until turned on; saved with the library) → {enabled}",
            always,
            |s, p| {
                let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("vision.setShare", "missing `on`"))?;
                s.vision.share_with_server = on;
                // (saved with the library, like the other preferences)
                s.save_prefs()?;
                Ok(json!({"enabled": on}))
            }
        ),
        cmd!(query "vision.indexCancel", "Cancel Search Indexing", [], None, "{}", always, |s, _| {
            if let Some(j) = s.vision.job() {
                j.cancel.store(true, Ordering::Relaxed);
            }
            Ok(s.vision.job().map_or(Value::Null, |j| j.json()))
        }),
        cmd!(
            "library.search",
            "Search by Description",
            [],
            None,
            "{q: text, limit?: 1..2000 (200), wait?: bool, source?: auto|local|server (auto)} — the library photos that best match a description (\"a dog on a beach at sunset\", in any language the model reads), best first, as the view's filter (Clear Filters to go back). `local` uses this device's model and index (vision.model.status, vision.index); `server` asks the sync server, which needs no model here; `auto` uses this device's index when it covers the library, else the server's. In the app the search runs in the background and the view updates when it is done → {query, source, photos: [{id, score}], indexed, libraryPhotos} (with `wait`; else {status: \"searching\"})",
            always,
            |s, p| {
                let c = "library.search";
                let q = str_param(p, "q").or_else(|| str_param(p, "query")).ok_or_else(|| bad(c, "missing text `q`"))?;
                let limit = p.get("limit").and_then(Value::as_u64).map_or(DEFAULT_LIMIT, |n| n as usize);
                let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
                s.vision_search(q, limit, wait, str_param(p, "source").unwrap_or("auto")).map_err(|e| bad(c, e))
            }
        ),
    ]
}
