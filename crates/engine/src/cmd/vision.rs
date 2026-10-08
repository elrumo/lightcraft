//! Search by description: the model (`vision.model.*`), indexing the library (`vision.index*`) and
//! the search itself (`library.search`). See [`crate::vision`].

use std::sync::atomic::Ordering;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::vision::{
    DEFAULT_LIMIT, FACE_BYTES, FACE_LICENSES, LICENSE_NAME, LICENSE_URL, MODEL_BYTES, TEXT_BYTES, TEXT_LICENSE_NAME, TEXT_LICENSE_URL,
};

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
            "{} — whether this build can search by description, whether the model is installed (and where), loaded, busy, and how many photos are indexed → {available, installed, dir, loaded, loading, busy, searching, indexed (null until the index has been opened), photos, sizeBytes, license, licenseUrl, mirrors, download: {running, done, total, file, error, finished}, index: {total, done, failed, running, cancelled, phase (describing|reading), error} | null, text: {available, enabled, installed, dir, indexed (photos read; null until opened), sizeBytes, license, licenseUrl, download: {…}}}",
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
            "{ids?: [photo ids], wait?: bool} — compute the search vector of every library photo that has none yet (or of `ids`), from an unedited rendering, and, when text search is on (vision.setText) and its models are installed, read the text in each photo too (slower: the reading phase); in the background unless `wait`; needs the search model or the text models (vision.model.status) → {total, done, failed, running, cancelled, phase, error}. Watch vision.indexProgress; vision.indexCancel stops it",
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
        cmd!(
            "vision.setText",
            "Search the Text in Photos",
            [],
            None,
            "{on: bool} — whether search also reads the text printed in photos (signs, menus, documents, screenshots; English, Chinese, Japanese and other languages) and finds photos by it. Off until turned on (saved with the library). Reading needs its own models (vision.text.download) and is slow: it happens when photos are indexed (vision.index) → {enabled}",
            always,
            |s, p| {
                let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("vision.setText", "missing `on`"))?;
                s.vision.text = on;
                s.save_prefs()?;
                Ok(json!({"enabled": on}))
            }
        ),
        cmd!(
            query "vision.text.download",
            "Download Text-Reading Models",
            [],
            None,
            "{acknowledged: true} — download the models that read the text in photos (PP-OCRv6, about 31 MB, Baidu's Apache License 2.0, not LightCraft's) in the background; only after the user agreed to it. Watch vision.model.status → text.download; vision.text.cancel stops it (it resumes later) → {started, installed, downloading}",
            always,
            |s, p| {
                let c = "vision.text.download";
                if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
                    return Err(bad(
                        c,
                        format!(
                            "the text-reading models are a {:.0} MB download under the {TEXT_LICENSE_NAME} ({TEXT_LICENSE_URL}): pass `acknowledged: true` once the user has agreed to download them",
                            TEXT_BYTES as f64 / 1e6
                        ),
                    ));
                }
                let started = s.vision.start_text_download().map_err(|e| bad(c, e))?;
                Ok(json!({"started": started, "installed": s.vision.text_installed(), "downloading": s.vision.text_download_status()["running"]}))
            }
        ),
        cmd!(
            query "vision.text.cancel",
            "Cancel Text-Reading Models Download",
            [],
            None,
            "{} — stop the text-reading models download (what has arrived is kept, and a new download resumes from it) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.vision.cancel_text_download()}))
        ),
        cmd!(
            "vision.setFaces",
            "Find People in Photos",
            [],
            None,
            "{on: bool} — whether the faces in photos are found and grouped into people (people.list). Off until turned on, and only after the user agreed to it (the face models are a download of their own, vision.faces.download); saved with the library. Faces and what they look like stay in this library's search folder (and on the sync server only if the user shares them); naming a person writes face regions to the catalog → {enabled}",
            always,
            |s, p| {
                let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("vision.setFaces", "missing `on`"))?;
                s.vision.faces = on;
                s.save_prefs()?;
                Ok(json!({"enabled": on}))
            }
        ),
        cmd!(
            query "vision.faces.download",
            "Download Face Models",
            [],
            None,
            "{acknowledged: true} — download the models that find and tell apart faces (YuNet, MIT; SFace, Apache 2.0; about 39 MB; their makers' licences, not LightCraft's) in the background; only after the user agreed to it. Watch vision.model.status → faces.download; vision.faces.cancel stops it (it resumes later) → {started, installed, downloading}",
            always,
            |s, p| {
                let c = "vision.faces.download";
                if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
                    return Err(bad(
                        c,
                        format!(
                            "the face models are a {:.0} MB download under the licences of their makers ({}; {}): pass `acknowledged: true` once the user has agreed to download them",
                            FACE_BYTES as f64 / 1e6,
                            FACE_LICENSES[0].0,
                            FACE_LICENSES[1].0
                        ),
                    ));
                }
                let started = s.vision.start_faces_download().map_err(|e| bad(c, e))?;
                Ok(json!({"started": started, "installed": s.vision.faces_installed(), "downloading": s.vision.faces_download_status()["running"]}))
            }
        ),
        cmd!(
            query "vision.faces.cancel",
            "Cancel Face Models Download",
            [],
            None,
            "{} — stop the face models download (what has arrived is kept, and a new download resumes from it) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.vision.cancel_faces_download()}))
        ),
        cmd!(
            query "people.list",
            "List People",
            [],
            None,
            "{all?: bool, limit?: 1..1000 (200)} — the people found in the photos (vision.setFaces on, vision.index): those nobody has named, and named people with faces not yet confirmed (`all`: everyone), biggest first → {people: [{id, name | null, faces, photos, pending (faces not yet written to the catalog), cover: {photo, rect}, photoIds}], totalPeople, faces, scanned, libraryPhotos}",
            always,
            |s, p| {
                let all = p.get("all").and_then(Value::as_bool).unwrap_or(false);
                let limit = p.get("limit").and_then(Value::as_u64).map_or(200, |n| n.clamp(1, 1000) as usize);
                s.people_list(all, limit).map_err(|e| bad("people.list", e))
            }
        ),
        cmd!(
            "people.name",
            "Name a Person",
            [],
            None,
            "{cluster: a person's id from people.list, name: text} — name a person: a face region with that name is added to each photo they are in (or named, when a box is there already), in one undoable step; faces that already have another name keep it. Naming an already named person confirms the faces found since → {name, faces, photos, keptOtherName}",
            always,
            |s, p| {
                let c = "people.name";
                let cluster = str_param(p, "cluster").ok_or_else(|| bad(c, "missing `cluster`"))?;
                let name = str_param(p, "name").ok_or_else(|| bad(c, "missing `name`"))?;
                s.people_name(cluster, name).map_err(|e| bad(c, e))
            }
        ),
        cmd!(
            "people.show",
            "Show a Person's Photos",
            [],
            None,
            "{cluster: a person's id from people.list} — the view becomes that person's photos, oldest first (Clear Filters to go back) → {person, photos}",
            always,
            |s, p| {
                let c = "people.show";
                s.people_show(str_param(p, "cluster").ok_or_else(|| bad(c, "missing `cluster`"))?).map_err(|e| bad(c, e))
            }
        ),
        cmd!(
            "people.deleteData",
            "Forget All Faces",
            [],
            None,
            "{} — forget every face found (the face index of this library); names already written to photos stay, they are catalog data (photo.removeRegion removes one). Turn finding off too with vision.setFaces → {faces, photos}",
            always,
            |s, _| s.people_delete_data().map_err(|e| bad("people.deleteData", e))
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
            "{q: text, limit?: 1..2000 (200), wait?: bool, source?: auto|local|server (auto)} — the library photos that best match a description (\"a dog on a beach at sunset\", in any language the model reads), best first, as the view's filter (Clear Filters to go back). `local` uses this device's model and index (vision.model.status, vision.index); `server` asks the sync server, which needs no model here; `auto` uses this device's index when it covers the library, else the server's. With text search on (vision.setText), photos with the words printed in them come first (`text: true`, any language the reader knows; a typo in a long word still matches). In the app the search runs in the background and the view updates when it is done → {query, source, photos: [{id, score, text}], indexed, libraryPhotos} (with `wait`; else {status: \"searching\"})",
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
