//! Read-only queries (not journaled): catalog, photos, develop state, controls, presets, albums.

use lightcraft_catalog::{Album, Photo, PhotoId};
use lightcraft_develop::{CONTROLS, controls};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_active};
use crate::{LibrarySource, Session};

pub fn photo_summary(p: &Photo) -> Value {
    json!({
        "id": p.id.0,
        "fileName": p.file_name,
        "format": p.format,
        "kind": p.kind,
        "width": p.width,
        "height": p.height,
        "captured": p.captured,
        "rating": p.rating,
        "flag": p.flag,
        "label": p.label,
        "edited": p.is_edited(),
        "title": p.meta.title,
        "keywords": p.meta.keywords,
        "camera": p.meta.camera,
        "deleted": p.deleted,
        "copyOf": p.copy_of.map(|c| c.0),
        "copyName": p.copy_name,
        // an undecodable raw variant, shown and edited from its embedded JPEG: why
        "previewOnly": p.preview_only,
    })
}

/// How deep [`album_json`] lists folders inside folders: a catalog that loops (an album inside
/// itself, which [`lightcraft_catalog::Catalog::repair`] puts right on load) lists no further
/// instead of overflowing the stack.
const MAX_ALBUM_DEPTH: usize = 64;

fn album_json(a: &Album, all: &[Album], cat: &lightcraft_catalog::Catalog, depth: usize) -> Value {
    let children: Vec<Value> = if depth < MAX_ALBUM_DEPTH {
        all.iter().filter(|c| c.parent == Some(a.id)).map(|c| album_json(c, all, cat, depth + 1)).collect()
    } else {
        Vec::new()
    };
    let mut v = json!({
        "id": a.id.0,
        "name": a.name,
        "folder": a.folder,
        "count": cat.album_count(a.id),
        "cover": a.cover.map(|c| c.0),
        "children": children,
    });
    if let Some(rules) = &a.smart {
        v["smart"] = json!(true);
        v["rules"] = serde_json::to_value(rules).unwrap_or_default();
        v["rulesText"] = json!(rules.describe_with(cat));
        // rules that no longer check (an album they test was deleted…), as `rule 1: …`
        v["problems"] = json!(cat.smart_album_problems(a.id).iter().map(ToString::to_string).collect::<Vec<_>>());
    }
    v
}

/// The photo a query is about: `id` when given (which must be in the library), else the active one.
fn photo_arg(s: &Session, p: &Value, c: &str) -> crate::Result<PhotoId> {
    match p.get("id").and_then(Value::as_u64).map(PhotoId) {
        Some(id) if s.catalog.photo(id).is_some() => Ok(id),
        Some(id) => Err(bad(c, format!("no such photo {}", id.0))),
        None => s.active().ok_or_else(|| bad(c, "no photo")),
    }
}

// Catalog queries also run in the 32-bit web build. A narrowing cast wraps
// an out-of-range offset to zero, returning the first page instead of no photos.
fn pagination_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "catalog.query", "Query Photos", [], None, "{filter?: Filter, sort?: Sort, offset?, limit?} — omit filter to list the current view; a stacked photo has `stack: {count, position, collapsed}`", always, |s, p| {
            let ids = if p.get("filter").is_some() || p.get("sort").is_some() {
                let f = p.get("filter").map(|f| serde_json::from_value(f.clone())).transpose().map_err(|e| bad("catalog.query", e.to_string()))?.unwrap_or_default();
                let so = p.get("sort").map(|f| serde_json::from_value(f.clone())).transpose().map_err(|e| bad("catalog.query", e.to_string()))?.unwrap_or_default();
                s.catalog.query(&f, &so)
            } else {
                s.visible_cloned()
            };
            let off = pagination_usize(p.get("offset").and_then(Value::as_u64).unwrap_or(0));
            let lim = pagination_usize(p.get("limit").and_then(Value::as_u64).unwrap_or(200));
            // a stacked photo's place in its stack: the grid's stack badge
            let stacks = if s.catalog.stacks().next().is_some() { s.catalog.stack_index() } else { Default::default() };
            let items: Vec<Value> = ids
                .iter()
                .skip(off)
                .take(lim)
                .filter_map(|id| s.catalog.photo(*id))
                .map(|p| {
                    let mut v = photo_summary(p);
                    if let Some((sid, pos)) = stacks.get(&p.id)
                        && let Some(st) = s.catalog.stack(*sid)
                    {
                        v["stack"] = json!({"count": st.photos.len(), "position": pos, "collapsed": st.collapsed});
                    }
                    v
                })
                .collect();
            Ok(json!({"total": ids.len(), "photos": items}))
        }),
        cmd!(query "catalog.stats", "Catalog Statistics", [], None, "{}", always, |s, _| {
            let all: Vec<_> = s.catalog.photos().filter(|p| p.in_library()).collect();
            Ok(json!({
                "photos": all.len(),
                "edited": all.iter().filter(|p| p.is_edited()).count(),
                "picks": all.iter().filter(|p| p.flag == lightcraft_catalog::Flag::Pick).count(),
                "rejects": all.iter().filter(|p| p.flag == lightcraft_catalog::Flag::Reject).count(),
                "deleted": s.catalog.photos().filter(|p| p.deleted).count(),
                "albums": s.catalog.albums().filter(|a| !a.folder).count(),
                "byDate": s.catalog.date_groups(),
                "keywords": s.catalog.keywords(),
            }))
        }),
        cmd!(query "library.state", "Library State", [], None, "{}", always, |s, _| {
            let label = match (s.source, s.library_folder.as_deref()) {
                (LibrarySource::LibraryFolder, Some(path)) => lightcraft_catalog::folders::folder_label(path),
                _ => s.source.label(&s.catalog),
            };
            // the Local folder shown, and how many of its photos aren't in the library ("Add N to My Photos")
            let browse = match s.browse.clone().filter(|_| s.source == LibrarySource::Folder) {
                Some(b) => {
                    let local = s.visible_cloned().iter().filter(|id| s.catalog.photo(**id).is_some_and(|p| p.local)).count();
                    json!({"path": b.path, "subfolders": b.subfolders, "local": local})
                }
                None => Value::Null,
            };
            let n = s.visible().len();
            let photos = s.visible_photos();
            Ok(json!({
                "source": s.source,
                "libraryFolder": s.library_folder.as_ref().filter(|_| s.source == LibrarySource::LibraryFolder),
                "browse": browse,
                "sourceLabel": label,
                "filter": s.filter,
                "sort": s.sort,
                "selection": s.selection,
                "visibleCount": n,
                // with the photos inside collapsed stacks: the grid's "24 photos"
                "visiblePhotos": photos,
                "activeMask": s.active_mask,
                "activeSpot": s.active_spot,
                "undo": s.undo.last().map(|u| u.label.clone()),
                "redo": s.redo.last().map(|u| u.label.clone()),
                "clipboard": s.clipboard.is_some(),
            }))
        }),
        cmd!(query "photo.allMetadata", "All Metadata", [], None, "{id?} → {exif: [{group, tag, name, value}], xmp: [{name, value}]} — every EXIF / TIFF / GPS tag of the file and its XMP properties", always, |s, p| {
            let id = photo_arg(s, p, "photo.allMetadata")?;
            let ph = s.catalog.photo(id).ok_or_else(|| super::bad("photo.allMetadata", "no such photo"))?.clone();
            let lightcraft_catalog::Source::File { path } = &ph.source else {
                return Ok(json!({"exif": [], "xmp": [], "note": "a generated demo photo has no file"}));
            };
            let bytes = match &s.media.file_bytes {
                Some(r) => r(path),
                None => std::fs::read(path).map_err(|e| format!("{path}: {e}")),
            }
            .map_err(|e| super::bad("photo.allMetadata", e))?;
            let exif: Vec<Value> = lightcraft_meta::file_tag_rows(&bytes)
                .into_iter()
                .map(|r| json!({"group": r.group, "tag": r.tag, "name": r.name, "value": r.value}))
                .collect();
            // XMP: the sidecar if there is one, else the file's own packet
            let packet = crate::sidecar::read_packet(path, ph.kind, s.sidecar_naming(ph.id)).map(|(x, _)| x).or_else(|| lightcraft_meta::embedded(&bytes).xmp);
            let xmp: Vec<Value> = packet
                .and_then(|x| lightcraft_meta::parse_xmp(&x).ok())
                .map(|d| d.properties.into_iter().filter(|(k, _)| !k.starts_with("lc:")).map(|(k, v)| json!({"name": k, "value": v.join("; ")})).collect())
                .unwrap_or_default();
            Ok(json!({"exif": exif, "xmp": xmp}))
        }),
        cmd!(query "albums.list", "List Albums", [], None, "{}", always, |s, _| {
            let all: Vec<Album> = s.catalog.albums().cloned().collect();
            Ok(Value::Array(all.iter().filter(|a| a.parent.is_none()).map(|a| album_json(a, &all, &s.catalog, 0)).collect()))
        }),
        cmd!(query "photo.inspect", "Inspect Photo", [], None, "{id?}", always, |s, p| {
            let id = photo_arg(s, p, "photo.inspect")?;
            let ph = s.catalog.photo(id).ok_or_else(|| bad("photo.inspect", "no such photo"))?;
            let mut v = serde_json::to_value(ph.as_ref()).unwrap_or_default();
            v["albums"] = json!(s.catalog.albums_of(id).iter().map(|a| a.0).collect::<Vec<_>>());
            v["history"] = json!(ph.history.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
            if let Some(st) = s.catalog.stack_of(id) {
                v["stack"] = json!({
                    "id": st.id.0,
                    "photos": st.photos.iter().map(|p| p.0).collect::<Vec<_>>(),
                    "position": st.position(id),
                    "collapsed": st.collapsed,
                });
            }
            Ok(v)
        }),
        cmd!(query "develop.get", "Get Develop Settings", [], None, "{id?} → the photo's develop settings; `process` is the rendering process they are interpreted by (see develop.updateProcess)", always, |s, p| {
            let id = photo_arg(s, p, "develop.get")?;
            Ok(s.develop_of(id)
                .map(|d| {
                    let mut v = d.to_json();
                    // saved settings leave V1 out (`ProcessVersion::is_legacy`); here it is always stated
                    if let Some(o) = v.as_object_mut() {
                        o.insert("process".into(), json!(d.process));
                    }
                    v
                })
                .unwrap_or(Value::Null))
        }),
        cmd!(
            query "view.frame",
            "Photo Frame",
            [],
            None,
            "{id?, applyCrop?: true} → {aspect, width, height, toNorm: [a,b,c,d,e,f], fromNorm, crop: {rect, angle, quad: [[x,y] ×4: top-left, top-right, bottom-right, bottom-left of the unflipped crop], toStraight: [6] (image → the straightened frame crop.drag takes), flipH, flipV}, guides: [[x0,y0,x1,y1]] (Guided Upright's, in image coordinates)} how the photo as shown (cropped, straightened, flipped) maps to the normalized image coordinates masks, spots, the crop and the pickers take: toNorm maps (u, v) in 0..1 across the shown image (top-left first) as (a·u + c·v + e, b·u + d·v + f); fromNorm is its inverse; width × height is the shown image at the photo's own resolution",
            always,
            |s, p| {
                let c = "view.frame";
                let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad(c, "no photo (pass `id`)"))?;
                let photo = s.catalog.photo(id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))?;
                let (sw, sh) = (photo.width.max(1) as usize, photo.height.max(1) as usize);
                let d = s.develop_of(id).unwrap_or_default();
                let apply = p.get("applyCrop").and_then(Value::as_bool).unwrap_or(true);
                let frame = lightcraft_pipeline::geometry::Frame::with_lens(sw, sh, &d, apply, s.embedded_lens(id).as_ref());
                let (w, h) = frame.native_size();
                let (w, h) = (if w.is_finite() { w.round().clamp(1.0, 1e9) } else { 1.0 }, if h.is_finite() { h.round().clamp(1.0, 1e9) } else { 1.0 });
                let to = frame.out_to_norm(w as usize, h as usize) * lightcraft_geom::Affine::scale(w, h);
                let from = to.inverse().unwrap_or(lightcraft_geom::Affine::IDENTITY);
                // the crop on the whole photo (the crop tool's frame): its corners, and how a point of the photo maps to
                // the straightened frame `crop.drag` takes
                let g = d.crop.geometry;
                let quad = lightcraft_pipeline::geometry::Frame { crop: g, ..frame.clone() }.crop_quad_norm();
                let (ow, oh) = (frame.ow.max(1.0), frame.oh.max(1.0));
                let centre = lightcraft_geom::Point::new(ow / 2.0, oh / 2.0);
                let straight = lightcraft_geom::Affine::scale(1.0 / ow, 1.0 / oh)
                    * lightcraft_geom::Affine::rotate_about(g.angle.to_radians(), centre)
                    * lightcraft_geom::Affine::scale(ow, oh);
                // Guided Upright's guides in the same image coordinates
                let gf = super::develop::guide_frame(s, id);
                let guides: Vec<[f64; 4]> = d
                    .geometry
                    .guides
                    .iter()
                    .map(|(a, b)| {
                        let (a, b) = (gf.corrected_to_transformed(*a), gf.corrected_to_transformed(*b));
                        [a.x, a.y, b.x, b.y]
                    })
                    .collect();
                Ok(json!({
                    "aspect": w / h, "width": w, "height": h, "toNorm": to.0, "fromNorm": from.0,
                    "guides": guides,
                    "crop": {
                        "rect": [g.rect.x0, g.rect.y0, g.rect.x1, g.rect.y1],
                        "angle": g.angle,
                        "quad": quad.iter().map(|q| [q.x, q.y]).collect::<Vec<_>>(),
                        "toStraight": straight.0,
                        "flipH": d.crop.flip_h,
                        "flipV": d.crop.flip_v,
                    },
                }))
            }
        ),
        cmd!(query "develop.controls", "List Develop Controls", [], None, "{section?} — every slider with range, default and current value; white balance as the active photo shows it: Kelvin (the effective white for As Shot and presets) for a raw, `wb.tempRel` / `wb.tintRel` (−100…100, relative to the photo's own colour; `develop.set` takes them) for a rendered photo", always, |s, p| {
            let active = s.active();
            let d = active.and_then(|id| s.develop_of(id)).unwrap_or_default();
            let sec = p.get("section").and_then(|v| serde_json::from_value::<lightcraft_develop::Section>(v.clone()).ok());
            // white balance as this photo shows it (the egui app's Color group does the same)
            let photo = active.and_then(|id| s.catalog.photo(id));
            let raw = photo.is_some_and(|p| p.develops_raw() && !p.relative_wb());
            // (with no photo, the plain table: Kelvin)
            let relative = photo.is_some() && !raw;
            let as_shot_rel = d.wb.mode == lightcraft_develop::WbMode::AsShot && photo.is_some_and(|p| p.relative_wb());
            let effective = (raw && photo.is_some()).then(|| active.map(|id| lightcraft_pipeline::local::effective_wb(&s.source_info(id), &d))).flatten();
            Ok(Value::Array(
                CONTROLS
                    .iter()
                    .filter(|c| sec.is_none_or(|x| x == c.section))
                    .map(|c| {
                        let (spec, value) = match c.id {
                            "wb.temp" if relative => (&controls::WB_TEMP_REL, if as_shot_rel { 0.0 } else { controls::kelvin_to_relative(d.wb.temp) }),
                            "wb.tint" if relative => (&controls::WB_TINT_REL, if as_shot_rel { 0.0 } else { d.wb.tint.clamp(-100.0, 100.0) }),
                            "wb.temp" => (c, effective.map_or(d.wb.temp, |e| e.0)),
                            "wb.tint" => (c, effective.map_or(d.wb.tint, |e| e.1)),
                            _ => (c, controls::get(&d, c.id).unwrap_or(c.default)),
                        };
                        let mut v = serde_json::to_value(spec).unwrap_or_default();
                        v["value"] = json!(value);
                        v
                    })
                    // indexed controls of the list elements that exist (`pointColor.0.hueShift`, …)
                    .chain(controls::indexed_instances(&d).into_iter().filter(|(_, c)| sec.is_none_or(|x| x == c.section)).map(|(id, c)| {
                        let mut v = serde_json::to_value(c).unwrap_or_default();
                        v["value"] = json!(controls::get(&d, &id));
                        v["id"] = json!(id);
                        v
                    }))
                    .collect(),
            ))
        }),
        cmd!(query "presets.list", "List Presets", [], None, "{} → [{id, name, group, favorite, builtin (built-in or the sync server's: not editable), hidden (the preset itself, `preset.hide`)}]", always, |s, _| {
            let hidden = |id: &String| s.hidden_presets.presets.contains(id);
            Ok(Value::Array(
                s.presets
                    .iter()
                    .map(|p| json!({"id": p.id, "name": p.name, "group": p.group, "favorite": p.favorite, "builtin": p.builtin, "hidden": hidden(&p.id)}))
                    .collect(),
            ))
        }),
        cmd!(
            query "presets.groups",
            "List Preset Groups",
            [],
            None,
            "{details?: bool} — the preset groups in the order the Presets panel shows them → [name], or with details [{name, hidden (`preset.hideGroup`), builtin (holds built-in or server presets: can't be renamed), user (holds the user's presets)}]",
            always,
            |s, p| {
                let groups = s.preset_groups();
                if !p.get("details").and_then(Value::as_bool).unwrap_or(false) {
                    return Ok(json!(groups));
                }
                let mut kinds: std::collections::HashMap<&str, (bool, bool)> = std::collections::HashMap::new();
                for pr in s.presets.iter() {
                    let k = kinds.entry(pr.group.as_str()).or_default();
                    k.0 |= pr.builtin;
                    k.1 |= !pr.builtin;
                }
                Ok(Value::Array(
                    groups
                        .iter()
                        .map(|g| {
                            let (builtin, user) = kinds.get(g.as_str()).copied().unwrap_or_default();
                            json!({"name": g, "hidden": s.hidden_presets.groups.contains(g), "builtin": builtin, "user": user})
                        })
                        .collect(),
                ))
            }
        ),
        cmd!(query "profiles.list", "List Profiles", [], None, "{} — every profile (built-in, then imported LUTs marked `imported`) with its group and favourite flag", always, |s, _| {
            let favorite = |id: &str| s.profile_favorites.iter().any(|f| f == id);
            let builtin = crate::presets::PROFILES.iter().map(|p| json!({"id": p.id, "name": p.name, "group": p.group, "favorite": favorite(p.id)}));
            // imported LUT profiles too (#618), marked as in `profiles.menu`
            let imported =
                s.lut_profiles.iter().map(|p| json!({"id": p.id, "name": p.name, "group": p.group, "favorite": favorite(&p.id), "imported": true}));
            Ok(Value::Array(builtin.chain(imported).collect()))
        }),
        cmd!(query "profiles.menu", "Profile Menu", [], None, "{} — favourites, recent (newest first) and groups", always, |s, _| Ok(s.profile_menu())),
        cmd!(query "history.list", "List History", [], None, "{id?} — returns {history: labels oldest first, cursor: index of the step the settings are at (later steps can be returned to), versions}", has_active, |s, p| {
            let id = photo_arg(s, p, "history.list")?;
            let ph = s.catalog.photo(id).ok_or_else(|| bad("history.list", "no such photo"))?;
            Ok(json!({
                "history": ph.history.iter().map(|h| h.label.clone()).collect::<Vec<_>>(),
                "cursor": ph.history_index(),
                "versions": ph.versions.iter().map(|v| json!({"name": v.name, "created": v.created})).collect::<Vec<_>>(),
            }))
        }),
        cmd!(query "app.gpu", "GPU Rendering", [], None, "{enabled?: bool} — allow/forbid GPU rendering (CPU fallback; LIGHTCRAFT_GPU=0 forbids it for the process); returns {enabled, available, adapter, reason (why the GPU is off), lastFallback (latest render redone on the CPU, and why)}", always, |_, p| {
            if let Some(on) = p.get("enabled").and_then(Value::as_bool) {
                lightcraft_gpu::set_enabled(on);
            }
            Ok(json!({
                "enabled": lightcraft_gpu::enabled(),
                "available": lightcraft_gpu::available(),
                "adapter": lightcraft_gpu::adapter_name(),
                "reason": lightcraft_gpu::unavailable_reason(),
                "lastFallback": lightcraft_gpu::last_fallback(),
            }))
        }),
        cmd!(query "library.memory", "Memory Usage", [], None, "{} — bytes held by each cache (decoded sources, rendered previews, GPU buffers; heap when instrumented)", always, |s, _| {
            Ok(serde_json::to_value(s.memory_report()).unwrap_or_default())
        }),
        cmd!(query "app.memoryBudget", "Memory Budget", [], None, "{mb?: number} — set the memory budget shared by the caches (default: a quarter of RAM, at most 1536 MB); returns the report", always, |s, p| {
            if let Some(mb) = p.get("mb").and_then(Value::as_f64) {
                if !(64.0..=1_048_576.0).contains(&mb) {
                    return Err(bad("app.memoryBudget", "mb must be 64..1048576"));
                }
                s.set_memory_budget((mb * 1048576.0) as usize);
            }
            Ok(serde_json::to_value(s.memory_report()).unwrap_or_default())
        }),
        cmd!(query "journal.list", "Command Journal", [], None, "{limit?}", always, |s, p| {
            let lim = p.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
            let start = s.journal.len().saturating_sub(lim);
            Ok(Value::Array(s.journal[start..].iter().map(|(c, p)| json!({"command": c, "params": p})).collect()))
        }),
    ]
}

#[cfg(test)]
mod pagination_tests {
    use super::*;

    #[test]
    fn offsets_and_limits_saturate_instead_of_wrapping_on_wasm() {
        assert_eq!(pagination_usize(0), 0);
        assert_eq!(pagination_usize(200), 200);
        assert_eq!(pagination_usize(u64::MAX), usize::MAX);
        #[cfg(target_pointer_width = "32")]
        assert_eq!(pagination_usize(u64::from(u32::MAX) + 1), usize::MAX);
    }
}
