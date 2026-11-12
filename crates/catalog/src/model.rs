//! Catalog records.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, WbMode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PhotoId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AlbumId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StackId(pub u64);

/// A stack: photos grouped under a top photo (bursts, brackets, merge results with their sources).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    pub id: StackId,
    /// Members in stack order; `photos[0]` is the top of the stack. At least two.
    pub photos: Vec<PhotoId>,
    /// Collapsed stacks show only their top photo in the grid.
    #[serde(default)]
    pub collapsed: bool,
}

impl Stack {
    /// The top photo (`PhotoId(0)`, which no photo has, for an empty stack read from damaged or
    /// untrusted data).
    pub fn top(&self) -> PhotoId {
        self.photos.first().copied().unwrap_or(PhotoId(0))
    }
    pub fn position(&self, id: PhotoId) -> Option<usize> {
        self.photos.iter().position(|p| *p == id)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    #[default]
    Image,
    Raw,
    Video,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flag {
    #[default]
    None,
    Pick,
    Reject,
}

impl Flag {
    pub fn parse(s: &str) -> Option<Flag> {
        match s.to_ascii_lowercase().as_str() {
            "pick" | "picked" | "flagged" => Some(Flag::Pick),
            "reject" | "rejected" => Some(Flag::Reject),
            "none" | "unflagged" | "" => Some(Flag::None),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorLabel {
    Red,
    Yellow,
    Green,
    Blue,
    Purple,
}

impl ColorLabel {
    pub const ALL: [ColorLabel; 5] = [ColorLabel::Red, ColorLabel::Yellow, ColorLabel::Green, ColorLabel::Blue, ColorLabel::Purple];
    pub fn parse(s: &str) -> Option<ColorLabel> {
        ColorLabel::ALL.into_iter().find(|c| format!("{c:?}").eq_ignore_ascii_case(s))
    }
}

/// Where the pixels come from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    /// A file on disk (native) or in the browser's storage (web).
    File { path: String },
    /// A procedurally generated demo scene (by `lightcraft-scenes` id).
    Demo { scene: u32 },
}

/// Copyright status (IPTC / XMP Rights Management `xmpRights:Marked`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CopyrightStatus {
    /// Not stated (no `xmpRights:Marked`).
    #[default]
    Unknown,
    /// `xmpRights:Marked` = True.
    Copyrighted,
    /// `xmpRights:Marked` = False.
    PublicDomain,
}

impl CopyrightStatus {
    pub const ALL: [CopyrightStatus; 3] = [CopyrightStatus::Unknown, CopyrightStatus::Copyrighted, CopyrightStatus::PublicDomain];
    /// `unknown` / `copyrighted` / `publicDomain` (also `public domain`, `public-domain`).
    pub fn parse(s: &str) -> Option<CopyrightStatus> {
        match s.trim().to_ascii_lowercase().replace([' ', '-', '_'], "").as_str() {
            "unknown" | "" => Some(CopyrightStatus::Unknown),
            "copyrighted" => Some(CopyrightStatus::Copyrighted),
            "publicdomain" => Some(CopyrightStatus::PublicDomain),
            _ => None,
        }
    }
    /// The id used by commands (`photo.setMeta`'s `copyrightStatus`).
    pub fn id(self) -> &'static str {
        match self {
            CopyrightStatus::Unknown => "unknown",
            CopyrightStatus::Copyrighted => "copyrighted",
            CopyrightStatus::PublicDomain => "publicDomain",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            CopyrightStatus::Unknown => "Unknown",
            CopyrightStatus::Copyrighted => "Copyrighted",
            CopyrightStatus::PublicDomain => "Public Domain",
        }
    }
    /// As `xmpRights:Marked`: `Some(true)` copyrighted, `Some(false)` public domain.
    pub fn marked(self) -> Option<bool> {
        match self {
            CopyrightStatus::Unknown => None,
            CopyrightStatus::Copyrighted => Some(true),
            CopyrightStatus::PublicDomain => Some(false),
        }
    }
    pub fn from_marked(m: Option<bool>) -> CopyrightStatus {
        match m {
            None => CopyrightStatus::Unknown,
            Some(true) => CopyrightStatus::Copyrighted,
            Some(false) => CopyrightStatus::PublicDomain,
        }
    }
    pub fn is_unknown(&self) -> bool {
        *self == CopyrightStatus::Unknown
    }
}

/// A shutter speed as cameras write it (`1/250`, `0.5`, `2"`, `30s`) in seconds; `None` for
/// anything that isn't a positive, finite time.
pub fn parse_shutter_seconds(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches(['s', '"']).trim();
    match s.split_once('/') {
        Some((n, d)) => Some(n.trim().parse::<f64>().ok()? / d.trim().parse::<f64>().ok()?),
        None => s.parse().ok(),
    }
    .filter(|v: &f64| v.is_finite() && *v > 0.0)
}

/// Descriptive + capture metadata.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    pub camera: String,
    pub lens: String,
    pub focal_mm: Option<f32>,
    pub aperture: Option<f32>,
    pub shutter: String,
    pub iso: Option<u32>,
    /// Place (sublocation, e.g. a landmark).
    pub location: String,
    pub city: String,
    pub state: String,
    pub country: String,
    pub gps: Option<(f64, f64)>,
    pub title: String,
    pub caption: String,
    /// Accessibility text and long description.
    pub alt_text: String,
    pub extended_description: String,
    pub copyright: String,
    /// Copyright status, rights usage terms and copyright info URL (IPTC Core rights fields).
    #[serde(skip_serializing_if = "CopyrightStatus::is_unknown")]
    pub copyright_status: CopyrightStatus,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub usage_terms: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub copyright_url: String,
    pub creator: String,
    pub keywords: Vec<String>,
    /// Face/pet/focus regions read from XMP (MWG-RS), on the upright (EXIF-oriented) photo.
    /// Removing or resizing one edits the catalog only; LightCraft never writes regions to XMP.
    /// Left out of the catalog JSON when empty (most photos), so older catalogs read unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<lightcraft_meta::Region>,
    /// Where the camera focused (read from the file's maker notes at import or Reload from Disk; Sony
    /// bodies whose files have been checked), on the upright (EXIF-oriented) photo. Read-only: never
    /// written to XMP. Left out of the catalog JSON when unknown, so older catalogs read unchanged. Boxed: 104 B
    /// inline in every photo of a library otherwise, for the few bodies that record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<Box<lightcraft_meta::FocusPoint>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub name: String,
    pub created: String,
    #[serde(deserialize_with = "crate::snapload::develop", serialize_with = "crate::compact::develop_out")]
    pub settings: Arc<DevelopSettings>,
    #[serde(default)]
    pub auto: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryStep {
    pub label: String,
    #[serde(deserialize_with = "crate::snapload::develop", serialize_with = "crate::compact::develop_out")]
    pub settings: Arc<DevelopSettings>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Photo {
    pub id: PhotoId,
    pub source: Source,
    pub file_name: String,
    #[serde(default)]
    pub kind: MediaKind,
    pub format: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub file_size: u64,
    /// ISO 8601 local capture time.
    #[serde(default)]
    pub captured: Option<String>,
    pub imported: String,
    #[serde(default)]
    pub meta: Meta,
    #[serde(default)]
    pub rating: u8,
    #[serde(default)]
    pub flag: Flag,
    #[serde(default)]
    pub label: Option<ColorLabel>,
    #[serde(deserialize_with = "crate::snapload::develop", serialize_with = "crate::compact::develop_out")]
    pub develop: Arc<DevelopSettings>,
    #[serde(default)]
    pub edited: Option<String>,
    #[serde(default)]
    pub versions: Vec<Version>,
    #[serde(default)]
    pub history: Vec<HistoryStep>,
    /// The History step the photo's settings are at, when it isn't the latest one (Go to History
    /// Step moved back): later steps stay listed and can be returned to until a new edit drops
    /// them. `None` (and catalogs written before the cursor existed) = the latest step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_cursor: Option<usize>,
    /// In "Recently Deleted".
    #[serde(default)]
    pub deleted: bool,
    /// In "Hidden": left out of All Photos, albums, search and People until the user unlocks Hidden
    /// or turns on Show Hidden ([`crate::HiddenMode`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// Seen while browsing a folder on disk (Local), not added to the library: only folder views
    /// show it until it is added.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
    /// The file was protected (locked) in the camera: it carried the file system's Read-Only attribute when it was
    /// imported. Cameras set it with their protect function; `Filter::locked` finds these photos.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// Video duration in seconds.
    #[serde(default)]
    pub duration: Option<f64>,
    /// Raw files: the camera's as-shot white balance (Kelvin, tint).
    #[serde(default)]
    pub as_shot_wb: Option<(f64, f64)>,
    /// Hash of the file's bytes (hex), for duplicate detection and preview-cache keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// Lens corrections embedded in the file (DNG `WarpRectilinear` / `FixVignetteRadial`), applied when
    /// "Enable Profile Corrections" is on. Boxed: the corrections are 1.7 KB and few photos have them, and every photo of
    /// a library is in memory (the record was 2.8 KB a photo with them inline). Written as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded_lens: Option<Box<lightcraft_develop::EmbeddedLens>>,
    /// A virtual copy: the photo it was copied from (it shares that photo's file but has its own
    /// settings, metadata and history).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_of: Option<PhotoId>,
    /// Virtual copies' name ("Copy 1").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_name: Option<String>,
    /// The settings import gave the photo when a user default (a raw/JPEG default preset, see
    /// the engine's import defaults) changed them from [`Photo::camera_defaults`]. They count as
    /// unedited, and Reset returns to them.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::snapload::develop_opt",
        serialize_with = "crate::compact::develop_opt_out"
    )]
    pub import_look: Option<Arc<DevelopSettings>>,
    /// Assisted culling scores (`None` until analysed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<Analysis>,
    /// A raw file whose sensor data can't be decoded yet (an unsupported raw variant): why (the
    /// raw decoder's reason, e.g. "Nikon Huffman-compressed NEF …"). The photo is shown and
    /// edited from the camera's embedded JPEG preview — a rendered image with the camera's
    /// picture style baked in — so it is treated as a rendered (non-raw) source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_only: Option<String>,
    /// The app version (`CARGO_PKG_VERSION`) whose decoder set [`Photo::preview_only`]; empty when
    /// it isn't set, and in catalogs from before format v9. A preview-only photo whose stamp
    /// differs from the running version is rechecked when its library opens (issue #662): newer
    /// builds may decode the file, older ones may decode less.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preview_checked: String,
    /// A Local record: the fingerprint of its state when the browse catalogued it, to tell
    /// whether the user changed anything since (see [`crate::local`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_baseline: Option<u64>,
    /// The original is one of the files in the user's library folders on the sync server, read
    /// there in place: its path, starting with the folder's name (`Photos/2024/Trip/IMG_1.CR3`).
    /// Set by the server when it finds the file (and when the file moves); `None` for every other
    /// photo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_path: Option<String>,
    /// The original stays where the user keeps it (their Apple Photos library, or a folder on one device), added with
    /// Keep Originals Where They Are: it is never copied into the library nor uploaded to the sync server, and only the
    /// record, its edits and small previews travel. `None` for every other photo. Set when the photo is added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

/// Where the original of a photo kept where it is lives ([`Photo::origin`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub kind: OriginKind,
    /// Apple Photos: the asset's cloud identifier, the same on every device signed in to one iCloud Photos library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
    /// The sync device that added the photo (its id on the server; 0 when it wasn't signed in).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub device: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OriginKind {
    /// An asset of the device's Apple Photos library.
    Photos,
    /// A file in a folder on the device's disk.
    Folder,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// What assisted culling measured on a photo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    /// Focus: 0 (blurred) .. 100 (crisp), from the detail in the photo's sharpest area.
    pub sharpness: f32,
    /// Exposure: share of clipped pixels, 0..1 (shadows + highlights).
    pub clipped: f32,
    /// Similar-shot group (burst), when the photo has neighbours that look alike.
    pub group: Option<u32>,
    /// The best photo of its group.
    pub best: bool,
}

/// Raw formats whose files carry vendor white-balance multipliers but no colour matrices: their starting
/// look is fitted to the file's own JPEG (the engine's `camera_preview::file_local_look` lists the same
/// formats). `format` is the file's extension or Lightroom's `fileFormat` name, in any case. Whether such a
/// file's white balance is edited relative to its as-shot look or in Kelvin depends on the camera:
/// [`relative_wb_camera`].
pub fn relative_wb_format(format: &str) -> bool {
    ["ARW", "NEF", "NRW", "RW2", "RWL", "RAW", "RAF", "CR3", "CR2", "PEF", "SRW", "ORF", "ORI", "ERF", "MRW"]
        .iter()
        .any(|f| format.eq_ignore_ascii_case(f))
}

/// Whether a raw of `format` shot on `camera` (Exif make and model, as [`Meta::camera`] keeps them) is developed
/// with a white balance relative to its as-shot look (6500 K / 0 means as shot, as for rendered photographs):
/// a file without colour matrices ([`relative_wb_format`]) whose camera has none measured either, nor
/// white-balance presets to read Kelvin from. A camera with matrices in `lightcraft_raw::spectral` edits in Kelvin:
/// the matrices turn its as-shot multipliers into a temperature, and the slider back, while its colour look stays
/// whatever the file's fit gives (issue #730). A camera in `lightcraft_raw::kelvin_presets` does the same through
/// its own presets. The engine's render path (`files.rs`) decides the same way from the file; both must agree.
pub fn relative_wb_camera(format: &str, camera: &str) -> bool {
    relative_wb_format(format) && lightcraft_raw::spectral::find_camera(camera).is_none() && !lightcraft_raw::kelvin_presets::covers_camera(camera)
}

/// Exif makes of more than one word whose models don't repeat them (a model that does, like `NIKON CORPORATION`'s
/// `NIKON D850`, needs no entry), each with the brand [`camera_name`] shows it as. The makes of the brands the raw
/// readers know (`lightcraft_raw`'s vendor checks: Olympus / OM, Minolta, Epson, Kodak, Ricoh / Pentax, Samsung,
/// Mamiya, Leica, Fujifilm, Phase One) as their cameras write them (checked against the Exif makes of the app's test raws,
/// 2026-11-12), and a few phone and camera makers of rendered files. The longest one that starts the camera is its make.
// ponytail: a list; a make of several words missing from it splits after its first word. Keeping the make apart in the
// catalog would be exact, but changes the catalog format and sync (card #368, option 2).
pub const LONG_MAKES: &[(&str, &str)] = &[
    ("OLYMPUS IMAGING CORP.", "OLYMPUS"),
    ("OLYMPUS CORPORATION", "OLYMPUS"),
    ("OLYMPUS OPTICAL CO.,LTD", "OLYMPUS"),
    ("OM Digital Solutions", "OM"),
    ("Minolta Co., Ltd.", "Minolta"),
    ("MINOLTA CO.,LTD", "MINOLTA"),
    ("KONICA MINOLTA", "KONICA MINOLTA"),
    ("Konica Minolta Camera, Inc.", "Konica Minolta"),
    ("SEIKO EPSON CORP.", "EPSON"),
    ("Eastman Kodak Company", "Kodak"),
    ("RICOH IMAGING COMPANY, LTD.", "RICOH"),
    ("SAMSUNG TECHWIN", "SAMSUNG"),
    ("SAMSUNG TECHWIN Co.", "SAMSUNG"),
    ("Mamiya-OP Co.,Ltd.", "Mamiya"),
    ("Leica Camera AG", "Leica"),
    ("FUJI PHOTO FILM CO., LTD.", "FUJIFILM"),
    ("CASIO COMPUTER CO.,LTD.", "CASIO"),
    ("Phase One", "Phase One"),
    ("Phase One A/S", "Phase One"),
    ("Arashi Vision", "Insta360"),
    ("LG Electronics", "LG"),
    ("HMD Global", "Nokia"),
];

/// The Exif make and model a [`Meta::camera`] was joined from (`"Make Model"`), as the camera wrote them, for a file
/// that carries them again (an export). The make ends where the model repeats its first word (`"Canon Canon EOS R6"`
/// is `Canon` / `Canon EOS R6`, `"NIKON CORPORATION NIKON D850"` is `NIKON CORPORATION` / `NIKON D850`), else after a
/// make of several words ([`LONG_MAKES`]), else after the first word (`"SONY ILCE-7M3"`, `"Apple iPhone 15 Pro"`).
/// One word is a model without a make; an empty camera is neither (`""`).
pub fn camera_make_model(camera: &str) -> (&str, &str) {
    let camera = camera.trim();
    let mut words = Vec::new();
    let mut from = 0;
    for w in camera.split_whitespace() {
        let at = camera.get(from..).and_then(|s| s.find(w)).map_or(from, |i| from + i);
        words.push((at, w));
        from = at + w.len();
    }
    let split = |at: usize| (camera.get(..at).unwrap_or_default().trim_end(), camera.get(at..).unwrap_or_default().trim_start());
    let Some(&(_, first)) = words.first() else { return ("", "") };
    if let Some(&(at, _)) = words.iter().skip(1).find(|(_, w)| w.eq_ignore_ascii_case(first)) {
        return split(at);
    }
    let long = LONG_MAKES
        .iter()
        .map(|(m, _)| m)
        .filter(|m| {
            camera.get(..m.len()).is_some_and(|p| p.eq_ignore_ascii_case(m))
                && camera.get(m.len()..).is_some_and(|r| r.starts_with(char::is_whitespace))
        })
        .max_by_key(|m| m.len());
    match (long, words.get(1)) {
        (Some(m), _) => split(m.len()),
        (None, Some(&(at, _))) => split(at),
        (None, None) => ("", camera),
    }
}

/// The camera as one name to show (Info, the camera filter's choices, the TV viewer), from a [`Meta::camera`] key, which
/// stays what filters and per-camera lookups match on: the model when it starts with the make's brand (`"Canon Canon EOS
/// R6"` is `Canon EOS R6`, `"NIKON CORPORATION NIKON D850"` is `NIKON D850`), else the brand and the model (`SONY
/// ILCE-7M3`, `Apple iPhone 15 Pro`, `"OLYMPUS IMAGING CORP. E-M5"` is `OLYMPUS E-M5`), else whichever there is. The brand
/// of a make of several words is [`LONG_MAKES`]'s, else the make's first word.
pub fn camera_name(camera: &str) -> String {
    let (make, model) = camera_make_model(camera);
    if make.is_empty() || model.is_empty() {
        return if make.is_empty() { model } else { make }.to_string();
    }
    let brand =
        LONG_MAKES.iter().find(|(m, _)| m.eq_ignore_ascii_case(make)).map(|(_, b)| *b).or_else(|| make.split_whitespace().next()).unwrap_or(make);
    let branded = model.get(..brand.len()).is_some_and(|p| p.eq_ignore_ascii_case(brand))
        && model.get(brand.len()..).is_some_and(|rest| !rest.starts_with(char::is_alphanumeric));
    if branded { model.to_string() } else { format!("{brand} {model}") }
}

/// This build's version: what a photo's [`Photo::preview_checked`] stamp is compared with. Every
/// workspace crate shares one version, so it is the app's.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The [`Photo::preview_checked`] stamp for a `preview_only` decided by this build: its version
/// when the photo is preview-only, empty otherwise.
pub fn preview_stamp(preview_only: &Option<String>) -> String {
    if preview_only.is_some() { APP_VERSION.to_string() } else { String::new() }
}

impl Photo {
    /// The History step the settings are at (the cursor, clamped to the list; `None` = no history).
    pub fn history_index(&self) -> Option<usize> {
        let last = self.history.len().checked_sub(1)?;
        Some(self.history_cursor.map_or(last, |c| c.min(last)))
    }

    pub fn new(id: PhotoId, source: Source, file_name: &str, format: &str, width: u32, height: u32, imported: &str) -> Photo {
        Photo {
            id,
            source,
            file_name: file_name.to_string(),
            kind: MediaKind::Image,
            format: format.to_string(),
            width,
            height,
            file_size: 0,
            captured: None,
            imported: imported.to_string(),
            meta: Meta::default(),
            rating: 0,
            flag: Flag::None,
            label: None,
            develop: Arc::new(DevelopSettings::default()),
            edited: None,
            versions: Vec::new(),
            history: Vec::new(),
            history_cursor: None,
            deleted: false,
            hidden: false,
            local: false,
            locked: false,
            duration: None,
            as_shot_wb: None,
            content_hash: None,
            embedded_lens: None,
            copy_of: None,
            copy_name: None,
            import_look: None,
            analysis: None,
            preview_only: None,
            preview_checked: String::new(),
            local_baseline: None,
            server_path: None,
            origin: None,
        }
    }
    /// The folder of [`Photo::server_path`] (`Photos/2024/Trip`).
    pub fn server_folder(&self) -> Option<&str> {
        self.server_path.as_deref().map(|p| p.rsplit_once('/').map_or("", |(dir, _)| dir))
    }
    /// Sets [`Photo::preview_only`] as this build decided it, with its stamp.
    pub fn set_preview_only(&mut self, preview_only: Option<String>) {
        self.preview_checked = preview_stamp(&preview_only);
        self.preview_only = preview_only;
    }
    /// A preview-only photo whose flag was set by another app version (or before stamps existed),
    /// so this build should probe its file again (issue #662).
    pub fn preview_only_stale(&self) -> bool {
        self.preview_only.is_some() && self.preview_checked != APP_VERSION
    }
    /// A raw file developed from its sensor data: not a rendered image, and not a raw shown from
    /// its embedded preview ([`Photo::preview_only`]).
    pub fn develops_raw(&self) -> bool {
        self.kind == MediaKind::Raw && self.preview_only.is_none()
    }
    /// A raw developed from a look fitted to its own JPEG (ARW, NEF, RW2, RAF, CR3, CR2, PEF, SRW, ORF: vendor
    /// WB multipliers, no colour matrices in the file; RWL and RAW are Leica's and the oldest Panasonic bodies'
    /// names for RW2 files). The engine's `camera_preview::file_local_look` covers the same formats.
    pub fn fitted_look(&self) -> bool {
        self.develops_raw() && relative_wb_format(&self.format)
    }
    /// White balance relative to the camera's as-shot look, as for rendered photographs: a
    /// [`Photo::fitted_look`] raw whose camera has no colour matrices measured either
    /// ([`relative_wb_camera`]). Other raws edit white balance in Kelvin.
    pub fn relative_wb(&self) -> bool {
        self.develops_raw() && relative_wb_camera(&self.format, &self.meta.camera)
    }
    /// The develop settings import gave this photo: [`Photo::camera_defaults`], or the user's
    /// default preset applied on top of them ([`Photo::import_look`]). Like the camera defaults,
    /// in the photo's own rendering process.
    pub fn import_defaults(&self) -> DevelopSettings {
        match &self.import_look {
            Some(l) => DevelopSettings { process: self.develop.process, ..(**l).clone() },
            None => self.camera_defaults(),
        }
    }
    /// The built-in defaults for this photo, before any user default preset: raws start from
    /// their as-shot white balance; lens corrections on when the file has them, and for a CR3, whose
    /// camera's vignetting correction is measured from its JPEG once it is decoded (the engine's
    /// `camera_preview::jpeg_vignetting`; no import probe sees it). They
    /// carry the photo's own rendering process ([`lightcraft_develop::ProcessVersion`]): which
    /// process a photo renders with is not an edit, so comparisons with its defaults (edited?
    /// still as imported?) don't change when a newer process exists. A new photo has the latest.
    pub fn camera_defaults(&self) -> DevelopSettings {
        let relative = self.relative_wb();
        let wb = if relative { Some((6500.0, 0.0)) } else { self.as_shot_wb };
        let mut d = match wb {
            Some((t, tint)) => DevelopSettings::for_raw(t, tint),
            None => DevelopSettings::default(),
        };
        if self.develops_raw() && !relative {
            d.wb.scale = lightcraft_develop::WbScale::Kelvin;
        }
        if self.embedded_lens.is_some() || (self.develops_raw() && self.format.eq_ignore_ascii_case("CR3")) {
            d.optics.lens_profile = true;
        }
        d.process = self.develop.process;
        d
    }
    /// The file name's extension without the dot, as written (`CR2` for `IMG_0042.CR2`); empty
    /// when the name has none (`README`, a dot file such as `.hidden`).
    pub fn extension(&self) -> &str {
        self.file_name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()).map_or("", |(_, ext)| ext)
    }
    /// Width and height in pixels as the photo is shown: turned by its orientation and cut by its
    /// crop. Straightening and Constrain Crop shrink the crop without changing its shape, so the
    /// aspect is exact and the edges can read a little larger than an export.
    pub fn shown_size(&self) -> (f64, f64) {
        let (w, h) = (self.width as f64, self.height as f64);
        let (w, h) = if self.develop.orientation.swaps_axes() { (h, w) } else { (w, h) };
        let r = self.develop.crop.geometry.rect;
        // a crop read from a file or an agent is hostile: no NaN, nothing outside the frame (with
        // Generative Expand, nothing beyond the grown photo)
        let most = if self.develop.crop.expand { 1.0 + 2.0 * lightcraft_geom::EXPAND_MAX } else { 1.0 };
        let part = |len: f64| if len.is_finite() { len.clamp(0.0, most) } else { 1.0 };
        (w * part(r.width()), h * part(r.height()))
    }
    /// Cropped or straightened.
    pub fn is_cropped(&self) -> bool {
        !self.develop.crop.geometry.is_identity()
    }
    /// How many keywords the photo has: a hierarchical keyword (`travel|italy|rome`) is one, and
    /// the same keyword in different case or with stray spaces counts once; blank ones don't count.
    pub fn keyword_count(&self) -> usize {
        let mut seen: Vec<String> = self.meta.keywords.iter().map(|k| k.trim().to_lowercase()).filter(|k| !k.is_empty()).collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    }
    /// The people in the photo: the names on its face regions, trimmed, each once (names that
    /// differ only in case are one person, as first seen). Pets and unnamed faces are not people.
    pub fn people(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for r in self.meta.regions.iter().filter(|r| r.kind == lightcraft_meta::RegionKind::Face) {
            let Some(name) = r.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) else { continue };
            if !out.iter().any(|o| o.to_lowercase() == name.to_lowercase()) {
                out.push(name);
            }
        }
        out
    }
    /// In the library: not deleted and not only browsed (Local).
    pub fn in_library(&self) -> bool {
        !self.deleted && !self.local
    }
    /// In the library and not hidden: what counts and tallies the user sees are made of.
    pub fn visible(&self) -> bool {
        self.in_library() && !self.hidden
    }
    /// Edited by the user: settings differ from what import gave the photo.
    pub fn is_edited(&self) -> bool {
        let mut defaults = self.import_defaults();
        // As Shot reads the source's current WB; stored numbers can predate a decoder update.
        if self.develop.wb.mode == WbMode::AsShot && defaults.wb.mode == WbMode::AsShot {
            defaults.wb = self.develop.wb;
        }
        !self.develop_is(defaults) && !self.develop.is_unedited()
    }
    /// Whether the photo's develop settings are `defaults`, the lens corrections switch aside when the file has no
    /// lens data: without any the switch corrects nothing, so a CR3 imported before CR3s started with it on (their
    /// correction is measured when decoded) still has its import settings and look.
    pub fn develop_is(&self, mut defaults: DevelopSettings) -> bool {
        if self.embedded_lens.is_none() {
            defaults.optics.lens_profile = self.develop.optics.lens_profile;
        }
        *self.develop == defaults
    }
    /// Capture time if known, else import time (sort key).
    pub fn date(&self) -> &str {
        self.captured.as_deref().unwrap_or(&self.imported)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: AlbumId,
    pub name: String,
    /// Containing folder (an album with `folder = true`).
    #[serde(default)]
    pub parent: Option<AlbumId>,
    /// Folders hold albums, not photos.
    #[serde(default)]
    pub folder: bool,
    #[serde(default)]
    pub photos: Vec<PhotoId>,
    #[serde(default)]
    pub cover: Option<PhotoId>,
    /// A smart album: its photos are every (non-deleted) photo matching these rules, evaluated
    /// live; `photos` stays empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart: Option<Box<crate::Filter>>,
    /// The Quick Collection: a regular album that B adds to (one per library).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quick: bool,
    /// Its place among the albums of the same kind in its folder, once the user ordered them by
    /// hand (see [`crate::Catalog::album_children`]); `None`: listed by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
}

impl Album {
    /// A regular (manual) album.
    pub fn new(id: AlbumId, name: impl Into<String>) -> Album {
        Album { id, name: name.into(), parent: None, folder: false, photos: Vec::new(), cover: None, smart: None, quick: false, order: None }
    }
    pub fn is_smart(&self) -> bool {
        self.smart.is_some()
    }
}

#[cfg(test)]
mod edited_tests {
    use super::*;
    use lightcraft_develop::WbScale;

    #[test]
    fn raw_with_import_settings_is_not_edited() {
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "a.arw", "ARW", 10, 10, "2026-10-01T00:00:00");
        p.as_shot_wb = Some((5200.0, 4.0));
        p.develop = Arc::new(p.import_defaults());
        assert!(!p.is_edited(), "import look counts as unedited");
        let mut d = (*p.develop).clone();
        d.light.exposure = 0.5;
        p.develop = Arc::new(d);
        assert!(p.is_edited());
        p.develop = Arc::new(DevelopSettings::default());
        assert!(!p.is_edited(), "a full reset is unedited too");
    }

    #[test]
    fn the_process_a_photo_renders_with_is_not_an_edit() {
        use lightcraft_develop::ProcessVersion;
        // a photo on another process than the latest (a V1 photo once a later process is the
        // latest, or a photo saved by a newer LightCraft)
        let other = ProcessVersion(ProcessVersion::LATEST.0 + 1);
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "a.dng", "DNG", 10, 10, "2026-10-01T00:00:00");
        p.kind = MediaKind::Raw;
        p.as_shot_wb = Some((5200.0, 4.0));
        assert_eq!(p.camera_defaults().process, ProcessVersion::LATEST, "a new photo gets the latest");
        p.develop = Arc::new(DevelopSettings { process: other, ..p.import_defaults() });
        assert_eq!((p.camera_defaults().process, p.import_defaults().process), (other, other));
        assert!(!p.is_edited(), "still as imported");
        // with a default preset's look as well
        let mut look = DevelopSettings::for_raw(5200.0, 4.0);
        look.light.exposure = 0.3;
        p.import_look = Some(Arc::new(look.clone()));
        p.develop = Arc::new(DevelopSettings { process: other, ..look });
        assert_eq!(p.import_defaults().process, other);
        assert!(!p.is_edited());
        let mut d = (*p.develop).clone();
        d.light.exposure = 1.0;
        p.develop = Arc::new(d);
        assert!(p.is_edited(), "an edit still is one");
    }

    /// Lens corrections start on for a raw whose file has lens data, and for every CR3: its camera's vignetting
    /// correction is measured from its JPEG only when it is decoded. A CR3 imported with them off (before CR3s started
    /// with them on) is still unedited while it has no lens data; once it has, the switch is an edit.
    #[test]
    fn cr3s_start_with_lens_corrections_on() {
        let raw = |name: &str, format: &str| {
            let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, name, format, 10, 10, "2026-10-01T00:00:00");
            p.kind = MediaKind::Raw;
            p
        };
        assert!(raw("a.cr3", "cr3").camera_defaults().optics.lens_profile);
        assert!(!raw("a.cr2", "CR2").camera_defaults().optics.lens_profile);
        assert!(!raw("a.arw", "ARW").camera_defaults().optics.lens_profile);
        let mut with_lens = raw("a.arw", "ARW");
        with_lens.embedded_lens = Some(Default::default());
        assert!(with_lens.camera_defaults().optics.lens_profile);
        let mut jpeg = raw("a.jpg", "CR3");
        jpeg.kind = MediaKind::Image;
        assert!(!jpeg.camera_defaults().optics.lens_profile, "only a raw that develops");
        let mut old = raw("a.cr3", "CR3");
        old.develop = Arc::new(DevelopSettings { optics: Default::default(), ..old.camera_defaults() });
        assert!(!old.develop.optics.lens_profile && !old.is_edited());
        assert!(old.develop_is(old.camera_defaults()), "still its import look (its embedded preview stands in)");
        old.embedded_lens = Some(Default::default());
        assert!(old.is_edited() && !old.develop_is(old.camera_defaults()), "with lens data to apply, off is an edit");
    }

    #[test]
    fn supported_raws_use_relative_white_balance() {
        for (name, format, relative) in [
            ("a.arw", "ARW", true),
            ("a.nef", "NEF", true),
            ("a.nrw", "nrw", true),
            ("a.rw2", "RW2", true),
            ("a.rwl", "RWL", true),
            ("a.raw", "RAW", true),
            ("a.raf", "rAf", true),
            ("a.cr3", "cr3", true),
            ("a.srw", "SRW", true),
            ("a.cr2", "CR2", true),
            ("a.pef", "pef", true),
            ("a.orf", "ORF", true),
            ("a.ori", "ORI", true),
            ("a.erf", "ERF", true),
            ("a.mrw", "MRW", true),
            ("a.dng", "DNG", false),
        ] {
            let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, name, format, 10, 10, "2026-10-01T00:00:00");
            p.kind = MediaKind::Raw;
            p.meta.camera = "Pentax K-1".into();
            p.as_shot_wb = Some((5200.0, 4.0));
            assert_eq!(p.relative_wb(), relative, "{format}");
            assert_eq!(p.fitted_look(), relative, "{format}");
            let wb = p.camera_defaults().wb;
            assert_eq!((wb.temp, wb.tint), if relative { (6500.0, 0.0) } else { (5200.0, 4.0) }, "{format}");
            assert_eq!(wb.scale, if relative { WbScale::Legacy } else { WbScale::Kelvin }, "{format}");
            p.develop = Arc::new(DevelopSettings::for_raw(5200.0, 4.0));
            assert!(!p.is_edited(), "{format}: old As Shot values are not an edit");
            let mut custom = (*p.develop).clone();
            custom.wb.mode = WbMode::Custom;
            p.develop = Arc::new(custom);
            assert!(p.is_edited(), "{format}: Custom WB is still an edit");
            // shown from the embedded preview: a rendered image, not a relative-WB raw
            p.preview_only = Some("unsupported".into());
            assert!(!p.relative_wb() && !p.fitted_look());
        }
    }

    /// HB-284: a camera joined as the catalog keeps it splits back into the Exif make and model the camera wrote.
    #[test]
    fn camera_splits_into_the_make_and_model_it_was_joined_from() {
        for (make, model) in [
            ("Canon", "Canon EOS R6"),
            ("Canon", "Canon EOS 5D Mark III"),
            ("NIKON CORPORATION", "NIKON D850"),
            ("NIKON CORPORATION", "NIKON Z 6_2"),
            ("SONY", "ILCE-7M3"),
            ("Apple", "iPhone 15 Pro"),
            ("FUJIFILM", "X-T4"),
            ("PENTAX Corporation", "PENTAX K-1"),
            ("Leica Camera AG", "LEICA Q2"),
            ("OLYMPUS IMAGING CORP.", "E-M5"),
            ("OM Digital Solutions", "OM-3"),
            ("OLYMPUS OPTICAL CO.,LTD", "E-10"),
            ("DJI", "FC3582"),
        ] {
            assert_eq!(camera_make_model(&format!("{make} {model}")), (make, model), "{make} / {model}");
        }
        // a file with a model but no make, and one with neither
        assert_eq!(camera_make_model("D850"), ("", "D850"));
        assert_eq!(camera_make_model(""), ("", ""));
        assert_eq!(camera_make_model("  "), ("", ""));
        // spaces inside the model stay as they were
        assert_eq!(camera_make_model(" Canon  Canon EOS  R6 "), ("Canon", "Canon EOS  R6"));
    }

    /// HB-368: every make the core's raw readers know, as their cameras write it, splits back to that make, and so does every
    /// make of several words in [`LONG_MAKES`] under a model that doesn't repeat it.
    #[test]
    fn every_make_the_readers_know_splits_back_to_itself() {
        for (make, model) in [
            ("Canon", "Canon EOS R5"),
            ("NIKON CORPORATION", "NIKON Z 8"),
            ("NIKON", "COOLPIX P1000"),
            ("SONY", "ILCE-1"),
            ("Hasselblad", "X2D 100C"),
            ("PENTAX Corporation", "PENTAX K-3"),
            ("PENTAX", "K10D"),
            ("RICOH IMAGING COMPANY, LTD.", "PENTAX K-1 Mark II"),
            ("RICOH IMAGING COMPANY, LTD.", "RICOH GR III"),
            ("SAMSUNG", "NX1"),
            ("SAMSUNG TECHWIN", "Pro 815"),
            ("SEIKO EPSON CORP.", "R-D1"),
            ("Eastman Kodak Company", "DCS Pro 14N"),
            ("KODAK", "DCS760C"),
            ("Mamiya-OP Co.,Ltd.", "MAMIYA ZD"),
            ("Leica Camera AG", "LEICA M11"),
            ("Leica Camera AG", "M8 Digital Camera"),
            ("FUJIFILM", "GFX100S"),
            ("FUJI PHOTO FILM CO., LTD.", "FinePixS2Pro"),
            ("OLYMPUS CORPORATION", "E-M1MarkII"),
            ("OM Digital Solutions", "OM-1"),
            ("Minolta Co., Ltd.", "DiMAGE A2"),
            ("MINOLTA CO.,LTD", "DiMAGE 7Hi"),
            ("KONICA MINOLTA", "DYNAX 7D"),
            ("Panasonic", "DC-S5M2"),
            ("SIGMA", "fp L"),
            ("Phase One", "IQ4 150MP"),
            ("Phase One A/S", "P45+"),
            ("Konica Minolta Camera, Inc.", "DiMAGE A2"),
            ("SAMSUNG TECHWIN Co.", "Pro 815"),
            ("LEICA CAMERA AG", "D-Lux 7"),
            ("Arashi Vision", "Insta360 X5"),
            ("CASIO COMPUTER CO.,LTD.", "EX-F1"),
            ("DJI", "FC3582"),
            ("Google", "Pixel 8 Pro"),
        ] {
            assert_eq!(camera_make_model(&format!("{make} {model}")), (make, model), "{make} / {model}");
        }
        for (make, _) in LONG_MAKES {
            assert_eq!(camera_make_model(&format!("{make} X1")), (*make, "X1"), "{make}");
            // in other cases too, as the make was written
            assert_eq!(camera_make_model(&format!("{} X1", make.to_lowercase())).1, "X1", "{make}");
        }
    }

    /// HB-366: the name shown for a camera says its brand once.
    #[test]
    fn a_camera_is_shown_with_its_brand_once() {
        for (camera, name) in [
            ("Canon Canon EOS R6", "Canon EOS R6"),
            ("NIKON CORPORATION NIKON D850", "NIKON D850"),
            ("PENTAX Corporation PENTAX K-1", "PENTAX K-1"),
            ("SONY ILCE-7M3", "SONY ILCE-7M3"),
            ("Apple iPhone 15 Pro", "Apple iPhone 15 Pro"),
            ("FUJIFILM X-T4", "FUJIFILM X-T4"),
            ("OLYMPUS IMAGING CORP. E-M5", "OLYMPUS E-M5"),
            ("OM Digital Solutions OM-3", "OM-3"),
            ("Eastman Kodak Company DCS Pro 14N", "Kodak DCS Pro 14N"),
            ("Arashi Vision Insta360 X5", "Insta360 X5"),
            ("Leica Camera AG LEICA Q2", "LEICA Q2"),
            ("Canon CanonX", "Canon CanonX"),
            // a model without a make, a make alone, nothing
            ("D850", "D850"),
            ("iPhone 15 Pro", "iPhone 15 Pro"),
            ("", ""),
            ("  ", ""),
        ] {
            assert_eq!(camera_name(camera), name, "{camera:?}");
        }
    }

    /// A raw without matrices in the file edits in Kelvin when its camera's matrices are measured (issue #730):
    /// the decision reads the catalog's camera string, and the photo's defaults say which scale they are on.
    #[test]
    fn raws_of_cameras_with_measured_matrices_use_kelvin_white_balance() {
        for (camera, format, relative) in [
            ("SONY ILCE-7M3", "ARW", false),
            ("SONY ILCE-7RM4A", "ARW", false),
            ("NIKON CORPORATION NIKON D850", "NEF", false),
            ("Canon Canon EOS 5D Mark III", "CR2", false),
            ("Canon Canon EOS R6m2", "CR3", false),
            ("Canon Canon EOS R6 Mark II", "CR3", false),
            ("Canon Canon EOS R7", "CR3", true),
            ("FUJIFILM X-T4", "RAF", false),
            ("ILCE-7M3", "ARW", false),
            ("SONY ILCE-7M2", "ARW", true),
            ("SONY ILCA-99M2", "ARW", false),
            ("SONY SLT-A33", "ARW", false),
            ("SONY ILCE-1", "ARW", true),
            ("", "ARW", true),
            ("SONY ILCE-7M3", "DNG", false),
            ("NIKON CORPORATION NIKON Z5_2", "NEF", false),
        ] {
            let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "a.raw", format, 10, 10, "2026-10-01T00:00:00");
            p.kind = MediaKind::Raw;
            p.meta.camera = camera.into();
            p.as_shot_wb = Some((4986.0, -2.0));
            assert_eq!(p.relative_wb(), relative, "{camera} {format}");
            assert_eq!(relative_wb_camera(format, camera), relative, "{camera} {format}");
            assert_eq!(
                p.fitted_look(),
                format == "ARW" || format == "NEF" || format == "CR2" || format == "CR3" || format == "RAF",
                "{camera} {format}"
            );
            let wb = p.camera_defaults().wb;
            assert_eq!((wb.temp, wb.tint), if relative { (6500.0, 0.0) } else { (4986.0, -2.0) }, "{camera} {format}");
            assert_eq!(wb.scale, if relative { WbScale::Legacy } else { WbScale::Kelvin }, "{camera} {format}");
            // a photo from an older catalog: stored As Shot on the relative scale, still not an edit
            p.develop = Arc::new(DevelopSettings::for_raw(6500.0, 0.0));
            assert!(!p.is_edited(), "{camera} {format}: old As Shot values are not an edit");
            // its as-shot white not yet read through the matrices (6500 / 0 from the old import): defaults still As Shot
            p.as_shot_wb = Some((6500.0, 0.0));
            assert_eq!(p.camera_defaults().wb.mode, WbMode::AsShot);
            assert!(!p.is_edited());
        }
    }

    /// Olympus and OM System bodies whose files carry the camera's white-balance presets edit in Kelvin
    /// (`lightcraft_raw::kelvin_presets`); the older ones without a table stay on the relative scale. A raw shown from its
    /// JPEG is a rendered image whatever the camera.
    #[test]
    fn olympus_bodies_with_a_preset_table_use_kelvin_white_balance() {
        for (camera, format, relative) in [
            ("OLYMPUS CORPORATION E-1", "ORF", false),
            ("OLYMPUS IMAGING CORP. E-500", "ORF", false),
            ("OLYMPUS IMAGING CORP. XZ-2", "ORF", false),
            ("OLYMPUS IMAGING CORP. E-M5MarkII", "orf", false),
            ("OLYMPUS CORPORATION PEN-F", "ORF", false),
            ("OM Digital Solutions OM-3", "ORI", false),
            ("OLYMPUS OPTICAL CO.,LTD E-10", "ORF", true),
            ("OLYMPUS OPTICAL CO.,LTD E-20,E-20N,E-20P", "ORF", true),
            ("OLYMPUS IMAGING CORP. SP550UZ", "ORF", true),
            ("OLYMPUS IMAGING CORP. C5060WZ", "ORF", true),
            ("", "ORF", true),
        ] {
            let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "a.orf", format, 10, 10, "2026-10-01T00:00:00");
            p.kind = MediaKind::Raw;
            p.meta.camera = camera.into();
            p.as_shot_wb = Some((5300.0, 0.0));
            assert_eq!(p.relative_wb(), relative, "{camera}");
            assert_eq!(relative_wb_camera(format, camera), relative, "{camera}");
            let wb = p.camera_defaults().wb;
            assert_eq!((wb.temp, wb.tint), if relative { (6500.0, 0.0) } else { (5300.0, 0.0) }, "{camera}");
            assert_eq!(wb.scale, if relative { WbScale::Legacy } else { WbScale::Kelvin }, "{camera}");
            // a photo from before the Kelvin scale: stored As Shot on the relative scale, still not an edit
            p.develop = Arc::new(DevelopSettings::for_raw(6500.0, 0.0));
            assert!(!p.is_edited(), "{camera}: old As Shot values are not an edit");
            p.as_shot_wb = Some((6500.0, 0.0));
            assert_eq!(p.camera_defaults().wb.mode, WbMode::AsShot, "{camera}");
            assert!(!p.is_edited(), "{camera}");
            p.preview_only = Some("Olympus compressed ORF".into());
            assert!(!p.relative_wb(), "{camera}: shown from its JPEG");
        }
    }
}
