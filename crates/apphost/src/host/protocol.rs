//! The messages between a frontend and the engine thread, as JSON: [`Request`]s in, [`Event`]s out.
//! The SwiftUI app's `LightCraftCore` decodes the same shapes (`docs/ios-swift.md` → *Protocol*);
//! `tests::fixtures` writes one of each kind to the Swift tests' fixture file so the two can't drift.

use lightcraft_catalog::PhotoId;
use lightcraft_engine::activity::TaskInfo;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::startup::Profile;

/// A request: `{"id": 7, "method": "engine.execute", "params": {...}}`. The reply event carries
/// the same `id`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// How to start: [`super::Host::open`]'s argument, as JSON from the frontend.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenConfig {
    /// The app's home folder (on iOS its container): the library goes in `Documents/`, AI models
    /// in `Library/Caches/`.
    #[serde(default)]
    pub home: Option<String>,
    /// The app's tmp folder: staging for picks and exports, the automation script. Default: the
    /// process's temp dir.
    #[serde(default)]
    pub tmp: Option<String>,
    /// The library folder (instead of the one in `home`).
    #[serde(default)]
    pub library: Option<String>,
    /// The in-memory demo library: nothing saved (previews, tests).
    #[serde(default)]
    pub demo: bool,
    /// The device's name, as sync shows it ("iPhone", "iPad").
    #[serde(default)]
    pub device_name: Option<String>,
    /// A control-protocol script to run at start (`LIGHTCRAFT_SCRIPT`; relative to `tmp`).
    #[serde(default)]
    pub script: Option<String>,
    /// Render worker threads (1–6; default: the machine's cores less one, at most 6).
    #[serde(default)]
    pub render_threads: Option<usize>,
    /// Bytes the system lets the app use (iOS: `os_proc_available_memory`); sizes the caches.
    /// Default: what `lightcraft-sysmem` reports.
    #[serde(default)]
    pub available_memory: Option<u64>,
    /// Whose rules start the engine: a phone's (default) or a Mac's ([`Profile`]).
    #[serde(default)]
    pub profile: Profile,
}

/// An error answer: `code` is the kind (the engine's error variants in camelCase —
/// `unknownCommand`, `disabled`, `badParams`, `catalog`, `notSaved`, `libraryInUse`,
/// `lightroomCatalog`, `other` — or the host's own: `badRequest`, `unknownMethod`, `panic`,
/// `closed`, `timeout`), `message` what to tell the user.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

impl ErrorBody {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        ErrorBody { code: code.into(), message: message.into() }
    }

    /// The answer for an engine error.
    pub fn engine(e: &lightcraft_engine::EngineError) -> Self {
        use lightcraft_engine::EngineError as E;
        let code = match e {
            E::UnknownCommand(_) => "unknownCommand",
            E::Disabled(..) => "disabled",
            E::BadParams { .. } => "badParams",
            E::Catalog(_) => "catalog",
            E::NotSaved(_) => "notSaved",
            E::LibraryInUse(_) => "libraryInUse",
            E::LightroomCatalog { .. } => "lightroomCatalog",
            E::Other(_) => "other",
        };
        ErrorBody::new(code, e.to_string())
    }
}

/// What the library and the view look like now: sent as a `state` event whenever any of it changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostState {
    /// Changes with every catalog change (an edit, a rating, an import…).
    pub catalog_revision: u64,
    /// Identifies the grid's photo list (source, filter, sort): a new value when it changes.
    pub visible_gen: u64,
    pub visible_count: usize,
    /// Changes when another library is opened (photo ids change meaning).
    pub library_generation: u64,
    /// Changes whenever the preset list may have (created, imported, renamed, or brought by the
    /// sync from another device): a presets view reads `presets.list` again.
    #[serde(default)]
    pub presets_generation: u64,
    /// The photo the loupe and the edit tools work on.
    pub active: Option<PhotoId>,
    pub selection_count: usize,
    /// What Undo and Redo would undo and redo.
    pub undo: Option<String>,
    pub redo: Option<String>,
    /// A slider drag in progress (its label).
    pub interaction: Option<String>,
    /// The Masking tool's selected mask and the Remove tool's selected spot.
    #[serde(default)]
    pub active_mask: Option<u32>,
    #[serde(default)]
    pub active_spot: Option<usize>,
    /// Changes the photo's catalog not on disk yet: the count (0 when everything is saved).
    pub unsaved: usize,
    /// The server sync, for the cloud icon (`None`: a local library, no server):
    /// `{state: signedOut|conflict|paused|error|syncing|idle, server, pending, uploads, downloads,
    /// reading: [read, total]|null, error}` (`sync.status` has the rest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<Value>,
    /// Face recognition's background scan while it is on: `{pendingPhotos, indexedFaces}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<Value>,
}

/// An event, `{"kind": "...", ...}`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Event {
    /// The answer to the request with this `id`: `result`, or `error`.
    Reply {
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ErrorBody>,
    },
    /// A new picture for a slot (`thumb:42`, `main`, `preview`, `before`…): its pixels are
    /// `token`'s lease (RGBA8, sRGB, straight alpha) until released. `final` is false for a
    /// stand-in (`quick`: where it came from) or a draft (a slider drag in progress).
    #[serde(rename_all = "camelCase")]
    Image {
        slot: String,
        photo: PhotoId,
        key: u64,
        token: u64,
        width: u32,
        height: u32,
        #[serde(rename = "final")]
        is_final: bool,
        draft: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        quick: Option<String>,
        ms: f64,
        /// For `main`: the 256-bin histogram (`r`, `g`, `b`, `luma`, `total`).
        #[serde(skip_serializing_if = "Option::is_none")]
        histogram: Option<Value>,
        /// For `region` (a zoom window): `{full: [w, h], x, y, w, h}`, the zoomed frame it is cut
        /// from and where it goes in it.
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<Value>,
        /// For `main` and `region` of an HDR edit on a display that can show it
        /// (`render.hdrDisplay`): `{token, width, height, peak}`, the same picture as float pixels:
        /// `token`'s lease is RGBA16F (half floats, 8 bytes a pixel, Display P3 encoded with the sRGB
        /// curve extended past 1: SDR white = 1, highlights up to the encoding of `peak`, which is
        /// linear light); the 8-bit picture is then in Display P3 too.
        #[serde(skip_serializing_if = "Option::is_none")]
        hdr: Option<Value>,
    },
    /// A render failed (an unreadable file, a missing original…).
    RenderFailed { slot: String, photo: PhotoId, key: u64, error: String },
    /// The library or the view changed.
    State { state: HostState },
    /// The background tasks (import, export, sync…), as `activity.list` gives them.
    Activity { tasks: Vec<TaskInfo> },
    /// Something to tell the user (English: the key the frontend translates).
    Message { text: String, level: String },
    /// Put `text` on the clipboard (a share link just made).
    Copy { text: String },
    /// Show a dialog the engine needs an answer from: `syncChoice` (signing in met a server that has
    /// a library already: `sync.resolveConflict`).
    Ask { dialog: String, params: Value },
    /// An automation script asks the UI (`ui.screenshot`, `ui.inspect`…): answer with
    /// `ui.reply {id, result | error}`.
    UiRequest { id: u64, method: String, params: Value },
    /// The engine stopped (after `close`).
    Closed,
}

impl Event {
    pub fn ok(id: u64, result: Value) -> Self {
        Event::Reply { id, result: Some(result), error: None }
    }

    pub fn err(id: u64, error: ErrorBody) -> Self {
        Event::Reply { id, result: None, error: Some(error) }
    }

    /// As JSON (an event that can't be written becomes a `message`: never lost silently).
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self)
            .unwrap_or_else(|e| serde_json::json!({"kind": "message", "text": format!("an event could not be written: {e}"), "level": "error"}))
    }
}
