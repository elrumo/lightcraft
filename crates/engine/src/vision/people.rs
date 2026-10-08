//! People: the faces found in the library, grouped by who they look like, so a person can be named
//! once for all their photos.
//!
//! The faces themselves are derived data in the library's `search/` folder (see [`super`]); a
//! name is not: naming a person writes an ordinary face region (the MWG-RS kind the People view
//! already reads from XMP) on each photo they are in, in one undoable step, so everything that
//! already works with named people — the People view, the `person:` search, the filter chip — works
//! with them, and the names sync with the catalog like any other edit. Unnamed people exist only in
//! the face index.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_meta::{Region, RegionKind};
use lightcraft_vision::Key;
use lightcraft_vision::faces::cluster::{self, JOIN};
use lightcraft_vision::faces::index::FaceIndex;
use serde_json::{Value, json};

use super::{FaceSpec, with_faces};
use crate::Session;

/// A region this much like a face (intersection over union) is that face.
const SAME_FACE: f32 = 0.4;
/// Photos listed for a person.
const PHOTOS_LISTED: usize = 12;
/// Longest name accepted, in characters.
pub const MAX_NAME: usize = 120;

/// A face of a person.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceRef {
    pub key: Key,
    pub index: u8,
    /// `[x0, y0, x1, y1]`, normalized to the upright photo.
    pub rect: [f32; 4],
    pub score: f32,
}

/// The faces that look like one person.
#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    /// Names the person: the id of their best face (`<photo key>.<face number>`).
    pub id: String,
    /// The best face first.
    pub members: Vec<FaceRef>,
    pub centroid: Vec<f32>,
}

/// The people the face index makes, kept while the index doesn't change.
#[derive(Default)]
pub(super) struct Cache {
    /// What the index looked like when `clusters` was made: (faces, photos, revision).
    at: Option<(usize, usize, u64)>,
    clusters: Arc<Vec<Cluster>>,
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let (w, h) = ((a[2].min(b[2]) - a[0].max(b[0])).max(0.0), (a[3].min(b[3]) - a[1].max(b[1])).max(0.0));
    let inter = w * h;
    let union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

fn region_rect(r: &Region) -> [f32; 4] {
    [r.rect.x0 as f32, r.rect.y0 as f32, r.rect.x1 as f32, r.rect.y1 as f32]
}

/// The name on the face region of `photo` that is `face`, if there is one (and a region at all).
fn region_at(regions: &[Region], face: &[f32; 4]) -> Option<usize> {
    regions.iter().position(|r| r.kind == RegionKind::Face && iou(&region_rect(r), face) >= SAME_FACE)
}

fn name_of(r: &Region) -> Option<&str> {
    r.name.as_deref().map(str::trim).filter(|n| !n.is_empty())
}

/// A clean name, or why it can't be one.
fn clean_name(name: &str) -> Result<String, String> {
    let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err("give the person a name".into());
    }
    if name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(format!("a name is at most {MAX_NAME} characters, without control characters"));
    }
    Ok(name)
}

/// The people the faces of `ix` make (best face first in each; biggest first).
pub fn clusters_of(ix: &FaceIndex) -> Vec<Cluster> {
    let faces = ix.faces();
    cluster::group(faces, JOIN)
        .into_iter()
        .filter_map(|g| {
            let members: Vec<FaceRef> = g
                .members
                .iter()
                .filter_map(|&i| faces.get(i))
                .map(|f| FaceRef { key: f.key, index: f.index, rect: f.rect, score: f.score })
                .collect();
            let first = g.members.first().and_then(|&i| faces.get(i))?;
            Some(Cluster { id: cluster::face_id(first), members, centroid: g.centroid })
        })
        .collect()
}

impl Session {
    /// Where the face index is: the models that made it (known without loading them).
    fn faces_spec(&self) -> FaceSpec {
        let engine = match &self.vision.injected_finder {
            Some(f) => f.engine().to_string(),
            None => lightcraft_vision::faces::ENGINE.to_string(),
        };
        FaceSpec { path: self.vision_faces_path(&engine), engine }
    }

    /// The people come from the sync server's faces when this device finds none itself (the web
    /// build, iOS, a computer that didn't turn finding people on) and the server does.
    fn people_from_server(&self) -> bool {
        let local = self.vision.faces_available() && (self.vision.faces || self.vision.faces_found() > 0);
        !local && self.vision.remote.faces_enabled()
    }

    /// The people the faces make (cached while the faces don't change). `wait`: when they come
    /// from the server, ask it now and wait for the answer.
    pub fn people_clusters(&mut self, wait: bool) -> Result<Arc<Vec<Cluster>>, String> {
        if self.people_from_server() {
            self.vision_fetch_people(wait);
            return Ok(self.vision.remote.people.as_ref().map(|(_, c)| c.clone()).unwrap_or_default());
        }
        let spec = self.faces_spec();
        let shared = self.vision.shared.clone();
        let at = with_faces(&shared, &spec, |ix| (ix.len(), ix.photos(), ix.revision()))?;
        if self.vision.people.at == Some(at) {
            return Ok(self.vision.people.clusters.clone());
        }
        let clusters = with_faces(&shared, &spec, |ix| clusters_of(ix))?;
        self.vision.people.at = Some(at);
        self.vision.people.clusters = Arc::new(clusters);
        Ok(self.vision.people.clusters.clone())
    }

    /// The name each of `cluster`'s faces already has on its photos' face regions, most common
    /// first, and how many faces have none yet.
    fn people_names(&self, cluster: &Cluster, photos: &HashMap<Key, Vec<PhotoId>>) -> (Vec<(String, usize)>, usize) {
        let mut votes: Vec<(String, usize)> = Vec::new();
        let mut pending = 0;
        for m in &cluster.members {
            let mut named = false;
            for id in photos.get(&m.key).into_iter().flatten() {
                let Some(p) = self.catalog.photo(*id) else { continue };
                if let Some(r) = region_at(&p.meta.regions, &m.rect).and_then(|i| p.meta.regions.get(i))
                    && let Some(n) = name_of(r)
                {
                    named = true;
                    match votes.iter_mut().find(|(v, _)| v.eq_ignore_ascii_case(n) || v.to_lowercase() == n.to_lowercase()) {
                        Some((_, c)) => *c += 1,
                        None => votes.push((n.to_string(), 1)),
                    }
                    break;
                }
            }
            if !named {
                pending += 1;
            }
        }
        votes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        (votes, pending)
    }

    /// The people found in the photos (`people.list`): the ones nobody has named, and the named
    /// ones with faces that aren't confirmed yet (`all`: every one). Biggest first.
    pub fn people_list(&mut self, all: bool, limit: usize, wait: bool) -> Result<Value, String> {
        let clusters = self.people_clusters(wait)?;
        let photos = self.vision_keys();
        let mut out = Vec::new();
        let mut people = 0;
        for c in clusters.iter() {
            let (names, pending) = self.people_names(c, &photos);
            let name = names.first().map(|(n, _)| n.clone());
            people += 1;
            if !all && name.is_some() && pending == 0 {
                continue;
            }
            if out.len() >= limit {
                continue;
            }
            let mut ids: Vec<PhotoId> = c.members.iter().flat_map(|m| photos.get(&m.key).into_iter().flatten().copied()).collect();
            ids.sort();
            ids.dedup();
            let Some(best) = c.members.first() else { continue };
            let Some(cover) = photos.get(&best.key).and_then(|p| p.first()) else { continue };
            out.push(json!({
                "id": c.id,
                "name": name,
                "faces": c.members.len(),
                "photos": ids.len(),
                "pending": pending,
                "cover": {"photo": cover.0, "rect": {"x0": best.rect[0], "y0": best.rect[1], "x1": best.rect[2], "y1": best.rect[3]}},
                "photoIds": ids.iter().take(PHOTOS_LISTED).map(|i| i.0).collect::<Vec<_>>(),
            }));
        }
        Ok(json!({
            "people": out,
            "totalPeople": people,
            "source": if self.people_from_server() { "server" } else { "local" },
            "faces": self.vision.faces_found(),
            "scanned": self.vision.faces_scanned(),
            "libraryPhotos": self.vision_photo_count(),
        }))
    }

    /// Finds `cluster` among the people, or says to look again (people are regrouped as photos are
    /// added, so an id can go away).
    fn people_cluster(&mut self, id: &str) -> Result<Cluster, String> {
        let clusters = self.people_clusters(false)?;
        clusters.iter().find(|c| c.id == id).cloned().ok_or_else(|| "that person is no longer there: look at the people again".to_string())
    }

    /// Names a person (`people.name`): their faces become face regions with that name on each of
    /// their photos, in one undoable step. A face that already has another name keeps it. Naming
    /// a person that is already named confirms the faces added since.
    pub fn people_name(&mut self, cluster: &str, name: &str) -> Result<Value, String> {
        let name = clean_name(name)?;
        let c = self.people_cluster(cluster)?;
        let photos = self.vision_keys();
        // (a photo with several of this person's faces is edited once, with all of them)
        let mut edited: HashMap<PhotoId, lightcraft_catalog::Meta> = HashMap::new();
        let (mut named, mut kept) = (0, 0);
        for m in &c.members {
            for id in photos.get(&m.key).into_iter().flatten() {
                let Some(p) = self.catalog.photo(*id) else { continue };
                let meta = edited.entry(*id).or_insert_with(|| p.meta.clone());
                match region_at(&meta.regions, &m.rect) {
                    Some(i) => {
                        let Some(r) = meta.regions.get_mut(i) else { continue };
                        match name_of(r) {
                            Some(n) if n.to_lowercase() == name.to_lowercase() => continue,
                            Some(_) => {
                                kept += 1;
                                continue;
                            }
                            None => r.name = Some(name.clone()),
                        }
                    }
                    None => {
                        let r = lightcraft_geom::Rect {
                            x0: f64::from(m.rect[0]),
                            y0: f64::from(m.rect[1]),
                            x1: f64::from(m.rect[2]),
                            y1: f64::from(m.rect[3]),
                        };
                        meta.regions.push(Region { rect: r, kind: RegionKind::Face, name: Some(name.clone()), description: None });
                    }
                }
                named += 1;
            }
        }
        let mut ops: Vec<Op> = Vec::new();
        let mut touched: HashSet<PhotoId> = HashSet::new();
        for (id, meta) in edited {
            // (only photos that changed)
            if self.catalog.photo(id).is_some_and(|p| p.meta != meta) {
                touched.insert(id);
                ops.push(Op::SetMeta { id, meta: Box::new(meta) });
            }
        }
        // (the same order whatever the map's: undo and the log see one batch)
        ops.sort_by_key(|op| if let Op::SetMeta { id, .. } = op { id.0 } else { 0 });
        if !ops.is_empty() {
            self.commit("Name People", Op::Batch { ops }).map_err(|e| e.to_string())?;
            // (regions are catalog data: the sidecar is never rewritten for them)
            self.skip_auto_write = true;
        }
        Ok(json!({"name": name, "faces": named, "photos": touched.len(), "keptOtherName": kept}))
    }

    /// Shows a person's photos (`people.show`): the view becomes those photos, oldest first.
    pub fn people_show(&mut self, cluster: &str) -> Result<Value, String> {
        let c = self.people_cluster(cluster)?;
        let photos = self.vision_keys();
        let (names, _) = self.people_names(&c, &photos);
        let mut ids: Vec<PhotoId> = c.members.iter().flat_map(|m| photos.get(&m.key).into_iter().flatten().copied()).collect();
        ids.sort();
        ids.dedup();
        ids.sort_by_key(|id| self.catalog.photo(*id).map(|p| (p.captured.clone(), p.id)));
        let label = names.first().map_or_else(|| "an unnamed person".to_string(), |(n, _)| n.clone());
        self.filter.only = ids.clone();
        self.filter.semantic = Some(label.clone());
        Ok(json!({"person": label, "photos": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))
    }

    /// Forgets every face found (`people.deleteData`): the face index, here, and what the sync server found. Names already written
    /// to photos stay (they are catalog data; remove them with `photo.removeRegion`).
    pub fn people_delete_data(&mut self, wait: bool) -> Result<Value, String> {
        let spec = self.faces_spec();
        let shared = self.vision.shared.clone();
        let (faces, photos) = with_faces(&shared, &spec, |ix| (ix.len(), ix.photos()))?;
        with_faces(&shared, &spec, |ix| ix.clear().map_err(|e| e.to_string()))??;
        self.vision.people = Default::default();
        // (and what the sync server found for this user, when it finds any)
        let server = self.vision_forget_remote_faces(wait);
        Ok(json!({"faces": faces, "photos": photos, "server": server}))
    }
}
