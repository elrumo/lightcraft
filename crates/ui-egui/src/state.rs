//! UI state (serde: saved as preferences, readable/settable through the control channel).

use serde::{Deserialize, Serialize};

mod dialog;
mod settings;
mod view;

pub use dialog::{Dialog, ExportPage, ExportThen, HiddenPrompt, NameEdit, SyncCounts, default_preset_groups};
pub use settings::{
    AppSettings, AppearanceMode, DarkTheme, EditFile, EditFormat, EditInMode, EditSpace, GridBadges, KeywordChips, LOUPE_EDGE_CEILING,
    LOUPE_EDGE_STEP, LaunchLibrary, LightTheme, LightroomImport, MIN_TEXTURE_SIDE, ModelStep, PHOTOCRAFT, PREVIEW_LIMITS, RECENT_LIBRARIES,
    STANDARD_PREVIEW_EDGE, StartupView, WINDOW_FRAME_CEILING,
};
pub use view::{
    Appearance, BRIEF_INFO_SECS, BeforeAfter, BriefInfo, CLICK_ZOOMS, CropOverlay, FOCUS_KEYWORDS_PASSES, GridJump, GridSpan, HiddenPanels,
    InfoOverlay, KeywordingView, LEFT_WIDTH, LayoutImage, LightsOut, LoupeInfo, LoupeInfoField, MIN_PHOTO_WIDTH, PanelWidth, RIGHT_WIDTH,
    RightPanel, THUMB_SIZE_MAX, THUMB_SIZE_MIN, THUMB_SIZE_STEP, ToolOverlay, ViewMode, Zoom,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    /// How large the app's window was and whether it was zoomed, to open it like that again (the
    /// desktop app; absent until a window has rested at a size). Read leniently: a window that
    /// is not one reads as none, and never costs the other settings.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::window_place::WindowPlace::lenient")]
    pub window: Option<crate::window_place::WindowPlace>,
    /// Shared panel arrangement; absent in legacy settings until their first layout.
    pub docking: Option<crate::docking::Workspace>,
    /// The arrangement generation `docking` was saved in ([`crate::docking::DOCKING_VERSION`]);
    /// settings files are read through [`crate::docking::drop_outdated`].
    pub docking_version: u32,
    #[serde(default = "crate::i18n::default_language")]
    pub language: crate::i18n::Locale,
    /// The Build Previews run last announced (its identity, finished?).
    #[serde(skip)]
    pub preview_build_seen: Option<(usize, bool)>,
    /// Saving the library is failing (announced with a toast; the top bar shows a warning until
    /// a later save succeeds).
    #[serde(skip)]
    pub unsaved_seen: bool,
    /// Compact layout: the grid is choosing photos (a tap adds or removes one; the action bar
    /// works on the chosen ones). Entered with Select or a long press; `view.selectMode`.
    #[serde(skip)]
    pub select_mode: bool,
    /// Compact layout: photos are being chosen from the library to add to this album (the album's +
    /// ▸ LightCraft): All Photos in select mode, with Add in place of the action bar. Leaving select
    /// mode goes back to the album.
    #[serde(skip)]
    pub adding_to: Option<u64>,
    /// Compact layout: rate-and-review mode on a photo (a tap on it): the bars slide away and an up
    /// or down swipe rates it (left half) or flags it (right half). `view.reviewMode`.
    #[serde(skip)]
    pub review: bool,
    /// Compact layout: the Edit tool's group (`profile`, `light`, `color`, `effects`, `detail`,
    /// `optics`, `lensBlur`, `calibration`); a phone shows one group at a time, as Lightroom's mobile app does.
    pub edit_group: String,
    /// Compact layout: light, dark, or like the system (`app.appearance.*`; Settings ▸ Interface).
    pub appearance: Appearance,
    /// Compact layout: how tall the tool sheet is, as the stop (0 small, 1 medium, 2 large) it was
    /// last left at (the grabber at its top snaps between them).
    pub sheet_detent: u8,
    /// Compact layout: the crop screen's tab, Aspect (false) or Geometry (true).
    #[serde(skip)]
    pub crop_geometry: bool,
    /// Compact layout: how many undo steps there were when a tool's screen (Crop, Remove, Masking)
    /// opened; its ✕ goes back to there.
    #[serde(skip)]
    pub screen_undo_base: Option<usize>,
    /// Compact layout: the masking screen's group of the selected mask's sliders shown in its sheet
    /// ("light", "color"…; empty: none).
    #[serde(skip)]
    pub mask_group: String,
    /// Compact layout: the searchable list of every command is open (the phone has no menu bar).
    #[serde(skip)]
    pub all_commands: bool,
    /// Compact layout: the Filter page is open, at its first list (`""`) or at one of the lists it
    /// opens (`type`, `camera`, `people`, `location`, `keyword`, `edited`); `view.filterSheet`.
    #[serde(skip)]
    pub filter_sheet: Option<String>,
    /// Compact layout: scroll the grid back to its top on the next frame (a tap on the top bar or,
    /// on iOS, on the status bar; `view.scrollToTop`).
    #[serde(skip)]
    pub scroll_top: bool,
    /// Compact layout: photos across a row of the grid (a pinch changes it; one of
    /// `panels::grid_zoom::STEPS`; `view.gridColumns`).
    pub grid_columns: u8,
    /// Compact layout: every photo, months or years (`view.gridSpan`).
    #[serde(skip)]
    pub grid_span: GridSpan,
    /// Compact layout: where the grid goes on its next frame (after a change of span).
    #[serde(skip)]
    pub grid_jump: Option<GridJump>,
    /// The grid last shown (justified or square): where Back from a photo goes.
    pub grid_mode: ViewMode,
    /// Compact layout: the page a name is typed on (a face, a person found in the photos, a
    /// person's new name, a face marked by hand), while it is open.
    #[serde(skip)]
    pub face_naming: Option<crate::panels::people_mobile::Naming>,
    /// Compact layout: the next tap on the photo marks a face there (`view.tagFace`).
    #[serde(skip)]
    pub tag_face: bool,
    /// Open the sync status popover this frame (`view.syncStatus`): under the top right corner,
    /// where a click on the cloud button would have put it.
    #[serde(skip)]
    pub sync_popover: bool,
    /// Help ▸ Debug ▸ Frame Rate (`view.frameRate`): the frame-rate display
    /// (`crate::frame_rate`). A debugging aid, never saved; `LIGHTCRAFT_FPS=1` starts with it on.
    #[serde(skip)]
    pub frame_rate: bool,
    pub view: ViewMode,
    pub left_panel: bool,
    pub right: RightPanel,
    /// The panel the right side showed before F8 (or Hide All Panels) hid it; F8 brings it back.
    pub right_hidden: RightPanel,
    /// Set while Hide All Panels (Shift+Tab) is in effect: what to bring back. Showing any panel
    /// by hand makes it stale (see [`UiState::all_panels_hidden`]).
    pub hidden_panels: Option<HiddenPanels>,
    /// Widths of the left sidebar and of the right panel in points (dragging their inner edge
    /// resizes them; within [`LEFT_WIDTH`] / [`RIGHT_WIDTH`]).
    pub left_width: f32,
    pub right_width: f32,
    /// Presets column open (opens to the left of the Edit panel).
    pub presets: bool,
    /// Presets column: show a live thumbnail of the photo with each preset.
    pub preset_thumbs: bool,
    /// The Presets panel also lists the presets and groups the user hid (dimmed), to show them again.
    pub show_hidden_presets: bool,
    pub filmstrip: bool,
    pub zoom: Zoom,
    /// Pan offset of the loupe when zoomed (image-normalized centre).
    pub pan: (f32, f32),
    /// Zoom applied by a click on the image (and Z / Space), in percent (100 = 1:1, 200 = 2:1);
    /// one of [`CLICK_ZOOMS`].
    pub click_zoom: u32,
    /// Animate the loupe rect toward its target (set by a click-zoom).
    #[serde(skip)]
    pub zoom_anim: bool,
    pub before_after: BeforeAfter,
    pub thumb_size: f32,
    /// Open Edit sections by id.
    pub open_sections: Vec<String>,
    /// Open flyouts (curve, mixer, grading…).
    pub open_flyouts: Vec<String>,
    /// Solo Mode (`view.soloSections`): opening one Edit section closes the others.
    pub single_panel: bool,
    pub show_clipping: bool,
    pub histogram: bool,
    /// Soft proofing (S in the loupe): render as `proof` would hold the photo.
    pub soft_proof: bool,
    pub proof: lightcraft_engine::pipeline::Proof,
    /// Soft proofing against a printer / paper ICC profile: (file, name), the intent and Simulate
    /// Paper & Ink. With a profile, `proof.profile` names its table.
    #[serde(skip)]
    pub proof_profile: Option<(String, String)>,
    #[serde(skip)]
    pub proof_intent: lightcraft_engine::proof::ProofIntent,
    pub proof_paper_ink: bool,
    /// Masking: show the selected mask as a rendered overlay (O), how (`MaskView` name, ⇧O cycles),
    /// in which colour and opacity (0..100, colour views), and whether pins are drawn.
    pub mask_overlay: bool,
    pub mask_overlay_mode: String,
    pub mask_overlay_color: [u8; 3],
    pub mask_overlay_opacity: f32,
    pub mask_pins: bool,
    /// When the Remove tool's spot pins and the mask pins show (Shift+H cycles). Mask pins also
    /// need [`mask_pins`](Self::mask_pins).
    pub tool_overlay: ToolOverlay,
    /// Masking: show every visible mask's region as one overlay at once (Ctrl+Shift+O / Cmd+Shift+O),
    /// instead of just the selected mask's ([`mask_overlay`]).
    pub mask_overlay_all: bool,
    /// The overlay (on, mode) to go back to when Show Luminance Map is turned off.
    #[serde(skip)]
    pub luminance_map_restore: Option<(bool, String)>,
    /// Mirroring of the triangle / spiral crop guides (0..4).
    pub crop_overlay_orient: u8,
    pub crop_overlay: CropOverlay,
    /// Last chosen aspect, retained across photos and launches; Original until a choice is made.
    #[serde(deserialize_with = "deserialize_crop_default_aspect")]
    pub crop_default_aspect: Option<serde_json::Value>,
    pub show_filenames: bool,
    /// What the square grid's caption shows: `filename`, `exposure` (shutter · aperture · ISO ·
    /// focal length) or `date`.
    pub grid_info: String,
    /// Photo counts next to sources and albums in the left panel.
    pub show_counts: bool,
    /// Face / pet boxes (read from XMP) over the photo in the loupe.
    pub face_boxes: bool,
    /// The left sidebar's sections: their order, which are hidden and which are folded shut.
    pub sidebar: crate::sidebar::SidebarLayout,
    /// What settings files said before [`UiState::sidebar`]: the ids of the folded sections. Read
    /// so they stay folded ([`UiState::sanitized`] moves them over), never written.
    #[serde(skip_serializing)]
    pub collapsed_sidebar: Vec<String>,
    /// A face's name being typed in the loupe.
    #[serde(skip)]
    pub name_edit: Option<NameEdit>,
    /// The person whose page the People view shows (their faces and the faces that look like them); `None`: everyone.
    #[serde(skip)]
    pub person_page: Option<String>,
    /// The person page a photo in the loupe was opened from (`None`: the People view's unnamed faces), and that photo:
    /// Escape goes back there (not to the grid).
    #[serde(skip)]
    pub person_from: Option<(Option<String>, u64)>,
    /// The last person page left: the People view offers a way back to it next to its title.
    #[serde(skip)]
    pub last_person: Option<String>,
    /// Assisted Culling's burst settings last confirmed (gap in seconds, similarity), for this
    /// session; `None` until the dialog is first confirmed (then 10 s, 0.93).
    #[serde(skip)]
    pub cull_burst: Option<(f32, f32)>,
    /// "More" faces the user hid with ×, for this session: (photo, region index).
    #[serde(skip)]
    pub dismissed_faces: std::collections::HashSet<(u64, usize)>,
    /// The unnamed faces selected in the People view (photo, region), the name being typed for them, the face last
    /// clicked (Shift-click selects a range from it) and whether the name box should take the keyboard.
    #[serde(skip)]
    pub unnamed_selected: std::collections::HashSet<(u64, usize)>,
    #[serde(skip)]
    pub unnamed_name: String,
    #[serde(skip)]
    pub unnamed_anchor: Option<usize>,
    #[serde(skip)]
    pub unnamed_focus: bool,
    /// Local sidebar locations hidden with “Remove from Local” (folders on disk are untouched).
    pub hidden_locations: Vec<String>,
    /// Files opened in an external editor (Photo ▸ Edit In), watched for what it saves: checked
    /// (`photo.checkExternalEdits`) when the window is focused again and at launch, so a save made
    /// after a restart, or while LightCraft was closed, still comes back. Paths, not photo ids: the
    /// list belongs to the app, not a library.
    pub external_edits: Vec<lightcraft_engine::cmd::TrackedEdit>,
    /// Whether the window had focus last frame (false at launch: the first focused frame checks
    /// the external edits).
    #[serde(skip)]
    pub was_focused: bool,
    /// When the watched folder was last scanned (monotonic app time, so a hidden window keeps
    /// scanning).
    #[serde(skip)]
    pub auto_import_at: f64,
    /// When the automatic XMP writes that failed were last tried again (monotonic app time), and how
    /// many were waiting then.
    #[serde(skip)]
    pub sidecar_retry_at: f64,
    #[serde(skip)]
    pub sidecars_waiting: usize,
    /// The cards and cameras already seen (volume roots) while Settings → Import → "Show the
    /// import review when a camera or card is connected" is on; `None` = not looked yet.
    #[serde(skip)]
    pub known_cards: Option<Vec<String>>,
    /// The last auto-import listing's warnings, shown once until they change.
    #[serde(skip)]
    pub auto_import_warned: Vec<String>,
    /// The develop control whose slider is being dragged (geometry sliders show a grid).
    #[serde(skip)]
    pub dragging_control: Option<String>,
    /// Alt (Option on macOS) is held this frame: a dragged Edit slider shows its temporary
    /// preview ([`lightcraft_pipeline::Overlay::for_control`]).
    #[serde(skip)]
    pub alt_held: bool,
    pub search: String,
    /// Where the Map was looking (latitude, longitude, zoom) when it was last used.
    pub map_view: Option<(f64, f64, f64)>,
    /// Focus the search field on the next frame (Edit → Find…).
    #[serde(skip)]
    pub focus_search: bool,
    /// Put the cursor in the Keywords panel's Add keyword box (Add Keywords): the egui pass it was
    /// asked on. A box drawn more than [`FOCUS_KEYWORDS_PASSES`] passes later doesn't take it, so
    /// a request made while the panel can't show expires.
    #[serde(skip)]
    pub focus_keywords: Option<u64>,
    /// A mask being renamed in the Masks list: its id and the edited name.
    #[serde(skip)]
    pub renaming_mask: Option<(u32, String)>,
    /// The Describe field (AI mask from a text prompt) while open: how the selection combines
    /// (`new` mask, or `add`/`subtract`/`intersect` on the selected one) and the text typed.
    #[serde(skip)]
    pub describe: Option<(String, String)>,
    /// A SAM 3 download was started from the app (to report its end once).
    #[serde(skip)]
    pub sam_downloading: bool,
    /// Settings ▸ AI Models: the model whose download or delete is waiting for the user's yes.
    #[serde(skip)]
    pub model_confirm: Option<(String, ModelStep)>,
    /// The search field describes a photo (search by description) instead of matching text.
    #[serde(skip)]
    pub ai_search: bool,
    /// Why the last search by description, download or indexing failed (shown under the field).
    #[serde(skip)]
    pub ai_search_error: Option<String>,
    /// The search model download was started from the app (to report its end once).
    #[serde(skip)]
    pub ai_search_downloading: bool,
    /// The user ticked "search the words in photos" and is being asked to download the models.
    #[serde(skip)]
    pub ai_text_offer: bool,
    /// The text-reading models download was started from the app (to report its end once).
    #[serde(skip)]
    pub ai_text_downloading: bool,
    /// The user asked to find people and is being asked to download the face models.
    #[serde(skip)]
    pub faces_offer: bool,
    /// The face models download was started from the app (to report its end once).
    #[serde(skip)]
    pub faces_downloading: bool,
    /// Why finding people, naming or forgetting failed (shown in the People view).
    #[serde(skip)]
    pub people_error: Option<String>,
    /// People found in photos as listed by the engine, with the state they were listed at.
    #[serde(skip)]
    pub people_found: Option<((usize, usize, u64, bool, u64), std::sync::Arc<Vec<serde_json::Value>>)>,
    /// Names being typed for people found in photos, by the person's id.
    #[serde(skip)]
    pub people_names: std::collections::HashMap<String, String>,
    /// Indexing was started for this switching-on (it is offered, not repeated, after).
    #[serde(skip)]
    pub ai_search_autostarted: bool,
    /// Photos the index can hold, with the catalog revision they were counted at.
    #[serde(skip)]
    pub ai_search_total: (u64, usize),
    /// When to start the zoomed-in detail pass of an AI mask (app time) and which mask: set by
    /// each click or description, so the pass runs once the clicking stops (monotonic app time).
    #[serde(skip)]
    pub detail_due: Option<(f64, u32)>,
    /// A mask component being renamed inline: (mask id, component index, name).
    pub renaming_component: Option<(u32, usize, String)>,
    /// Close the window on the next frame (File → Quit).
    #[serde(skip)]
    pub quit: bool,
    /// Photos being dragged from the grid (dropped on an album to add them).
    #[serde(skip)]
    pub dragging_photos: Option<Vec<u64>>,
    /// An album just made: the Albums tree opens the folders down to it, once.
    #[serde(skip)]
    pub reveal_album: Option<u64>,
    /// A keyword just made from the sidebar: Keywords opens down to it, once.
    #[serde(skip)]
    pub reveal_keyword: Option<String>,
    /// A folder Photo ▸ Show in Folders went to (its Folders row's path): the sidebar's Folders
    /// section opens down to it and scrolls to it, once.
    #[serde(skip)]
    pub reveal_folder: Option<String>,
    /// An album or folder row being dragged in the sidebar (dropped on a folder to move it there).
    #[serde(skip)]
    pub dragging_album: Option<u64>,
    /// A sidebar section being dragged by its header (dropped between sections to move it there).
    #[serde(skip)]
    pub dragging_section: Option<crate::sidebar::SidebarSection>,
    /// Selected curve channel in the Curve flyout.
    pub curve_channel: String,
    /// Selected mixer mode: "hue" | "saturation" | "luminance" | "all".
    pub mixer_mode: String,
    /// Selected colour grading tab: "3way" | "shadows" | "midtones" | "highlights".
    pub grading_mode: String,
    /// Colour grading wheels (shadows, midtones, highlights) hidden for viewing by their eye, and
    /// the photo that applies to: not an edit, and forgotten on moving to another photo or quitting.
    #[serde(skip)]
    pub grading_hidden: Option<(lightcraft_catalog::PhotoId, [bool; 3])>,
    /// Active on-canvas tool: "", "brush", "linear", "radial", "wbPicker", "straighten", "remove".
    pub tool: String,
    /// Guided Upright: the selected guide (photo id + index) while the tool is open. A Delete /
    /// Backspace on it removes that guide alone; a click away or leaving the tool clears it.
    #[serde(skip)]
    pub selected_guide: Option<(u64, usize)>,
    pub brush_size: f32,
    pub brush_feather: f32,
    pub brush_flow: f32,
    pub brush_erase: bool,
    /// Brush Auto Mask: dabs stick to areas like the one under the brush centre.
    pub brush_auto_mask: bool,
    /// Brush Pen Pressure: a pen's pressure scales each dab's size and flow (a mouse paints at full pressure).
    pub brush_pressure: bool,
    /// Remove tool brush: size (fraction of the long edge), feather and opacity (0..100).
    pub remove_size: f32,
    pub remove_feather: f32,
    pub remove_opacity: f32,
    /// Remove tool ▸ Generative AI: new Remove strokes are filled by the AI Remove model (saved in
    /// `ui.json`, so it stays as the user left it).
    pub remove_ai: bool,
    /// Remove tool ▸ Generative AI ▸ High quality: paint with LaMa (larger, slower) when it is downloaded and this device runs it.
    pub remove_hq: bool,
    /// Crop ▸ Expand ▸ Fill With: the Generative Expand model the user picked (`migan`, `lama`,
    /// `classic`; empty: the best one on this device). Saved in `ui.json`.
    pub expand_model: String,
    /// The browser build: a Generate waiting for its notice to be drawn (it runs on the page's
    /// thread), and whether a frame has passed.
    #[serde(skip)]
    pub expand_pending: Option<(String, bool)>,
    /// Selected Point Color sample.
    pub point_color: usize,
    /// Point Color "Visualize range": the selected sample's range in colour, the rest grey.
    pub point_color_visualize: bool,
    /// Light panel "Visualize HDR": grey below SDR white, colour bands above (HDR edits).
    pub hdr_visualize: bool,
    /// Lens Blur "Visualize Depth": the depth the blur uses, warm near and cool far.
    pub depth_visualize: bool,
    /// Lens Blur asked for its depth model on a photo without depth: the panel offers the download.
    pub lens_blur_offer: bool,
    /// Lens Blur's Refine brush: `focus`, `blur` or `erase` (painted with the `lensBrush` tool).
    pub lens_brush: String,
    /// Red Eye panel: selected correction, and whether new ones are pet eyes.
    pub eye: usize,
    pub eye_pet: bool,
    /// Remove tool: Visualize Spots (high-pass black/white view) and its threshold 0..100.
    pub visualize_spots: bool,
    pub spots_threshold: f32,
    /// The library filter bar above the grid.
    pub filter_bar: bool,
    /// Folders kept in Local (Browse Folder…, Keep in Local / Add to Local; `local.addRoot`):
    /// listed after the built-in locations, across restarts, whatever is browsed.
    pub local_roots: Vec<String>,
    /// The top-level Local row listed for this session only, for a browsed folder outside every
    /// kept location; it stays while browsing below it (see `panels::left::local_places`).
    #[serde(skip)]
    pub local_browse_root: Option<String>,
    /// Culling: after a rating, flag or colour-label key, move to the next photo.
    pub auto_advance: bool,
    /// Full-screen preview (F): the photo alone on black, no chrome.
    pub fullscreen: bool,
    /// Lights Out (Shift+L): not saved, every launch starts with the lights on.
    #[serde(skip)]
    pub lights_out: LightsOut,
    /// Window ▸ Second Window.
    pub second_window: bool,
    /// The keyword painter: clicking a photo in the grid toggles this keyword on it.
    #[serde(skip)]
    pub keyword_painter: Option<String>,
    /// What the Keywording box shows (Lightroom Classic's Keyword Tags views).
    pub keywording_view: KeywordingView,
    /// The Keywording box, grouped: every keyword of this selection (its hash) is shown, however
    /// many lines that takes ("+N more" was clicked). Another selection's list is short again.
    #[serde(skip)]
    pub keyword_chips_all: Option<u64>,
    /// Sections of the right panel folded shut by a click on their title (`keywording`,
    /// `keywordList`); the others are open.
    #[serde(deserialize_with = "lenient_ids")]
    pub folded_sections: Vec<String>,
    /// The Keyword List's open levels (lower-case paths).
    pub keyword_list_open: Vec<String>,
    /// The keyword picked in the Keyword List (− deletes it, Edit edits it).
    #[serde(skip)]
    pub keyword_list_selected: Option<String>,
    /// A keyword being dragged in the Keyword List (onto another to nest it).
    #[serde(skip)]
    pub dragging_keyword: Option<String>,
    /// A running slideshow (full screen; see [`crate::panels::slideshow`]).
    #[serde(skip)]
    pub slideshow: Option<Box<crate::panels::slideshow::Run>>,
    /// Info overlay on the loupe.
    pub info_overlay: InfoOverlay,
    /// When the loupe last changed photo (View Options' Show Briefly When Photo Changes).
    #[serde(skip)]
    pub brief_info: BriefInfo,
    /// The loupe's layout image overlay (View ▸ Loupe Overlay).
    pub layout_image: LayoutImage,
    /// Navigator mini map in the loupe while zoomed in.
    pub navigator: bool,
    /// App preferences (Settings dialog).
    pub settings: AppSettings,
    /// Window full screen requested (⇧⌘F); the host applies it (`None` = no change pending).
    #[serde(skip)]
    pub window_fullscreen: Option<bool>,
    /// Compare view: (select, candidate) photo ids.
    #[serde(skip)]
    pub compare: Option<(u64, u64)>,
    /// Reference view: the reference photo.
    #[serde(skip)]
    pub reference: Option<u64>,
    /// Transient toast text, expiry (seconds of app time), and optional colour-label styling.
    #[serde(skip)]
    pub toast: Option<(String, f64, Option<lightcraft_catalog::ColorLabel>)>,
    /// The result of the last Find Missing Photos (it searches in the background).
    #[serde(skip)]
    pub last_find_missing: Option<serde_json::Value>,
    /// The outcome (success or error) of the last Find Missing Photos task.
    #[serde(skip)]
    pub last_find_missing_result: Option<Result<serde_json::Value, String>>,
    /// Injected delay for Find Missing Photos worker (test-only, for timeout testing).
    #[cfg(test)]
    #[serde(skip)]
    pub find_missing_test_delay: Option<std::time::Duration>,
    /// The library (its `Session::library_generation`) whose preview-only raws were rechecked
    /// (`crate::recheck`).
    #[serde(skip)]
    pub rechecked_library: Option<u64>,
    #[serde(skip)]
    pub status: String,
    #[serde(skip)]
    pub dialog: Option<Dialog>,
    /// Compact layout: the dialog that just closed, drawn while its page slides away.
    #[serde(skip)]
    pub dialog_leaving: Option<Dialog>,
    /// The Print dialog's page, selection and drag.
    #[serde(skip)]
    pub print: crate::print_view::PrintUi,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            window: None,
            docking: None,
            docking_version: crate::docking::DOCKING_VERSION,
            language: crate::i18n::default_language(),
            preview_build_seen: None,
            unsaved_seen: false,
            select_mode: false,
            adding_to: None,
            review: false,
            edit_group: "light".into(),
            appearance: Appearance::System,
            sheet_detent: 1,
            crop_geometry: false,
            screen_undo_base: None,
            mask_group: String::new(),
            all_commands: false,
            filter_sheet: None,
            scroll_top: false,
            grid_columns: 3,
            grid_span: GridSpan::All,
            grid_jump: None,
            grid_mode: ViewMode::PhotoGrid,
            face_naming: None,
            tag_face: false,
            sync_popover: false,
            frame_rate: std::env::var("LIGHTCRAFT_FPS").is_ok_and(|v| matches!(v.trim(), "1" | "on" | "true" | "yes")),
            luminance_map_restore: None,
            view: ViewMode::Detail,
            map_view: None,
            left_panel: false,
            left_width: LEFT_WIDTH.default,
            right_width: RIGHT_WIDTH.default,
            right: RightPanel::Edit,
            right_hidden: RightPanel::None,
            hidden_panels: None,
            presets: false,
            preset_thumbs: false,
            show_hidden_presets: false,
            filmstrip: true,
            zoom: Zoom::Fit,
            pan: (0.5, 0.5),
            click_zoom: 100,
            zoom_anim: false,
            before_after: BeforeAfter::Off,
            thumb_size: 220.0,
            open_sections: vec!["light".into()],
            open_flyouts: vec![],
            single_panel: false,
            show_clipping: false,
            histogram: true,
            soft_proof: false,
            proof: lightcraft_engine::pipeline::Proof { dest_warning: false, ..Default::default() },
            proof_profile: None,
            proof_intent: lightcraft_engine::proof::ProofIntent::Perceptual,
            proof_paper_ink: false,
            mask_overlay: true,
            mask_overlay_mode: "color".into(),
            mask_overlay_color: [230, 30, 40],
            mask_overlay_opacity: 50.0,
            mask_pins: true,
            tool_overlay: ToolOverlay::Auto,
            mask_overlay_all: false,
            crop_overlay: CropOverlay::Thirds,
            crop_default_aspect: Some(serde_json::json!("original")),
            crop_overlay_orient: 0,
            show_filenames: true,
            grid_info: "filename".into(),
            show_counts: true,
            face_boxes: true,
            sidebar: Default::default(),
            collapsed_sidebar: Vec::new(),
            name_edit: None,
            person_page: None,
            person_from: None,
            last_person: None,
            cull_burst: None,
            dismissed_faces: Default::default(),
            unnamed_selected: Default::default(),
            unnamed_name: String::new(),
            unnamed_anchor: None,
            unnamed_focus: false,
            hidden_locations: Vec::new(),
            dragging_control: None,
            alt_held: false,
            external_edits: Vec::new(),
            was_focused: false,
            auto_import_at: 0.0,
            sidecar_retry_at: 0.0,
            sidecars_waiting: 0,
            known_cards: None,
            auto_import_warned: Vec::new(),
            search: String::new(),
            focus_search: false,
            focus_keywords: None,
            renaming_mask: None,
            describe: None,
            detail_due: None,
            sam_downloading: false,
            model_confirm: None,
            ai_search: false,
            ai_search_error: None,
            ai_search_downloading: false,
            ai_text_offer: false,
            ai_text_downloading: false,
            faces_offer: false,
            faces_downloading: false,
            people_error: None,
            people_found: None,
            people_names: Default::default(),
            ai_search_autostarted: false,
            ai_search_total: (0, 0),
            renaming_component: None,
            quit: false,
            dragging_photos: None,
            reveal_album: None,
            reveal_keyword: None,
            reveal_folder: None,
            dragging_album: None,
            dragging_section: None,
            curve_channel: "parametric".into(),
            mixer_mode: "hue".into(),
            grading_mode: "3way".into(),
            grading_hidden: None,
            tool: String::new(),
            selected_guide: None,
            brush_size: 0.04,
            brush_feather: 50.0,
            brush_flow: 60.0,
            brush_erase: false,
            brush_auto_mask: false,
            brush_pressure: true,
            remove_size: 0.02,
            remove_feather: 50.0,
            remove_opacity: 100.0,
            remove_ai: false,
            remove_hq: false,
            expand_model: String::new(),
            expand_pending: None,
            point_color: 0,
            point_color_visualize: false,
            hdr_visualize: false,
            depth_visualize: false,
            lens_blur_offer: false,
            lens_brush: "focus".into(),
            eye: 0,
            eye_pet: false,
            visualize_spots: false,
            spots_threshold: 50.0,
            auto_advance: false,
            fullscreen: false,
            lights_out: LightsOut::On,
            slideshow: None,
            second_window: false,
            keyword_painter: None,
            keywording_view: KeywordingView::default(),
            keyword_chips_all: None,
            folded_sections: Vec::new(),
            keyword_list_open: Vec::new(),
            keyword_list_selected: None,
            dragging_keyword: None,
            info_overlay: InfoOverlay::Off,
            brief_info: BriefInfo::default(),
            layout_image: LayoutImage::default(),
            navigator: true,
            settings: AppSettings::default(),
            window_fullscreen: None,
            filter_bar: false,
            local_roots: Vec::new(),
            local_browse_root: None,
            compare: None,
            reference: None,
            toast: None,
            last_find_missing: None,
            last_find_missing_result: None,
            #[cfg(test)]
            find_missing_test_delay: None,
            rechecked_library: None,
            status: String::new(),
            dialog: None,
            dialog_leaving: None,
            print: Default::default(),
        }
    }
}

impl UiState {
    /// Which colour grading wheels (shadows, midtones, highlights) are hidden for viewing `photo`.
    pub fn grading_hidden_for(&self, photo: lightcraft_catalog::PhotoId) -> [bool; 3] {
        match self.grading_hidden {
            Some((p, h)) if p == photo => h,
            _ => [false; 3],
        }
    }

    /// Show or hide colour grading wheel `i` (0 shadows, 1 midtones, 2 highlights) on `photo`.
    pub fn set_grading_hidden(&mut self, photo: lightcraft_catalog::PhotoId, i: usize, hidden: bool) {
        let mut h = self.grading_hidden_for(photo);
        if let Some(slot) = h.get_mut(i) {
            *slot = hidden;
        }
        self.grading_hidden = h.iter().any(|x| *x).then_some((photo, h));
    }

    /// A right-panel section that folds by its title is folded shut.
    pub fn section_folded(&self, id: &str) -> bool {
        self.folded_sections.iter().any(|s| s == id)
    }
    pub fn toggle_folded(&mut self, id: &str) {
        if self.section_folded(id) {
            self.folded_sections.retain(|s| s != id);
        } else {
            self.folded_sections.push(id.to_string());
        }
    }

    pub fn section_open(&self, id: &str) -> bool {
        self.open_sections.iter().any(|s| s == id)
    }
    pub fn toggle_section(&mut self, id: &str) {
        if self.section_open(id) {
            self.open_sections.retain(|s| s != id);
        } else if self.single_panel {
            self.solo_section(id);
        } else {
            self.open_sections.push(id.to_string());
        }
    }
    /// Opens `id` and closes every other Edit section (⌥-click on a section header).
    pub fn solo_section(&mut self, id: &str) {
        self.open_sections.clear();
        self.open_sections.push(id.to_string());
    }
    /// Turns Solo Mode on or off. Turning it on keeps only the section opened last.
    pub fn set_single_panel(&mut self, on: bool) {
        self.single_panel = on;
        if on {
            let keep = self.open_sections.len().saturating_sub(1);
            self.open_sections.drain(..keep);
        }
    }
    /// Whether the sidebar section with this id is folded shut (an unknown id: no).
    pub fn sidebar_section_collapsed(&self, id: &str) -> bool {
        crate::sidebar::SidebarSection::from_id(id).is_some_and(|s| self.sidebar.is_collapsed(s))
    }
    pub fn flyout_open(&self, id: &str) -> bool {
        self.open_flyouts.iter().any(|s| s == id)
    }
    pub fn toggle_flyout(&mut self, id: &str) {
        if self.flyout_open(id) {
            self.open_flyouts.retain(|s| s != id);
        } else {
            self.open_flyouts.push(id.to_string());
        }
    }
    /// Clamp values restored from disk.
    pub fn sanitized(mut self) -> Self {
        self.thumb_size = self.thumb_size.clamp(THUMB_SIZE_MIN, THUMB_SIZE_MAX);
        self.grid_columns = crate::panels::grid_zoom::snap(self.grid_columns);
        if !matches!(self.grid_mode, ViewMode::PhotoGrid | ViewMode::SquareGrid) {
            self.grid_mode = ViewMode::PhotoGrid;
        }
        self.left_width = LEFT_WIDTH.clamp(self.left_width);
        self.right_width = RIGHT_WIDTH.clamp(self.right_width);
        self.brush_size = self.brush_size.clamp(0.002, 0.5);
        self.sidebar.adopt_collapsed(&std::mem::take(&mut self.collapsed_sidebar));
        self.dialog = None;
        self.fullscreen = false;
        if !crate::state::PREVIEW_LIMITS.contains(&self.settings.preview_limit) {
            self.settings.preview_limit = AppSettings::default().preview_limit;
        }
        match self.settings.startup_view {
            StartupView::Last => {}
            StartupView::Grid => self.view = ViewMode::PhotoGrid,
            StartupView::Detail => self.view = ViewMode::Detail,
        }
        self
    }
}

/// A list of ids read leniently: what isn't a list is none, and what isn't text in it is left out.
fn lenient_ids<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(value.as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect())
}

fn deserialize_crop_default_aspect<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<serde_json::Value>, D::Error> {
    // Legacy null means no choice yet. An explicitly unlocked crop is saved as "free".
    Ok(Option::<serde_json::Value>::deserialize(d)?.or_else(|| Some(serde_json::json!("original"))))
}

/// The navigation/document/modal/panel state a gesture was admitted under. The same automation
/// id can mean a different control after any of these changes; the request is canceled rather
/// than retargeted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GestureFrame {
    pub(crate) view: crate::state::ViewMode,
    pub(crate) active: Option<lightcraft_catalog::PhotoId>,
    pub(crate) library: u64,
    pub(crate) dialog: Option<std::mem::Discriminant<crate::state::Dialog>>,
    pub(crate) left_panel: bool,
    pub(crate) right: crate::state::RightPanel,
    pub(crate) presets: bool,
    pub(crate) filmstrip: bool,
}

impl UiState {
    /// Every side panel and the filmstrip are hidden.
    fn no_panel_shown(&self) -> bool {
        !self.left_panel && self.right == RightPanel::None && !self.presets && !self.filmstrip
    }

    /// Hide All Panels is in effect: its snapshot is kept and nothing has been shown since. (Showing
    /// a panel by hand drops it out of this state, so the next Shift+Tab hides again instead of
    /// bringing back panels the user had put away.)
    pub fn all_panels_hidden(&self) -> bool {
        self.hidden_panels.is_some() && self.no_panel_shown()
    }

    /// F8: hide the right panel, or show the one that was there (Edit when there was none).
    /// `show` forces the state. Returns whether the panel is shown now.
    pub fn toggle_right_panel(&mut self, show: Option<bool>) -> bool {
        let show = show.unwrap_or(self.right == RightPanel::None);
        if show && self.right == RightPanel::None {
            self.right = self.right_hidden.restored();
        } else if !show && self.right != RightPanel::None {
            self.right_hidden = self.right.restored();
            self.right = RightPanel::None;
        }
        self.right != RightPanel::None
    }

    /// Shift+Tab: hide the side panels, the Presets column and the filmstrip, remembering what was
    /// shown; again, bring back exactly that. When nothing is shown and nothing was remembered, it
    /// shows the usual panels (left sidebar, the last right panel, filmstrip). `hide` forces the
    /// direction. Returns whether the panels are hidden now.
    pub fn toggle_all_panels(&mut self, hide: Option<bool>) -> bool {
        let hide = hide.unwrap_or_else(|| !self.all_panels_hidden() && !(self.no_panel_shown() && self.hidden_panels.is_none()));
        if hide {
            if self.no_panel_shown() {
                return true;
            }
            self.hidden_panels =
                Some(HiddenPanels { left_panel: self.left_panel, right: self.right, presets: self.presets, filmstrip: self.filmstrip });
            if self.right != RightPanel::None {
                self.right_hidden = self.right.restored();
            }
            self.left_panel = false;
            self.right = RightPanel::None;
            self.presets = false;
            self.filmstrip = false;
        } else {
            match self.hidden_panels.take() {
                Some(was) if self.no_panel_shown() => {
                    self.left_panel = was.left_panel;
                    self.right = if was.right == RightPanel::None { RightPanel::None } else { was.right.restored() };
                    self.presets = was.presets;
                    self.filmstrip = was.filmstrip;
                }
                _ => {
                    self.hidden_panels = None;
                    self.left_panel = true;
                    if self.right == RightPanel::None {
                        self.right = self.right_hidden.restored();
                    }
                    self.filmstrip = true;
                }
            }
        }
        self.all_panels_hidden()
    }
}

/// A pointer press delivered to the UI, kept so a canceled gesture knows that a button is down.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InjectedPress {
    pub(crate) button: egui::PointerButton,
}

/// A file `ui.dropFiles` delivers as dropped, as a real OS drop arrives in `raw.dropped_files`.
#[derive(Debug)]
pub(crate) struct InjectedDrop(pub(crate) std::path::PathBuf);

impl egui::DroppedFile for InjectedDrop {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }

    #[cfg(target_arch = "wasm32")]
    fn bytes_async(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + '_>> {
        Box::pin(async { Err("the web build has no files to read".to_string()) })
    }
}
