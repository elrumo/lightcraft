use super::*;

pub type PickFiles = Box<dyn FnMut() -> Vec<String>>;

/// A save dialog: suggested file name → chosen path (`None` = cancelled).
pub type SaveFile = Box<dyn FnMut(&str) -> Option<String>>;

pub type WriteFn = Box<dyn FnMut(&str, &[u8]) -> Result<(), String>>;

/// A writer other threads can use (background export).
pub type SharedWrite = std::sync::Arc<dyn Fn(&str, &[u8]) -> Result<(), String> + Send + Sync>;

/// Runs a prepared export batch without blocking the UI where there are no threads (the browser):
/// the host takes the batch to an executor of its own, keeps the [`Pending`](lightcraft_apphost::export::Pending)
/// current (progress, ✕) and ends it with the outcome; an error means it did not start.
pub type ExportRunner = Box<dyn FnMut(lightcraft_apphost::export::Prepared, lightcraft_apphost::export::Pending) -> Result<(), String>>;

pub type PngEncode = Box<dyn Fn(&lightcraft_raster::Rgba8) -> Vec<u8>>;

/// A folder chooser (`None` = cancelled).
pub type PickFolder = Box<dyn FnMut() -> Option<String>>;

/// Reveal a file in the system file manager (Finder / Explorer / the folder on Linux).
pub type RevealFn = Box<dyn FnMut(&str) -> Result<(), String>>;

/// Open a URL in the user's browser.
pub type OpenUrlFn = Box<dyn FnMut(&str) -> Result<(), String>>;

/// Open a file in an application (`app` = "" for the system's default one).
pub type OpenWithFn = Box<dyn FnMut(&str, &str) -> Result<(), String>>;

/// Open several files with one start of an application (`app` as for [`OpenWithFn`]).
pub type OpenAllWithFn = Box<dyn FnMut(&[String], &str) -> Result<(), String>>;

/// Linux model-folder work: wait for the default opener on a background task.
#[cfg(target_os = "linux")]
pub type OpenModelFolderFn = std::sync::Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// The ICC profile of the screen showing the window, as the system colour-manages it (`None`:
/// not known).
pub type ScreenProfileFn = Box<dyn FnMut() -> Option<Vec<u8>>>;

/// Where the pointer is in the window while files are dragged in from outside it, in points from its
/// top left (`None`: not known). winit sends no pointer position during such a drag.
pub type FileDragPointerFn = Box<dyn FnMut() -> Option<egui::Pos2>>;

/// Whether an external editor can be started (its name as in [`Services::open_with`]); the
/// error says why not and what to do (e.g. PhotoCraft isn't installed).
pub type CheckEditorFn = Box<dyn Fn(&str) -> Result<(), String>>;

/// Something the host does with the session (web: back up or restore the library in browser
/// storage); the work may finish asynchronously.
pub type HostAction = Box<dyn FnMut(&mut Session) -> Result<Value, String>>;

/// Where the host's own picker takes photos from ([`Services::host_pick`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickSource {
    /// The system photo library (iOS: Photos).
    Photos,
    /// Image files (iOS: Files, iCloud Drive, external drives).
    Files,
    /// A folder of photos (iOS: a folder in Files).
    Folder,
}

/// Show the host's picker for `source`. It returns at once; the picked photos arrive later as
/// `file.addPhotos {paths, staged: true}` (copies in the host's staging folder).
pub type HostPick = Box<dyn FnMut(PickSource) -> Result<(), String>>;

/// Exports handed to the system instead of written to a folder the user chooses (iOS: the share
/// sheet, with Save Image, Save to Files, AirDrop and other apps). The export dialog then has no
/// folder field; each export empties `dir` first, writes there and offers what it wrote.
pub struct ShareExports {
    /// Where exports are written (the host's own staging folder; emptied before each export).
    pub dir: String,
    /// Offer the exported files (absolute paths, at least one).
    pub share: Box<dyn FnMut(&[String])>,
}

/// How a save to the photo library ended: how many photos went in, or why none did. Called on any
/// thread.
pub type SaveDone = Box<dyn FnOnce(Result<usize, String>) + Send>;

/// Add exported files (absolute paths, at least one) to the device's photo library (iOS: PhotoKit).
/// It returns at once and calls the `SaveDone` when the library has them (or has refused), after
/// which the host asks for a repaint. The files stay where they are.
pub type SaveToPhotos = Box<dyn FnMut(&[String], SaveDone)>;

/// What finished saves (see [`SaveToPhotos`]) left for the app to say, taken by `export_task::poll`.
pub type SavedInbox = std::sync::Arc<std::sync::Mutex<Vec<Result<usize, String>>>>;

/// The system's dark/light appearance where egui doesn't report one (Linux desktops whose
/// windowing layer says nothing, e.g. many Wayland compositors). The host answers from what it
/// last heard and wakes the UI itself when that changes; it is never polled for changes.
pub type SystemThemeFn = Box<dyn Fn(&egui::Context) -> Option<egui::Theme>>;

/// Platform services injected by the host app (desktop or web).
#[derive(Default)]
pub struct Services {
    /// Native file dialogs for commands, shown off the UI thread and answered later
    /// (`pick`; #191). With it, the synchronous pickers below serve only the buttons inside
    /// dialogs; without it (the web, tests) they answer the commands too.
    pub picker: Option<pick::Picker>,
    /// The host's own file picker, which hands files over later, by content or as copies (the web:
    /// the browser's file chooser; iOS: the Files picker). Commands that can take files that way
    /// ask it ([`pick::ask_host`]; File ▸ Import Profiles & Presets…) when the dialogs above aren't
    /// there.
    pub host_files: Option<pick::HostPicker>,
    /// Show an open dialog for photos; returns paths.
    pub pick_files: Option<PickFiles>,
    /// Local ONNX model; a manifest must accompany it.
    pub pick_denoise_model: Option<PickFiles>,
    /// Open dialog for a face model (`.onnx`; Settings ▸ Faces ▸ Add a model file…).
    pub pick_model_file: Option<PickFiles>,
    /// Open dialog for preset files (`.lcpreset`, `.xmp`, `.lrtemplate`, `.zip`, `.dng`, Luminar `.lmp` / `.mplumpack`).
    pub pick_preset_files: Option<PickFiles>,
    /// Open dialog for a GPS track log (`.gpx`; Photo ▸ Auto-Tag from Tracklog…).
    pub pick_tracklog: Option<PickFiles>,
    /// Open dialog for a monitor ICC profile (`.icc` / `.icm`; Settings ▸ Display; desktop only).
    pub pick_display_profile: Option<PickFiles>,
    /// The profile of the screen showing the window, used while none is chosen in Settings ▸
    /// Display (macOS, whose windows aren't colour-managed: issue #787).
    pub screen_profile: Option<ScreenProfileFn>,
    /// The pointer while files are dragged in from outside the window (macOS). Without it the album
    /// under them can't be told, so the sidebar doesn't light one up and a drop goes to the album
    /// the grid shows.
    pub file_drag_pointer: Option<FileDragPointerFn>,
    /// Save dialog for an exported `.lcpreset` file.
    pub save_preset_file: Option<SaveFile>,
    /// Open dialog for a keyword list file (`.txt`, Photo Supreme's `.utf8`; File ▸ Import Keywords…).
    pub pick_keyword_list: Option<PickFiles>,
    /// Save dialog for an exported keyword list (File ▸ Export Keywords…).
    pub save_keyword_list: Option<SaveFile>,
    /// Open dialog for point-curve preset files (`.lccurve`).
    pub pick_curve_preset_files: Option<PickFiles>,
    /// Save dialog for an exported `.lccurve` file.
    pub save_curve_preset_file: Option<SaveFile>,
    /// Open dialog for slideshow template files (`.json`; Settings ▸ Slideshow ▸ Templates ▸ Import…).
    pub pick_slideshow_templates: Option<PickFiles>,
    /// Save dialog for an exported slideshow template (`.json`).
    pub save_slideshow_template: Option<SaveFile>,
    /// Open dialog for print template files (`.lcprint`; File ▸ Print… ▸ Templates ▸ Import…).
    pub pick_print_templates: Option<PickFiles>,
    /// Save dialog for an exported print template (`.lcprint`).
    pub save_print_template: Option<SaveFile>,
    pub write: Option<WriteFn>,
    /// Create-new writer (never replaces an existing path), for screenshots of destinations that
    /// did not exist when the request was admitted. Required for such captures: falling back to
    /// [`Self::write`] would reopen the admission-to-write race.
    pub write_new: Option<WriteFn>,
    /// Thread-safe writer: with it, UI-started exports run in the background (desktop only).
    pub write_shared: Option<SharedWrite>,
    /// Without threads: a host that can run an export between frames instead (the browser, whose
    /// workers render and encode the photos). Used when there is no `write_shared`.
    pub export_runner: Option<ExportRunner>,
    /// PNG encoder (the host links an image encoder; the UI crate stays codec-free).
    pub png: Option<PngEncode>,
    /// Show a file in the system file manager (desktop only).
    pub reveal: Option<RevealFn>,
    /// The host's current log file (`<settings>/logs/lightcraft.log`), which Help ▸ Open Log
    /// Folder reveals (#260). None where no log is kept: the web, `--memory`, `LIGHTCRAFT_NO_PREFS`.
    pub log_file: Option<String>,
    /// Choose a folder (Settings → General → Open Library…; desktop only).
    pub pick_folder: Option<PickFolder>,
    /// Open a Lightroom Classic `.lrcat` catalog for read-only import.
    pub pick_lightroom_catalog: Option<PickFolder>,
    /// Open a web link in the browser (Help menu, About, Discord button).
    pub open_url: Option<OpenUrlFn>,
    /// Open a file in an external editor (Edit in External Editor; desktop only).
    pub open_with: Option<OpenWithFn>,
    /// Open several files with one start of the editor (Open Original; desktop only): an editor
    /// that keeps one window takes them all. None = [`Services::open_with`] for each.
    pub open_all_with: Option<OpenAllWithFn>,
    /// Optional worker-safe model-folder opener alongside `open_with` (Linux desktop only).
    #[cfg(target_os = "linux")]
    pub open_model_folder: Option<OpenModelFolderFn>,
    /// Checked before Edit In renders a copy, so a missing editor leaves no copy behind (none =
    /// assume it can).
    pub check_editor: Option<CheckEditorFn>,
    /// File ▸ Back Up Library…: save the whole library (catalog and originals) as one file the
    /// user keeps (web only: there the library lives in browser storage, which the browser may
    /// clear; on the desktop it is a folder backed up like any other).
    pub backup_library: Option<HostAction>,
    /// File ▸ Restore Library from Backup… (web only; keeps the current library).
    pub restore_library: Option<HostAction>,
    /// Runs self-hosted sync requests ([`sync_ui`]); `None`: this host doesn't sync.
    pub sync_exec: Option<sync_ui::SyncExec>,
    /// Fetches map tiles for the Map view ([`panels::map`]); `None`: only the built-in map.
    pub tile_exec: Option<panels::map::TileExec>,
    /// Exports go to the system's share sheet instead of a folder (iOS; see [`ShareExports`]).
    pub share_exports: Option<ShareExports>,
    /// Adds exports to the device's photo library (iOS: Save to Photos in the share sheet and the
    /// photo's top bar; needs [`Services::share_exports`], whose folder the files are written to).
    pub save_to_photos: Option<SaveToPhotos>,
    /// The host's own pickers (iOS: Photos, Files): File ▸ Import from Photos… / from Files… /
    /// Folder from Files…, and the compact grid's + button.
    pub host_pick: Option<HostPick>,
    /// Cameras, phones and cards the host reads itself (iOS, macOS: ImageCaptureCore): File ▸
    /// Import from Device, the phone's + ▸ Connected Camera/Card ([`camera_import`]).
    pub cameras: Option<Box<dyn CameraHost>>,
    /// Plays haptic feedback (iOS: the Taptic Engine); the compact layout asks for it on touch
    /// gestures and choices (`haptics`).
    pub haptic: Option<haptics::PlayHaptic>,
    /// Fetches AI Remove's model in the browser build ([`erase::ModelFetch`]); `None`: the
    /// engine downloads it (desktop and phone apps).
    pub erase_model: Option<erase::ModelFetch>,
    /// Fetches Lens Blur's depth model in the browser build, as [`Services::erase_model`] does
    /// AI Remove's; `None`: the engine downloads it.
    pub depth_model: Option<erase::ModelFetch>,
    /// The system clipboard's text, for Paste in a text field's touch edit menu
    /// (`panels::mobile::edit_menu`; iOS: the on-screen keyboard has no ⌘V). A host that can only
    /// read it asynchronously (the web) starts the read here, returns `None` and sends the text
    /// itself as an `egui::Event::Paste` on a later frame.
    pub clipboard_text: Option<Box<dyn FnMut() -> Option<String>>>,
    /// The system appearance for Appearance Mode ▸ Auto, ahead of egui's own report.
    pub system_theme: Option<SystemThemeFn>,
}
