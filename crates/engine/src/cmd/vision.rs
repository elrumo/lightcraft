//! Search by description: the model (`vision.model.*`), indexing the library (`vision.index*`) and
//! the search itself (`library.search`). See [`crate::vision`].

use std::sync::atomic::Ordering;

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::vision::{
    DEFAULT_LIMIT, FACE_BYTES, FACE_LICENSES, LICENSE_NAME, LICENSE_URL, MODEL_BYTES, SENSITIVE_BYTES, SENSITIVE_LICENSE_NAME, SENSITIVE_LICENSE_URL,
    TEXT_BYTES, TEXT_LICENSE_NAME, TEXT_LICENSE_URL,
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
                let started = s.vision.start_download(&s.activity).map_err(|e| bad(c, e))?;
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
            "{ids?: [photo ids], wait?: bool} — compute the search vector of every library photo that has none yet (or of `ids`), from an unedited rendering (a synced photo whose original stays on the server has its small preview downloaded for it: waited for with `wait`, else it is indexed by a later run), and, when text search is on (vision.setText) and its models are installed, read the text in each photo too (slower: the reading phase); in the background unless `wait`; needs the search model or the text models (vision.model.status) → {total, done, failed, running, cancelled, phase, error}. Watch vision.indexProgress; vision.indexCancel stops it",
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
            query "vision.helper",
            "Processing for the Server",
            [],
            None,
            "{} — how this device is doing as the sync server's processing unit (vision.setHelper) → {parts, paused: [part] (vision.setHelperPaused), supported, available: {part: true | why not here}, running, state (working|waiting|paused|stopped), done: {search, text, faces, sensitive}, failed, error, unavailable: {part: why}, refused: {part: why}}",
            always,
            |s, _| Ok(s.vision_helper_status())
        ),
        cmd!(
            "vision.setHelper",
            "Process Photos for the Server",
            [],
            None,
            "{parts: [search|text|faces|sensitive]} — the model work this device does for the sync server it is signed in to, instead of the server: the server stops doing those parts for this user, and this device claims a few photos at a time, fetches only their previews, processes them in memory (it needs those models: vision.model.download and the like), sends the results back and forgets the photos. Nothing else leaves the server and editing stays on the device. Saved with the library; [] gives the work back to the server → vision.helper",
            always,
            |s, p| {
                let parts: Vec<String> = p
                    .get("parts")
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad("vision.setHelper", "missing `parts`"))?
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                s.vision_helper_set(&parts).map_err(|e| bad("vision.setHelper", e))
            }
        ),
        cmd!(
            "vision.setHelperPaused",
            "Pause Processing for the Server",
            [],
            None,
            "{parts: [search|text|faces|sensitive]} — which of the work this device does for its server (vision.setHelper) is paused: those parts stay this device's, so the server doesn't take them back, but it claims no photos for them until they are left out of the list again. [] resumes everything. Saved with the library → vision.helper",
            always,
            |s, p| {
                let parts: Vec<String> = p
                    .get("parts")
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad("vision.setHelperPaused", "missing `parts`"))?
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                s.vision_helper_set_paused(&parts).map_err(|e| bad("vision.setHelperPaused", e))
            }
        ),
        cmd!(
            "vision.setLocalOnly",
            "Search on This Device Only",
            [],
            None,
            "{on: bool} — whether searching by description stays on this device (its model, downloaded here: vision.model.download) even when the sync server can search for it. Off by default: a server whose admin turned search on answers every device's searches, so nobody downloads the model; on, no search text is sent to the server (sending the search data, vision.setShare, is its own choice). Saved with the library; no effect on a device that can't run the model → {enabled}",
            always,
            |s, p| {
                let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("vision.setLocalOnly", "missing `on`"))?;
                s.vision.local_only = on;
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
                let started = s.vision.start_text_download(&s.activity).map_err(|e| bad(c, e))?;
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
                let started = s.vision.start_faces_download(&s.activity).map_err(|e| bad(c, e))?;
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
            "vision.setSensitive",
            "Check Photos for Sensitive Content",
            [],
            None,
            "{on: bool} — whether photos are checked for sensitive content (nudity and the like) when they are indexed (vision.index), and the ones that might be listed (library.source {kind: sensitive}) and blurred in the grid (library.setBlurSensitive). Nothing is hidden by it. Off until turned on, and only after the user agreed to it (the model is a download of its own, vision.sensitive.download); saved with the library → {enabled}",
            always,
            |s, p| {
                let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("vision.setSensitive", "missing `on`"))?;
                s.vision.sensitive = on;
                s.save_prefs()?;
                Ok(json!({"enabled": on}))
            }
        ),
        cmd!(
            "vision.setSensitiveBackend",
            "Choose What Checks Photos for Sensitive Content",
            [],
            None,
            "{backend: auto|cpu|gpu} — which weights of the sensitive-content model check photos: auto (the GPU's when this device has a GPU and the GPU weights are downloaded, else the CPU model), cpu (the int8 model, on the processor) or gpu (the full model on the graphics processor; an error when the weights aren't downloaded), for whichever model `vision.setSensitiveModel` chose. Saved with the library → vision.sensitiveBackend",
            always,
            |s, p| {
                let b = p.get("backend").and_then(Value::as_str).ok_or_else(|| bad("vision.setSensitiveBackend", "missing `backend`"))?;
                s.vision.set_sensitive_backend(b).map_err(|e| bad("vision.setSensitiveBackend", e))?;
                s.save_prefs()?;
                Ok(s.vision.sensitive_backend_status())
            }
        ),
        cmd!(
            "vision.setSensitiveModel",
            "Choose the Model That Checks Photos for Sensitive Content",
            [],
            None,
            "{model: auto|compact|large} — which sensitive-content model checks photos: auto (the large one when it is downloaded, else the compact one), compact (Falconsai's, 224 px) or large (AdamCodd's, 384 px, more accurate and slower; an error when it isn't downloaded). Each model keeps its own scores, so a photo is checked again by the other. Saved with the library → vision.sensitiveBackend",
            always,
            |s, p| {
                let m = p.get("model").and_then(Value::as_str).ok_or_else(|| bad("vision.setSensitiveModel", "missing `model`"))?;
                s.vision.set_sensitive_model(m).map_err(|e| bad("vision.setSensitiveModel", e))?;
                s.save_prefs()?;
                Ok(s.vision.sensitive_backend_status())
            }
        ),
        cmd!(
            query "vision.sensitiveBackend",
            "What Checks Photos for Sensitive Content",
            [],
            None,
            "{} → {choice: auto|cpu|gpu, using: cpu|gpu|null (what runs now; null until the model loads), model: auto|compact|large, usingModel: compact|large|null, cpuInstalled, gpuInstalled (the compact model's files), largeInstalled (either of its files), largeCpuInstalled, largeGpuInstalled, gpuAvailable (this device has a GPU the model can run on)}",
            always,
            |s, _| Ok(s.vision.sensitive_backend_status())
        ),
        cmd!(
            query "vision.sensitive.download",
            "Download Sensitive-Content Model",
            [],
            None,
            "{acknowledged: true} — download the model that checks photos for sensitive content (Falconsai's NSFW image classifier, about 87 MB, Apache License 2.0, not LightCraft's) in the background; only after the user agreed to it. Watch vision.model.status → sensitive.download; vision.sensitive.cancel stops it (it resumes later) → {started, installed, downloading}",
            always,
            |s, p| {
                let c = "vision.sensitive.download";
                if p.get("acknowledged").and_then(Value::as_bool) != Some(true) {
                    return Err(bad(
                        c,
                        format!(
                            "the sensitive-content model is a {:.0} MB download under the {SENSITIVE_LICENSE_NAME} ({SENSITIVE_LICENSE_URL}): pass `acknowledged: true` once the user has agreed to download it",
                            SENSITIVE_BYTES as f64 / 1e6
                        ),
                    ));
                }
                let started = s.vision.start_sensitive_download(&s.activity).map_err(|e| bad(c, e))?;
                Ok(json!({"started": started, "installed": s.vision.sensitive_installed(), "downloading": s.vision.sensitive_download_status()["running"]}))
            }
        ),
        cmd!(
            query "vision.sensitive.cancel",
            "Cancel Sensitive-Content Model Download",
            [],
            None,
            "{} — stop the sensitive-content model download (what has arrived is kept, and a new download resumes from it) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": s.vision.cancel_sensitive_download()}))
        ),
        cmd!(
            query "vision.sensitive.list",
            "List Possibly Sensitive Photos",
            [],
            None,
            "{wait?: bool} — the photos the sensitive-content model flagged (here, or on the sync server when it checks them; `wait` asks the server now), most likely first → {photos: [{id, score}], min}",
            always,
            |s, p| {
                if p.get("wait").and_then(Value::as_bool) == Some(true) {
                    s.vision_fetch_sensitive(true);
                }
                let photos: Vec<Value> = s.sensitive_photos().iter().map(|(id, score)| json!({"id": id.0, "score": score})).collect();
                Ok(json!({"photos": photos, "min": crate::vision::SENSITIVE_MIN}))
            }
        ),
        cmd!(
            "library.hideSensitive",
            "Hide All Possibly Sensitive Photos",
            [],
            None,
            "{} — hide every photo the sensitive-content model flagged (one undo step) → {changed}",
            always,
            |s, _| {
                let ids: Vec<PhotoId> = s.sensitive_photos().iter().map(|(id, _)| *id).collect();
                Ok(json!({"changed": super::hidden::set_hidden(s, &ids, true)?}))
            }
        ),
        cmd!(
            "library.setBlurSensitive",
            "Blur Sensitive Photos",
            [],
            None,
            "{on: bool} — blur the photos the sensitive-content model flagged in the grid and filmstrip (on by default; synced with the library) → {enabled}",
            always,
            |s, p| {
                let on = p.get("on").and_then(Value::as_bool).ok_or_else(|| bad("library.setBlurSensitive", "missing `on`"))?;
                s.blur_sensitive = on;
                s.save_prefs()?;
                Ok(json!({"enabled": on}))
            }
        ),
        cmd!(
            query "people.list",
            "List People",
            [],
            None,
            "{all?: bool, limit?: 1..1000 (200), wait?: bool} — the people found (here, or on the sync server when it finds them and this device doesn't; `wait` asks the server now) in the photos (vision.setFaces on, vision.index): those nobody has named, and named people with faces not yet confirmed (`all`: everyone), biggest first → {people: [{id, name | null, faces, photos, pending (faces not yet written to the catalog), cover: {photo, rect}, photoIds}], totalPeople, source: local|server, faces, scanned, libraryPhotos}",
            always,
            |s, p| {
                let all = p.get("all").and_then(Value::as_bool).unwrap_or(false);
                let limit = p.get("limit").and_then(Value::as_u64).map_or(200, |n| n.clamp(1, 1000) as usize);
                let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
                s.people_list(all, limit, wait).map_err(|e| bad("people.list", e))
            }
        ),
        cmd!(
            query "people.named",
            "Named People",
            [],
            None,
            "{all?: bool} → [{name, photos, cover: {photo, rect}}] the people named on the library's photos (in any app), most photos first, each with their largest face (`rect` on the photo's upright frame: render it with the host's render.face); the grid's filter applies, except a person filter (`all`: the whole library)",
            always,
            |s, p| {
                let filter = if p.get("all").and_then(Value::as_bool).unwrap_or(false) {
                    lightcraft_catalog::Filter::default()
                } else {
                    lightcraft_catalog::Filter { person: None, ..s.filter.clone() }
                };
                Ok(Value::Array(
                    s.catalog
                        .people_in(&filter)
                        .into_iter()
                        .map(|p| json!({"name": p.name, "photos": p.count, "cover": {"photo": p.photo.0, "rect": p.face}}))
                        .collect(),
                ))
            }
        ),
        cmd!(
            "people.rename",
            "Rename a Person",
            [],
            None,
            "{from, to} — every face named `from` (any case) across the library gets the name `to`, in one undoable step → {named, photos}",
            always,
            |s, p| {
                let c = "people.rename";
                let from = str_param(p, "from").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(c, "missing `from`"))?;
                let to = str_param(p, "to").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(c, "missing `to`"))?;
                let faces: Vec<Value> =
                    faces_named(&s.catalog, from).into_iter().map(|(photo, index)| json!({"photo": photo, "index": index})).collect();
                if faces.is_empty() {
                    return Err(bad(c, "Nobody has that name any more"));
                }
                s.execute("faces.nameFaces", &json!({"faces": faces, "name": to}))
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
            "{wait?: bool} — forget every face found (the face index of this library, and what the sync server found for this user, if it finds any); names already written to photos stay, they are catalog data (photo.removeRegion removes one). Turn finding off too with vision.setFaces → {faces, photos, server (the server was asked)}",
            always,
            |s, p| {
                let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
                s.people_delete_data(wait).map_err(|e| bad("people.deleteData", e))
            }
        ),
        cmd!(
            "vision.reprocess",
            "Process Photos Again",
            [],
            None,
            "{part: search|text|faces|sensitive, scope?: device|server|both (both)} — forget what one kind of processing found and do it again: `device` deletes this library's results (names written to photos stay) and starts indexing, `server` deletes this user's results on the sync server (which then does every photo again, or has the device that does it for it do so), `both` does both when this library is signed in. The models stay → {part, device, server, indexing}",
            always,
            |s, p| {
                let c = "vision.reprocess";
                let part = p.get("part").and_then(Value::as_str).ok_or_else(|| bad(c, "missing `part`"))?;
                let scope = p.get("scope").and_then(Value::as_str).unwrap_or("both");
                s.vision_reprocess(part, scope).map_err(|e| bad(c, e))
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
            "{q: text, limit?: 1..2000 (200), wait?: bool, source?: auto|local|server (auto)} — the library photos that best match a description (\"a dog on a beach at sunset\", in any language the model reads), best first, as the view's filter (Clear Filters to go back). `local` uses this device's model and index (vision.model.status, vision.index); `server` asks the sync server, which needs no model here; `auto` uses this device's index when it covers the library, else the server's. With text search on (vision.setText), photos with the words printed in them come first (`text: true`, any language the reader knows; a typo in a long word still matches). A one-word query that is part of a file name (`IMG_4008`, `img_4008.dng`) puts those photos first (`name: true`), even before anything is indexed. In the app the search runs in the background and the view updates when it is done → {query, source, photos: [{id, score, text, name}], indexed, libraryPhotos} (with `wait`; else {status: \"searching\"})",
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

/// The faces (photo, region index) named `from`, any case, on the library's photos.
pub fn faces_named(cat: &lightcraft_catalog::Catalog, from: &str) -> Vec<(u64, usize)> {
    let from = from.trim();
    let mut out = Vec::new();
    for p in cat.photos().filter(|p| p.in_library()) {
        for (i, r) in p.meta.regions.iter().enumerate() {
            if r.kind == lightcraft_meta::RegionKind::Face && r.name.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(from)) {
                out.push((p.id.0, i));
            }
        }
    }
    out
}
