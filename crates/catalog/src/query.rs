//! Filtering, search and sorting.

use serde::{Deserialize, Serialize};

use crate::search::TextQuery;
use crate::{AlbumId, Catalog, ColorLabel, Flag, MediaKind, Photo, PhotoId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RatingOp {
    #[default]
    AtLeast,
    Exactly,
    AtMost,
}

/// What the grid shows. Empty/None fields don't filter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Filter {
    /// Free-text search: every word must match filename, title, caption, keywords, camera, lens,
    /// place fields or format; place names and dates are understood in context ("photos in madrid
    /// in june", see [`crate::search`]). Tokens like `rating:3`, `flag:pick`, `iso:>800`,
    /// `camera:x2`, `place:madrid`, `near:40.4,-3.7,5` are fielded.
    pub text: String,
    pub rating: u8,
    pub rating_op: RatingOp,
    pub flag: Option<Flag>,
    pub label: Option<ColorLabel>,
    /// Only these photos (Find Similar results…); empty = no constraint.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<PhotoId>,
    /// The description whose best matches `only` holds, best first (search by description). Unlike
    /// a plain `only`, an empty `only` here means "no matches", and the view shows them in `only`'s
    /// order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic: Option<String>,
    /// Any of these labels (the filter bar's multi-select); empty = no constraint.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<ColorLabel>,
    pub kind: Option<MediaKind>,
    pub edited: Option<bool>,
    pub album: Option<AlbumId>,
    /// Show "Recently Deleted" instead of the library.
    pub deleted: bool,
    /// Capture date prefix (`2026`, `2026-04`, `2026-04-12`).
    pub date: Option<String>,
    /// A folder the library's photos were imported from: those photos and the ones imported from
    /// the folders inside it (see [`crate::folders`]). Photos only browsed in Local are never part
    /// of it, unlike [`Filter::folder`].
    pub library_folder: Option<String>,
    /// A keyword; hierarchical keywords match their children too (`travel` finds `travel|italy`).
    pub keyword: Option<String>,
    /// A person: photos with a named face region of this name (case-insensitive), as read from XMP.
    pub person: Option<String>,
    pub camera: Option<String>,
    /// Lens (case-insensitive substring).
    pub lens: Option<String>,
    /// Capture date range, inclusive: `dateFrom` is compared as a lower bound (`2026-04-01`),
    /// `dateTo` as a prefix upper bound (`2026-04` includes all of April).
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    /// Import date prefix.
    pub imported: Option<String>,
    /// Imported at or after this time (ISO; Recently Added).
    pub imported_from: Option<String>,
    /// Photo Merge results: `hdr`, `panorama`, `hdrPanorama` or `any` (see [`merged_kind`]).
    pub merged: Option<String>,
    /// A folder on disk: its files only (browsed ones too); `subfolders` includes everything below.
    pub folder: Option<String>,
    pub subfolders: bool,
    /// A folder of the user's library folders on the sync server: the photos in it and in the
    /// folders below (by [`Photo::server_path`]; `""` = every photo kept in a library folder).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_folder: Option<String>,
    /// Smart-album rules (all / any / none, nested groups; see [`crate::rules`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_set: Option<crate::RuleSet>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    #[default]
    CaptureDate,
    ImportDate,
    EditDate,
    FileName,
    Rating,
    FileSize,
    /// A shuffle fixed by [`Sort::seed`]: the same seed always gives the same order, and photos
    /// added or edited later never reshuffle the others. Pick a new seed to reshuffle.
    Random,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sort {
    pub key: SortKey,
    pub ascending: bool,
    /// Date headers in the grid (date sort keys only).
    pub group: crate::GroupBy,
    /// Which shuffle [`SortKey::Random`] gives (ignored by the other keys).
    pub seed: u64,
}

impl Default for Sort {
    fn default() -> Self {
        Sort { key: SortKey::CaptureDate, ascending: false, group: crate::GroupBy::Auto, seed: 0 }
    }
}

/// splitmix64 finaliser: a stateless, platform-independent mix (no RNG state, no `rand` version
/// to drift), so a seed names the same shuffle on every machine.
pub fn mix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A photo's place in the shuffle for `seed`.
fn shuffle_rank(seed: u64, id: PhotoId) -> u64 {
    mix64(mix64(seed) ^ id.0)
}

impl Filter {
    /// Human-readable summary of the active rules (smart album tooltips, `album.list`).
    pub fn describe(&self) -> String {
        self.describe_in(None)
    }

    /// [`Filter::describe`] naming the albums its rules test (`RuleSet::describe_with`).
    pub fn describe_with(&self, cat: &crate::Catalog) -> String {
        self.describe_in(Some(cat))
    }

    fn describe_in(&self, cat: Option<&crate::Catalog>) -> String {
        let mut v: Vec<String> = Vec::new();
        if self.rating > 0 {
            let op = match self.rating_op {
                RatingOp::AtLeast => "≥",
                RatingOp::Exactly => "=",
                RatingOp::AtMost => "≤",
            };
            v.push(format!("rating {op} {}", self.rating));
        }
        if let Some(f) = self.flag {
            v.push(format!("flag {}", format!("{f:?}").to_lowercase()));
        }
        if let Some(l) = self.label {
            v.push(format!("label {}", format!("{l:?}").to_lowercase()));
        }
        if !self.labels.is_empty() {
            v.push(format!("label {}", self.labels.iter().map(|l| format!("{l:?}").to_lowercase()).collect::<Vec<_>>().join(" or ")));
        }
        if let Some(k) = self.kind {
            v.push(format!("kind {}", format!("{k:?}").to_lowercase()));
        }
        if let Some(e) = self.edited {
            v.push(if e { "edited".into() } else { "unedited".into() });
        }
        for (name, val) in [
            ("keyword", &self.keyword),
            ("person", &self.person),
            ("camera", &self.camera),
            ("lens", &self.lens),
            ("date", &self.date),
            ("folder", &self.library_folder),
            ("imported", &self.imported),
        ] {
            if let Some(x) = val {
                v.push(format!("{name} {x}"));
            }
        }
        match (&self.date_from, &self.date_to) {
            (Some(a), Some(b)) => v.push(format!("captured {a} – {b}")),
            (Some(a), None) => v.push(format!("captured from {a}")),
            (None, Some(b)) => v.push(format!("captured until {b}")),
            _ => {}
        }
        if !self.text.trim().is_empty() {
            v.push(format!("“{}”", self.text.trim()));
        }
        if let Some(q) = &self.semantic {
            v.push(format!("looks like “{}”", q.trim()));
        }
        if let Some(rs) = self.rule_set.as_ref().filter(|r| !r.rules.is_empty()) {
            v.push(cat.map_or_else(|| rs.describe(), |c| rs.describe_with(c)));
        }
        if v.is_empty() { "all photos".into() } else { v.join(", ") }
    }

    /// The albums this filter tests: its album field and its rules' Album rules.
    pub fn albums_tested(&self) -> Vec<crate::AlbumId> {
        let mut out: Vec<crate::AlbumId> = self.album.into_iter().collect();
        out.extend(self.rule_set.as_ref().map(crate::RuleSet::albums_tested).unwrap_or_default());
        out
    }

    /// Whether matches depend on the clock (only "in the last…" rules do).
    pub fn depends_on_now(&self) -> bool {
        self.rule_set.as_ref().is_some_and(crate::RuleSet::depends_on_now)
    }

    /// Whether `p` passes. Reads the search text each call: [`Catalog::query`] and
    /// [`Catalog::count`] read it once for all photos.
    pub fn matches(&self, p: &Photo, cat: &Catalog) -> bool {
        self.matches_in(p, cat, self.library_root().as_deref(), &TextQuery::parse(&self.text))
    }

    /// [`Filter::library_folder`] as a folder identity ([`folder_key`]); `None` without a choice.
    fn library_root(&self) -> Option<String> {
        // as given: a folder may be named with spaces at its edge, and trimming would make it
        // another folder
        self.library_folder.as_deref().filter(|d| !d.trim().is_empty()).map(folder_key)
    }

    /// [`Filter::matches`] with the library folder already turned into its identity and the search
    /// text compiled from `self.text`, so a query over a big library does that once, not once per
    /// photo.
    fn matches_in(&self, p: &Photo, cat: &Catalog, root: Option<&str>, text: &TextQuery) -> bool {
        if p.deleted != self.deleted {
            return false;
        }
        match &self.folder {
            // browsed photos only show in folder views
            None if p.local => return false,
            None => {}
            Some(dir) => {
                let crate::Source::File { path } = &p.source else { return false };
                if !in_folder(path, dir, self.subfolders) {
                    return false;
                }
            }
        }
        if let Some(dir) = &self.server_folder
            && !p.server_path.as_deref().is_some_and(|path| in_server_folder(path, dir))
        {
            return false;
        }
        if self.rating > 0 {
            let ok = match self.rating_op {
                RatingOp::AtLeast => p.rating >= self.rating,
                RatingOp::Exactly => p.rating == self.rating,
                RatingOp::AtMost => p.rating <= self.rating,
            };
            if !ok {
                return false;
            }
        }
        if self.flag.is_some_and(|f| f != p.flag) || self.label.is_some_and(|l| Some(l) != p.label) || self.kind.is_some_and(|k| k != p.kind) {
            return false;
        }
        if self.edited.is_some_and(|e| e != p.is_edited()) {
            return false;
        }
        if let Some(rs) = &self.rule_set
            && !rs.matches(p, cat)
        {
            return false;
        }
        if !self.labels.is_empty() && !p.label.is_some_and(|l| self.labels.contains(&l)) {
            return false;
        }
        if (self.semantic.is_some() || !self.only.is_empty()) && !self.only.contains(&p.id) {
            return false;
        }
        if let Some(want) = &self.merged {
            match merged_kind(&p.file_name) {
                Some(k) if want == "any" || want == k => {}
                _ => return false,
            }
        }
        if let Some(a) = self.album
            && !cat.album_contains(a, p)
        {
            return false;
        }
        if let Some(from) = &self.date_from
            && match p.captured.as_deref() {
                Some(c) => c < from.as_str(),
                None => true,
            }
        {
            return false;
        }
        if let Some(to) = &self.date_to
            && match p.captured.as_deref() {
                Some(c) => c.get(..to.len()).unwrap_or(c) > to.as_str(),
                None => true,
            }
        {
            return false;
        }
        if let Some(l) = &self.lens
            && !p.meta.lens.to_lowercase().contains(&l.to_lowercase())
        {
            return false;
        }
        if let Some(d) = &self.date
            && !p.captured.as_deref().is_some_and(|c| c.starts_with(d.as_str()))
        {
            return false;
        }
        if let Some(root) = root
            && !photo_in_root(p, root)
        {
            return false;
        }
        if let Some(d) = &self.imported
            && !p.imported.starts_with(d.as_str())
        {
            return false;
        }
        if let Some(d) = &self.imported_from
            && p.imported.as_str() < d.as_str()
        {
            return false;
        }
        if let Some(k) = &self.keyword
            && !p.meta.keywords.iter().any(|x| crate::keywords::is_under(x, k))
        {
            return false;
        }
        if let Some(n) = &self.person
            && !crate::search::has_person(p, n)
        {
            return false;
        }
        if let Some(c) = &self.camera
            && !p.meta.camera.eq_ignore_ascii_case(c)
        {
            return false;
        }
        text.matches(p)
    }
}

impl Catalog {
    /// Photos matching `filter`, in `sort` order (ties broken by id for stability).
    pub fn query(&self, filter: &Filter, sort: &Sort) -> Vec<PhotoId> {
        let text = TextQuery::parse(&filter.text);
        let root = filter.library_root();
        let mut v: Vec<&Photo> = self.photos().map(|p| p.as_ref()).filter(|p| filter.matches_in(p, self, root.as_deref(), &text)).collect();
        v.sort_by(|a, b| {
            let o = match sort.key {
                SortKey::CaptureDate => a.captured.cmp(&b.captured).then_with(|| a.imported.cmp(&b.imported)),
                SortKey::ImportDate => a.imported.cmp(&b.imported),
                SortKey::EditDate => a.edited.cmp(&b.edited),
                SortKey::FileName => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
                SortKey::Rating => a.rating.cmp(&b.rating),
                SortKey::FileSize => a.file_size.cmp(&b.file_size),
                SortKey::Random => shuffle_rank(sort.seed, a.id).cmp(&shuffle_rank(sort.seed, b.id)),
            }
            .then(a.id.cmp(&b.id));
            if sort.ascending { o } else { o.reverse() }
        });
        v.into_iter().map(|p| p.id).collect()
    }

    /// How many photos [`Catalog::query`] lists, without sorting them.
    pub fn count(&self, filter: &Filter) -> usize {
        let text = TextQuery::parse(&filter.text);
        let root = filter.library_root();
        self.photos().filter(|p| filter.matches_in(p, self, root.as_deref(), &text)).count()
    }

    /// The places and dates the search text of `filter` was read as ("photos in madrid in june"),
    /// for the interface to say so; see [`crate::search::Understood`].
    pub fn understood(&self, filter: &Filter) -> Vec<crate::search::Understood> {
        if filter.text.trim().is_empty() {
            return vec![];
        }
        let photos: Vec<&Photo> = self.photos().map(|p| p.as_ref()).filter(|p| p.in_library()).collect();
        TextQuery::parse(&filter.text).understood(&photos)
    }

    /// Year → month → day counts for the "By Date" section (newest first).
    pub fn date_groups(&self) -> Vec<DateGroup> {
        use std::collections::BTreeMap;
        let mut years: BTreeMap<&str, (BTreeMap<&str, usize>, BTreeMap<&str, usize>)> = BTreeMap::new();
        for p in self.photos().filter(|p| p.in_library()) {
            let Some(d) = p.captured.as_deref() else { continue };
            if d.len() >= 10 {
                let (months, days) = years.entry(&d[..4]).or_default();
                *months.entry(&d[..7]).or_default() += 1;
                *days.entry(&d[..10]).or_default() += 1;
            }
        }
        let newest_first = |m: BTreeMap<&str, usize>| m.into_iter().rev().map(|(k, n)| (k.to_string(), n)).collect::<Vec<_>>();
        years
            .into_iter()
            .rev()
            .map(|(year, (months, days))| DateGroup {
                year: year.to_string(),
                count: months.values().sum(),
                months: newest_first(months),
                days: newest_first(days),
            })
            .collect()
    }

    /// All keywords with usage counts, sorted by name.
    pub fn keywords(&self) -> Vec<(String, usize)> {
        let mut m: std::collections::BTreeMap<String, usize> = Default::default();
        for p in self.photos().filter(|p| p.in_library()) {
            for k in &p.meta.keywords {
                *m.entry(k.clone()).or_default() += 1;
            }
        }
        m.into_iter().collect()
    }

    /// The people named on faces in the library (MWG regions read from XMP): how many photos each
    /// appears in and the photo showing their largest face (for a card's picture). Most photos first,
    /// then by name. Names that differ only in case are one person, shown as first seen; a person
    /// twice in one photo counts once.
    pub fn people(&self) -> Vec<Person> {
        self.people_in(&Filter::default())
    }

    /// [`Self::people`] among the photos `filter` lets through (its `person` is ignored): what the
    /// People view offers while other filters (a date, a rating, an album…) are active, so picking
    /// a person never ends in an empty grid.
    pub fn people_in(&self, filter: &Filter) -> Vec<Person> {
        let filter = Filter { person: None, ..filter.clone() };
        let mut m: std::collections::HashMap<String, (Person, f64)> = Default::default();
        let text = TextQuery::parse(&filter.text);
        let root = filter.library_root();
        for p in self.photos().filter(|p| filter.matches_in(p, self, root.as_deref(), &text)) {
            let mut seen: Vec<String> = Vec::new();
            for r in p.meta.regions.iter().filter(|r| r.kind == lightcraft_meta::RegionKind::Face) {
                let Some(name) = r.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) else { continue };
                let key = name.to_lowercase();
                // the face's size in pixels of the photo
                let area = (r.rect.x1 - r.rect.x0) * (r.rect.y1 - r.rect.y0) * p.width as f64 * p.height as f64;
                let (person, best) =
                    m.entry(key.clone()).or_insert_with(|| (Person { name: name.to_string(), count: 0, photo: p.id, face: r.rect }, area));
                if !seen.contains(&key) {
                    person.count += 1;
                    seen.push(key);
                }
                if area > *best || (area == *best && p.id < person.photo) {
                    (person.photo, person.face, *best) = (p.id, r.rect, area);
                }
            }
        }
        let mut v: Vec<Person> = m.into_values().map(|(p, _)| p).collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        v
    }
}

/// A person named on faces in the library ([`Catalog::people`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub name: String,
    /// Photos they appear in.
    pub count: usize,
    /// The photo with their largest face, and that face (normalized, in the photo's upright frame).
    pub photo: PhotoId,
    pub face: lightcraft_meta::Rect,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DateGroup {
    pub year: String,
    pub count: usize,
    /// `YYYY-MM` and counts, newest first.
    pub months: Vec<(String, usize)>,
    /// `YYYY-MM-DD` and counts, newest first.
    pub days: Vec<(String, usize)>,
}

/// The folders of the user's library folders on the sync server, each with how many photos are
/// in it or below (Recently Deleted and Local records left out), by path.
pub fn server_folders(cat: &Catalog) -> std::collections::BTreeMap<String, usize> {
    let mut out = std::collections::BTreeMap::new();
    for dir in cat.photos().filter(|p| !p.deleted && !p.local).filter_map(|p| p.server_folder()) {
        let mut end = Some(dir.len());
        while let Some(e) = end {
            let Some(d) = dir.get(..e).filter(|d| !d.is_empty()) else { break };
            *out.entry(d.to_string()).or_insert(0) += 1;
            end = d.rfind('/');
        }
    }
    out
}

/// Whether a [`Photo::server_path`] lies in server folder `dir` (or below it; `""`: anywhere).
pub fn in_server_folder(path: &str, dir: &str) -> bool {
    let dir = dir.trim_matches('/');
    dir.is_empty() || path.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))
}

/// Whether file `path` is directly in `dir` (or anywhere below it with `deep`). Both `/` and `\\`
/// separate.
pub fn in_folder(path: &str, dir: &str, deep: bool) -> bool {
    let dir = dir.trim_end_matches(['/', '\\']);
    let Some(rest) = path.strip_prefix(dir) else { return false };
    let Some(rest) = rest.strip_prefix(['/', '\\']) else { return false };
    !rest.is_empty() && (deep || !rest.contains(['/', '\\']))
}

/// A path read the way [`folder_key`] reads it: the part `..` can't climb above (`//server/share`,
/// `c:`; empty for `/` and relative paths), the names below it, and whether it is absolute.
pub(crate) struct SplitPath {
    pub prefix: String,
    pub parts: Vec<String>,
    pub absolute: bool,
}

/// Both separators, repeated separators (also between a server and its share), `.` and a
/// trailing separator dropped, `..` resolved lexically, a Windows verbatim prefix removed;
/// `lower_drive` lower-cases a drive letter (the identity does, a display name wants it as is).
pub(crate) fn split_path(path: &str, lower_drive: bool) -> SplitPath {
    let mut s = path.replace('\\', "/");
    if let Some(rest) = s.strip_prefix("//?/").or_else(|| s.strip_prefix("//./")) {
        s = match rest.strip_prefix("UNC/") {
            Some(unc) => format!("//{unc}"),
            None => rest.to_string(),
        };
    }
    let (prefix, rest) = if let Some(unc) = s.strip_prefix("//").filter(|u| !u.is_empty() && !u.starts_with('/')) {
        let mut segs = unc.split('/').filter(|x| !x.is_empty());
        let (server, share) = (segs.next().unwrap_or(""), segs.next().unwrap_or(""));
        (format!("//{server}/{share}"), segs.collect::<Vec<_>>().join("/"))
    } else if let (true, Some(drive), Some(rest)) = (matches!(s.as_bytes(), [a, b':', ..] if a.is_ascii_alphabetic()), s.get(..2), s.get(2..)) {
        (if lower_drive { drive.to_ascii_lowercase() } else { drive.to_string() }, rest.to_string())
    } else {
        (String::new(), s.clone())
    };
    let absolute = rest.starts_with('/') || !prefix.is_empty();
    let mut parts: Vec<String> = Vec::new();
    for c in rest.split('/') {
        match c {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| p != "..") => {
                parts.pop();
            }
            ".." if absolute => {}
            c => parts.push(c.to_string()),
        }
    }
    SplitPath { prefix, parts, absolute }
}

/// A folder path's identity, for telling whether two spellings name the same folder: `/` and
/// `\\` are both separators, repeated separators, `.` and a trailing separator are dropped,
/// `..` is resolved lexically, a Windows verbatim prefix (`\\?\`) is removed and the drive
/// letter lower-cased; on Windows (case-insensitive file names) the whole path is lower-cased.
/// File names are otherwise compared as written, so on a case-insensitive disk elsewhere
/// (macOS, a Samba share) two spellings that differ in case are two folders here.
/// No file-system access: the paths compared should already be absolute.
pub fn folder_key(path: &str) -> String {
    let SplitPath { prefix, parts, absolute } = split_path(path, true);
    let key = if absolute { format!("{prefix}/{}", parts.join("/")) } else { parts.join("/") };
    if cfg!(windows) { key.to_lowercase() } else { key }
}

/// Whether `path` is folder `root` or lies somewhere inside it, however either is spelled
/// (see [`folder_key`]).
pub fn folder_within(path: &str, root: &str) -> bool {
    key_within(&folder_key(path), &folder_key(root))
}

/// The names below folder `root` that lead to `path`, as the caller spelled them (`None` when
/// `path` is not at or inside `root`; empty when it is `root`). Compared by [`folder_key`], but
/// the names keep their case, which the key lowers on Windows.
pub fn folder_rest(path: &str, root: &str) -> Option<Vec<String>> {
    if !key_within(&folder_key(path), &folder_key(root)) {
        return None;
    }
    let depth = split_path(root, false).parts.len();
    Some(split_path(path, false).parts.into_iter().skip(depth).collect())
}

/// Whether a library photo (not one only browsed in Local) lies in the folder whose
/// [`folder_key`] is `root`. A plain POSIX path (no `\`, `.`, `..`, repeated or trailing
/// separators) is its own identity, so it is read without building a key for it: the check runs
/// for every photo of the library on each refresh, and this keeps it cheap.
pub(crate) fn photo_in_root(p: &Photo, root: &str) -> bool {
    if p.local {
        return false;
    }
    let crate::Source::File { path } = &p.source else { return false };
    let plain = path.starts_with('/')
        && !path.starts_with("//")
        && !path.ends_with('/')
        && !path.contains(['\\'])
        && !path.contains("//")
        && !path.contains("/.");
    if !cfg!(windows) && plain && root.starts_with('/') && !root.starts_with("//") {
        let r = root.trim_end_matches('/');
        return path.strip_prefix(r).is_some_and(|rest| rest.starts_with('/'));
    }
    crate::local::folder_of(p).is_some_and(|f| key_within(&f, root))
}

/// [`folder_within`] for two paths already turned into their [`folder_key`].
pub(crate) fn key_within(p: &str, r: &str) -> bool {
    // `.` or `a/..` name no folder: not "everything"
    if r.is_empty() {
        return false;
    }
    // a disk's root key ends in `/` (`c:/`, `//srv/share/`), a folder's does not: same folder
    let (p, r) = (p.trim_end_matches('/'), r.trim_end_matches('/'));
    p == r || p.strip_prefix(r).is_some_and(|rest| rest.starts_with('/'))
}

/// The kind of Photo Merge result a file is, from the name merges give it (`IMG_1-HDR.dng`,
/// `IMG_1-Pano.dng`, `IMG_1-HDR-Pano.dng` — also the names other raw editors use):
/// `hdr`, `panorama` or `hdrPanorama`.
pub fn merged_kind(file_name: &str) -> Option<&'static str> {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s).to_ascii_lowercase();
    let stem = stem.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-' || c == ' ');
    if stem.ends_with("-hdr-pano") {
        Some("hdrPanorama")
    } else if stem.ends_with("-hdr") {
        Some("hdr")
    } else if stem.ends_with("-pano") {
        Some("panorama")
    } else {
        None
    }
}
