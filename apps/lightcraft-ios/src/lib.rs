//! LightCraft on iOS (`docs/ios.md`): the egui UI (its compact touch layout on phones), started
//! from the C `main` of the Xcode project in `xcode/`. The library is saved in the app's Documents
//! folder (new ones start with the demo photos) and can sync with a self-hosted server (Settings ▸
//! Sync, `docs/sync.md`). The native pieces come from `lightcraft-ios-host`: photos are imported
//! from the Photos and Files pickers (copied into the library), exports go to the share sheet,
//! HEIC is decoded by ImageIO, touch gestures and choices tap the Taptic Engine, and the app saves,
//! pauses the GPU and frees memory as iOS asks.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::cell::RefCell;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_engine::Session;
use lightcraft_ios_host::{BackgroundTask, InterfaceStyle, Lifecycle, PickKind, Picked};
use lightcraft_ui_egui::haptics::Haptic;
use lightcraft_ui_egui::prefs::PrefsWriter;
use lightcraft_ui_egui::{LightcraftApp, PickSource, SaveDone, Services, ShareExports};
use serde_json::json;

/// Picks the pickers delivered (from any thread), joined to the app on the next frame.
type Inbox = Arc<Mutex<Vec<Picked>>>;

/// The app and what the host keeps beside it. Shared (`Rc<RefCell<…>>`) between eframe, which
/// drives the frames, and UIKit's lifecycle notifications, which arrive between frames.
struct Host {
    app: LightcraftApp,
    prefs: PrefsWriter,
    inbox: Inbox,
    /// Time to finish an export after the app went to the background.
    background: Option<BackgroundTask>,
    /// UIKit's safe area as egui-winit last read it (it reads it only on some window events).
    safe_area: egui::SafeAreaInsets,
    /// The style the app's windows were last forced to (so UIKit's chrome follows what the app draws).
    style: Option<InterfaceStyle>,
}

impl Host {
    fn logic(&mut self, ctx: &egui::Context) {
        let picks = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(PoisonError::into_inner));
        for p in picks {
            self.joined(ctx, p);
        }
        self.app.logic(ctx);
        self.prefs.tick(&mut self.app, ctx);
        self.follow_appearance(ctx);
        // an export kept going in the background has finished
        if self.background.is_some() && self.app.export.is_none() {
            self.background = None;
        }
    }

    /// The status bar, the keyboard and the system's sheets take the style the app is drawn in
    /// (light, dark, or the system's own); with "System" the system's style is looked at again
    /// every couple of seconds, as nothing tells the app when it changes.
    fn follow_appearance(&mut self, ctx: &egui::Context) {
        let want = if self.app.dark { InterfaceStyle::Dark } else { InterfaceStyle::Light };
        if self.style != Some(want) {
            match lightcraft_ios_host::set_interface_style(Some(want)) {
                Ok(()) => self.style = Some(want),
                Err(e) => log::warn!("appearance: {e}"),
            }
        }
        if self.app.ui.appearance == lightcraft_ui_egui::state::Appearance::System {
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
    }

    /// Picked photos (copies in the staging folder) into the import review.
    fn joined(&mut self, ctx: &egui::Context, p: Picked) {
        if let Some(msg) = picked_message(&p) {
            log::warn!("{msg}: {:?}", p.failed);
            self.app.toast(ctx, msg);
        }
        if p.files.is_empty() {
            return;
        }
        let paths: Vec<String> = p.files.iter().map(|f| f.to_string_lossy().to_string()).collect();
        self.app.ui.view = lightcraft_ui_egui::state::ViewMode::PhotoGrid;
        if let Err(e) = self.app.run("file.addPhotos", json!({"paths": paths, "staged": true})) {
            self.app.toast(ctx, e);
        }
    }

    /// Write everything that isn't on disk yet: the app may be ended without notice once it is in
    /// the background.
    fn save(&mut self) {
        if let Err(e) = self.app.session.persist() {
            log::error!("saving the library: {e}");
        }
        self.app.session.save_view();
        if let Err(e) = self.prefs.save(&self.app) {
            log::error!("{e}");
        }
    }

    fn lifecycle(&mut self, event: Lifecycle) {
        log::info!("lifecycle: {event:?}");
        match event {
            Lifecycle::WillResignActive => self.save(),
            Lifecycle::DidEnterBackground => {
                self.save();
                // an export in progress gets the time the system allows to finish
                if self.app.export.is_some() && self.background.is_none() {
                    self.background = BackgroundTask::begin("LightCraft export");
                }
                // no GPU in the background, and a much smaller memory limit
                lightcraft_engine::gpu::pause();
                let freed = self.app.session.release_memory();
                log::info!("background: {} MB of decoded photos released", freed >> 20);
            }
            Lifecycle::WillEnterForeground | Lifecycle::DidBecomeActive => {
                lightcraft_engine::gpu::resume();
                self.background = None;
            }
            Lifecycle::MemoryWarning => {
                let freed = self.app.session.release_memory();
                log::warn!("memory warning: {} MB of decoded photos released", freed >> 20);
            }
            Lifecycle::WillTerminate => {
                self.save();
                if let Err(e) = self.app.session.close_library() {
                    log::error!("closing the library: {e}");
                }
            }
        }
    }
}

/// The keyboard's Return as egui wants it: an Enter key press (it confirms a text field). winit
/// hands it over as a `"\n"` character, which egui drops (`docs/ios-gaps.md`, A1.9).
fn press_enter(raw: &mut egui::RawInput, times: u32) {
    for _ in 0..times.min(8) {
        for pressed in [true, false] {
            raw.events.push(egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
        }
    }
}

/// Text fields and the iOS keyboard. egui asks to "interrupt the IME composition" each time a
/// focused `TextEdit` is tapped (it requests focus again), and egui-winit does that with
/// `resignFirstResponder` + `becomeFirstResponder`: the keyboard dropped and came back on every tap.
/// winit's iOS view takes plain text (no marked text), so there is nothing to interrupt: drop the
/// request. And egui-winit keeps copied text inside the app on iOS: put it on the pasteboard too.
struct TextInput;

impl egui::Plugin for TextInput {
    fn debug_name(&self) -> &'static str {
        "lightcraft-ios-text-input"
    }
    fn output_hook(&mut self, _ctx: &egui::Context, output: &mut egui::FullOutput) {
        if let Some(ime) = &mut output.platform_output.ime {
            ime.should_interrupt_composition = false;
        }
        for c in &output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = c {
                lightcraft_ios_host::set_pasteboard_text(text);
            }
        }
    }
}

/// The safe area with the on-screen keyboard counted in, so the UI (`content_rect`: panels,
/// sheets, dialogs) stays above it and the field being typed in isn't covered.
fn above_keyboard(mut safe: egui::SafeAreaInsets, keyboard: f32) -> egui::SafeAreaInsets {
    safe.0.bottom = safe.0.bottom.max(keyboard);
    safe
}

/// What to tell the user about a pick that didn't fully work.
fn picked_message(p: &Picked) -> Option<String> {
    let n = p.failed.len();
    match (n, p.files.len()) {
        (0, _) => None,
        (n, 0) => Some(format!("{n} item{} couldn't be copied from the picker", if n == 1 { "" } else { "s" })),
        (n, ok) => Some(format!("{ok} photo{} copied; {n} couldn't be", if ok == 1 { "" } else { "s" })),
    }
}

struct App(Rc<RefCell<Host>>);

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(mut h) = self.0.try_borrow_mut() {
            h.logic(ctx);
        }
    }
    /// What shows behind the UI: in the status bar and home indicator areas, which the bars' colour
    /// (black, or white in the light appearance) should fill, not eframe's dark grey.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let dark = self.0.try_borrow().map_or(true, |h| h.app.dark);
        (if dark { egui::Color32::BLACK } else { egui::Color32::WHITE }).to_normalized_gamma_f32()
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        press_enter(raw, lightcraft_ios_host::take_return_presses());
        // winit ignores the theme on iOS: the system's style is read from UIKit (the main screen's
        // traits, which the app forcing a style on its windows doesn't change)
        raw.system_theme = lightcraft_ios_host::system_interface_style().map(|s| match s {
            InterfaceStyle::Light => egui::Theme::Light,
            InterfaceStyle::Dark => egui::Theme::Dark,
        });
        if let Ok(mut h) = self.0.try_borrow_mut() {
            if let Some(s) = raw.safe_area_insets {
                h.safe_area = s;
            }
            raw.safe_area_insets = Some(above_keyboard(h.safe_area, lightcraft_ios_host::keyboard_height()));
            h.app.raw_input_hook(raw);
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // the root ui spans the whole screen; keep the panels out of the status bar, Dynamic Island and
        // home indicator (egui-winit reads the insets from UIKit into `content_rect`)
        let safe = ui.ctx().content_rect();
        if let Ok(mut h) = self.0.try_borrow_mut() {
            ui.scope_builder(egui::UiBuilder::new().max_rect(safe), |ui| h.app.ui(ui));
        }
    }
    fn on_exit(&mut self) {
        if let Ok(mut h) = self.0.try_borrow_mut() {
            h.lifecycle(Lifecycle::WillTerminate);
        }
    }
}

/// Sends every log record to the system log (NSLog, through the Objective-C host) and appends it to
/// `lightcraft-ios.log` in the app's tmp dir: a bundled app's stderr isn't shown anywhere, and winit
/// exits the process on iOS when eframe fails to start.
struct FileLog;

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        use std::io::Write;
        let line = format!("{} {}: {}", r.level(), r.target(), r.args());
        // the device console too (`idevicesyslog | grep LightCraft`, Console.app)
        lightcraft_ios_host::console(&line);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("lightcraft-ios.log")) {
            let _ = writeln!(f, "{line}");
        }
    }
    fn flush(&self) {}
}

/// egui-wgpu's default device limits ask for more than some Metal adapters offer (the simulator
/// allows 15 inter-stage shader variables, the default wants 16), so request defaults clamped to
/// what the adapter has.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuSetup, wgpu};
    let mut c = eframe::egui_wgpu::WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(n) = &mut c.wgpu_setup {
        n.device_descriptor = std::sync::Arc::new(|adapter| wgpu::DeviceDescriptor {
            label: Some("lightcraft device"),
            required_limits: wgpu::Limits { max_texture_dimension_2d: 8192, ..wgpu::Limits::default() }.or_worse_values_from(&adapter.limits()),
            ..Default::default()
        });
    }
    c
}

/// Where the library lives: `$LIGHTCRAFT_LIBRARY`, else `LightCraft Library` in the app's Documents
/// folder (on iOS `$HOME` is the app's container: kept across launches and updates, backed up).
pub fn library_dir(library: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    lightcraft_engine::library::default_dir_in(library, home, true)
}

/// The library on the device (a new one starts with the demo photos), or, when it can't be opened,
/// the in-memory demo library — nothing saved, no sync — with the reason in the log.
pub fn open_session(dir: Option<&Path>) -> Session {
    match dir {
        Some(dir) => {
            let mut s = Session::new().with_fs().with_system_clock();
            match s.open_library(dir, true) {
                Ok(r) => {
                    let replayed = r.replayed;
                    log::info!("library {}: {} photos, {replayed} log records replayed", dir.display(), s.catalog.len());
                    return s;
                }
                Err(e) => log::error!("can't open the library {}: {e}; using an in-memory demo library (not saved, can't sync)", dir.display()),
            }
        }
        None => log::error!("no home folder for the library; using an in-memory demo library (not saved, can't sync)"),
    }
    Session::with_demo().with_fs().with_system_clock()
}

/// The staging folders in the app's tmp folder: copies the pickers made (moved into the library
/// by the import review) and the last export (handed to the share sheet).
fn staging(tmp: &Path) -> (PathBuf, PathBuf) {
    (tmp.join("Import"), tmp.join("Exports"))
}

/// What a previous run left there: picks the review didn't take (cancelled, duplicates) and
/// exports that were shared or dismissed.
fn clean_staging(tmp: &Path) {
    let (import, export) = staging(tmp);
    for d in [import, export] {
        if let Err(e) = std::fs::remove_dir_all(&d)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("{}: {e}", d.display());
        }
    }
}

/// Files a folder pick copies: the formats the library imports.
fn is_photo(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| lightcraft_engine::import::EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn pick_kind(s: PickSource) -> PickKind {
    match s {
        PickSource::Photos => PickKind::Photos,
        PickSource::Files => PickKind::Files,
        PickSource::Folder => PickKind::Folder,
    }
}

/// The UI's haptic feedback as the Taptic Engine plays it.
fn play_haptic(h: Haptic) {
    lightcraft_ios_host::haptic(match h {
        Haptic::Selection => lightcraft_ios_host::Haptic::Selection,
        Haptic::Light => lightcraft_ios_host::Haptic::Light,
        Haptic::Medium => lightcraft_ios_host::Haptic::Medium,
        Haptic::Warning => lightcraft_ios_host::Haptic::Warning,
    });
}

/// ImageIO's decode as `lightcraft-codecs` takes it (HEIC / HEIF and AVIF have no pure-Rust
/// decoder): pixels as stored, premultiplied, in the image's own colour space.
fn system_image(i: lightcraft_ios_host::Image) -> lightcraft_codecs::SystemImage {
    lightcraft_codecs::SystemImage {
        width: i.width,
        height: i.height,
        pixels: match i.pixels {
            lightcraft_ios_host::Pixels::Rgba8(v) => lightcraft_codecs::SystemPixels::Rgba8(v),
            lightcraft_ios_host::Pixels::Rgba16(v) => lightcraft_codecs::SystemPixels::Rgba16(v),
        },
        premultiplied: true,
        bit_depth: i.bit_depth,
        icc: i.icc,
        exif: None,
        orientation: i.orientation,
        source_width: i.source_width,
        source_height: i.source_height,
    }
}

fn imageio_decode(bytes: &[u8], max: Option<(u32, u32)>) -> Result<lightcraft_codecs::SystemImage, String> {
    lightcraft_ios_host::decode_image(bytes, max).map(system_image)
}

/// What the app can do on iOS: the Photos and Files pickers (their copies arrive in `inbox`),
/// exports through the share sheet, sync requests on worker threads (pure-Rust TLS, Mozilla roots).
fn services(ctx: egui::Context, inbox: Inbox, tmp: &Path) -> Services {
    let (import_dir, export_dir) = staging(tmp);
    let repaint = ctx.clone();
    let pick: Rc<dyn Fn(PickSource) -> Result<(), String>> = Rc::new(move |source| {
        let (inbox, ctx) = (inbox.clone(), ctx.clone());
        let deliver = Box::new(move |p: Picked| {
            inbox.lock().unwrap_or_else(PoisonError::into_inner).push(p);
            ctx.request_repaint();
        });
        lightcraft_ios_host::pick(pick_kind(source), &import_dir, is_photo, deliver)
    });
    let photos = pick.clone();
    Services {
        // Import Photos… (and its shortcut on an iPad keyboard): the Photos picker
        pick_files: Some(Box::new(move || {
            if let Err(e) = photos(PickSource::Photos) {
                log::error!("Photos picker: {e}");
            }
            Vec::new() // the picks arrive later, through the inbox
        })),
        host_pick: Some(Box::new(move |s| pick(s))),
        share_exports: Some(ShareExports {
            dir: export_dir.to_string_lossy().to_string(),
            share: Box::new(|files: &[String]| {
                let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
                if let Err(e) = lightcraft_ios_host::share(&paths) {
                    log::error!("share sheet: {e}");
                }
            }),
        }),
        // Save to Photos (the share sheet's row, the photo's top bar): PhotoKit, add-only access
        save_to_photos: Some(Box::new(move |files: &[String], done: SaveDone| {
            let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
            let repaint = repaint.clone();
            lightcraft_ios_host::save_to_photos(
                &paths,
                Box::new(move |result| {
                    done(result);
                    repaint.request_repaint();
                }),
            );
        })),
        write: Some(Box::new(lightcraft_engine::export::write_file)),
        write_shared: Some(Arc::new(lightcraft_engine::export::write_file)),
        png: Some(Box::new(|img: &lightcraft_raster::Rgba8| {
            lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
        })),
        sync_exec: Some(lightcraft_ui_egui::sync_ui::native_exec()),
        tile_exec: Some(lightcraft_ui_egui::panels::map::native_exec()),
        haptic: Some(Box::new(play_haptic)),
        clipboard_text: Some(Box::new(lightcraft_ios_host::pasteboard_text)),
        ..Services::default()
    }
}

/// Development aid: `LIGHTCRAFT_SCRIPT=<file>` runs the control-protocol requests in it, one JSON
/// object per line (`{"method": …, "params": …}`, `docs/control-protocol.md`; `{"sleep": ms}`
/// waits), and appends each reply to `<file>.out`. This is how a Mac drives the app on a device
/// without touching it (`docs/ios.md` → *Driving the app on a device*). The file lives in the app's
/// own sandbox, so nothing is exposed on the network.
fn run_script(path: PathBuf, ctx: egui::Context) -> std::sync::mpsc::Receiver<lightcraft_ui_egui::ControlRequest> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("lc-script".into()).spawn(move || {
        use std::io::Write;
        let out_path = path.with_extension("out");
        let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&out_path).ok();
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            log::error!("script {}: {e}", path.display());
            String::new()
        });
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
            let reply = if let Some(ms) = v.get("sleep").and_then(serde_json::Value::as_u64) {
                std::thread::sleep(std::time::Duration::from_millis(ms.min(600_000)));
                json!({"ok": true})
            } else if let Some(method) = v.get("method").and_then(serde_json::Value::as_str) {
                let mut params = v.get("params").cloned().unwrap_or(json!({}));
                // a relative `path` (a screenshot's) is next to the script: the app's tmp folder
                if let Some(rel) = params.get("path").and_then(serde_json::Value::as_str).filter(|p| Path::new(p).is_relative())
                    && let Some(dir) = path.parent()
                {
                    params["path"] = json!(dir.join(rel).to_string_lossy());
                }
                let (req, reply) = lightcraft_ui_egui::ControlRequest::new(method, params);
                if tx.send(req).is_err() {
                    break;
                }
                ctx.request_repaint();
                reply.recv_timeout(std::time::Duration::from_secs(120)).unwrap_or_else(|_| json!({"ok": false, "error": "timeout"}))
            } else {
                json!({"ok": false, "error": "not a request"})
            };
            if let Some(f) = out.as_mut() {
                // one write per line, so a reader never sees half a reply
                let _ = f.write_all(format!("{}\n", json!({"request": v, "reply": reply})).as_bytes());
            }
        }
        log::info!("script {} done", path.display());
    });
    if let Err(e) = spawned {
        log::error!("script: {e}");
    }
    rx
}

/// A lifecycle notification for the app (between frames, on the main thread).
fn on_lifecycle(host: &Weak<RefCell<Host>>, event: Lifecycle) {
    if let Some(h) = host.upgrade()
        && let Ok(mut h) = h.try_borrow_mut()
    {
        h.lifecycle(event);
        return;
    }
    // (the app is busy or gone: at least keep the GPU out of the background)
    match event {
        Lifecycle::DidEnterBackground => lightcraft_engine::gpu::pause(),
        Lifecycle::WillEnterForeground | Lifecycle::DidBecomeActive => lightcraft_engine::gpu::resume(),
        _ => {}
    }
}

/// Runs the app; returns when the event loop ends (on iOS, normally never).
pub fn run() -> eframe::Result {
    let _ = log::set_logger(&FileLog).map(|()| log::set_max_level(log::LevelFilter::Info));
    lightcraft_engine::guard::install_hook(std::env::temp_dir().join("lightcraft-panics.log"));
    // caches sized to what iOS lets this app use, before anything is cached
    let available = lightcraft_sysmem::available_memory();
    let budget = lightcraft_engine::memory::set_budget(lightcraft_engine::memory::budget_for_limit(available));
    log::info!("memory: {} MB available to the app, budget {} MB", available.map_or(0, |a| a >> 20), budget >> 20);
    lightcraft_engine::memory::set_release_hook(|| {
        let _ = lightcraft_sysmem::release_free_memory();
    });
    lightcraft_codecs::set_system_decoder(&[lightcraft_codecs::Format::Heif, lightcraft_codecs::Format::Avif], imageio_decode);
    let tmp = std::env::temp_dir();
    clean_staging(&tmp);
    let (prefs, prefs_warning, keep_prefs_file) = lightcraft_ui_egui::prefs::load_prefs();
    if let Some(p) = &prefs {
        lightcraft_engine::gpu::set_enabled(p.settings.gpu);
    }
    eframe::run_native(
        "LightCraft",
        eframe::NativeOptions { wgpu_options: wgpu_options(), ..Default::default() },
        Box::new(move |cc| {
            let dir = library_dir(std::env::var_os("LIGHTCRAFT_LIBRARY"), std::env::var_os("HOME"));
            let inbox = Inbox::default();
            let mut app = LightcraftApp::new(open_session(dir.as_deref()), services(cc.egui_ctx.clone(), inbox.clone(), &tmp));
            match prefs {
                Some(ui) => app.ui = ui,
                // a first start: the phone's square grid, as Lightroom's mobile app shows a library
                None => app.ui.view = lightcraft_ui_egui::state::ViewMode::SquareGrid,
            }
            if let Some(script) = std::env::var_os("LIGHTCRAFT_SCRIPT").filter(|s| !s.is_empty()) {
                // a relative path is in the app's tmp folder (where `devicectl … copy to` puts it)
                app = app.with_control(run_script(tmp.join(script), cc.egui_ctx.clone()));
            }
            lightcraft_ui_egui::i18n::set_language(app.ui.language);
            app.notices.extend(prefs_warning);
            let prefs = PrefsWriter::new(&app, keep_prefs_file);
            let host = Rc::new(RefCell::new(Host { app, prefs, inbox, background: None, safe_area: Default::default(), style: None }));
            let weak = Rc::downgrade(&host);
            if let Err(e) = lightcraft_ios_host::observe_lifecycle(Box::new(move |event| on_lifecycle(&weak, event))) {
                log::error!("lifecycle notifications: {e}");
            }
            cc.egui_ctx.add_plugin(TextInput);
            // the layout moves above the keyboard (`above_keyboard`) on the next frame
            let ctx = cc.egui_ctx.clone();
            if let Err(e) = lightcraft_ios_host::observe_keyboard(Box::new(move || ctx.request_repaint())) {
                log::error!("keyboard notifications: {e}");
            }
            Ok(Box::new(App(host)))
        }),
    )
}

/// C entry point called from `main` (`xtool/Sources/LightCraftHost/LightCraftMain.m`).
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn lightcraft_ios_main() {
    let result = run();
    // the simulator shows no stderr for a bundled app; leave the outcome where `simctl get_app_container` finds it
    let msg = match result {
        Ok(()) => "lightcraft: event loop ended".to_string(),
        Err(e) => format!("lightcraft: {e}"),
    };
    log::error!("{msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lightcraft-ios-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn host(lib: &Path) -> Host {
        let app = LightcraftApp::new(open_session(Some(lib)), Services::default());
        // keep_file: these tests never write the developer's own ui.json
        let prefs = PrefsWriter::new(&app, true);
        Host { app, prefs, inbox: Inbox::default(), background: None, safe_area: Default::default(), style: None }
    }

    /// Backgrounding saves the view and pauses the GPU; coming back resumes it.
    #[test]
    fn backgrounding_saves_and_pauses_the_gpu() {
        let root = temp("lifecycle");
        let lib = root.join("Library");
        let mut h = host(&lib);
        let ids: Vec<u64> = h.app.session.catalog.photos().take(2).map(|p| p.id.0).collect();
        h.app.run("library.select", json!({"ids": ids})).unwrap();
        h.lifecycle(Lifecycle::WillResignActive);
        let view = std::fs::read_to_string(lib.join("view.json")).unwrap();
        assert!(view.contains(&ids[1].to_string()), "{view}");
        h.lifecycle(Lifecycle::DidEnterBackground);
        assert!(lightcraft_engine::gpu::paused());
        assert!(h.background.is_none(), "no export running: no background time asked for");
        h.lifecycle(Lifecycle::MemoryWarning);
        h.lifecycle(Lifecycle::WillEnterForeground);
        assert!(!lightcraft_engine::gpu::paused());
        drop(h);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Picked copies go to the import review (moved into the library from there); failures are said.
    #[test]
    fn picks_open_the_import_review() {
        let root = temp("picks");
        let mut h = host(&root.join("Library"));
        let ctx = egui::Context::default();
        h.joined(&ctx, Picked { cancelled: true, ..Default::default() });
        assert!(h.app.scan.is_none(), "a cancelled pick does nothing");
        let staged = root.join("tmp").join("Import");
        std::fs::create_dir_all(&staged).unwrap();
        let img = lightcraft_raster::Rgba8 { width: 4, height: 4, data: vec![[9, 99, 199, 255]; 16] };
        let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(staged.join("IMG_0001.png"), png).unwrap();
        h.joined(&ctx, Picked { files: vec![staged.join("IMG_0001.png")], failed: vec!["IMG_0002: gone".into()], cancelled: false });
        assert!(h.app.scan.is_some(), "the review scans the picked copies");
        assert!(h.app.ui.toast.as_ref().is_some_and(|(t, _)| t.contains("couldn't be")), "{:?}", h.app.ui.toast);
        drop(h);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pick_messages_and_photo_files() {
        assert_eq!(picked_message(&Picked::default()), None);
        let p = Picked { files: vec![], failed: vec!["a".into()], cancelled: false };
        assert_eq!(picked_message(&p).as_deref(), Some("1 item couldn't be copied from the picker"));
        let p = Picked { files: vec!["x".into(), "y".into()], failed: vec!["a".into()], cancelled: false };
        assert_eq!(picked_message(&p).as_deref(), Some("2 photos copied; 1 couldn't be"));
        assert!(
            is_photo(Path::new("/a/IMG_1.HEIC")) && is_photo(Path::new("b.dng")) && !is_photo(Path::new("IMG_1.AAE")) && !is_photo(Path::new("x"))
        );
    }

    #[test]
    fn imageio_results_become_codec_images() {
        let i = lightcraft_ios_host::Image {
            width: 2,
            height: 1,
            pixels: lightcraft_ios_host::Pixels::Rgba16(vec![65535, 0, 0, 65535, 0, 65535, 0, 65535]),
            bit_depth: 10,
            icc: None,
            orientation: 8,
            source_width: 4000,
            source_height: 2000,
        };
        let s = system_image(i);
        assert!(s.premultiplied && s.bit_depth == 10 && s.orientation == 8 && s.source_width == 4000);
        assert!(matches!(s.pixels, lightcraft_codecs::SystemPixels::Rgba16(ref v) if v.len() == 8));
    }

    /// The keyboard's Return confirms a single-line field: it loses focus with Enter pressed (A1.9).
    #[test]
    fn the_keyboards_return_confirms_a_text_field() {
        let ctx = egui::Context::default();
        let mut text = String::from("trip");
        let mut confirmed = false;
        for (frame, returns) in [(0, 0), (1, 0), (2, 1)] {
            let mut raw = egui::RawInput::default();
            press_enter(&mut raw, returns);
            let mut out = ctx.run_ui(raw, |ui| {
                let r = ui.text_edit_singleline(&mut text);
                if frame == 0 {
                    r.request_focus();
                }
                confirmed |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            });
            out.textures_delta.clear(); // (no renderer here)
        }
        assert!(confirmed);
    }

    /// Tapping a text field that already has the keyboard must not ask winit to hide and show it
    /// again (egui's "interrupt composition" on every tap; the plugin drops it).
    #[test]
    fn tapping_a_focused_text_field_keeps_the_keyboard() {
        let interrupts = |plugin: bool| {
            let ctx = egui::Context::default();
            if plugin {
                ctx.add_plugin(TextInput);
            }
            let mut text = String::from("trip");
            let at = egui::pos2(20.0, 10.0);
            let tap =
                |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            let mut seen = vec![];
            for events in [vec![], vec![egui::Event::PointerMoved(at), tap(true)], vec![tap(false)], vec![tap(true)], vec![tap(false)]] {
                let raw = egui::RawInput { events, ..Default::default() };
                let mut out = ctx.run_ui(raw, |ui| {
                    ui.text_edit_singleline(&mut text);
                });
                out.textures_delta.clear(); // (no renderer here)
                seen.push(out.platform_output.ime.map(|i| i.should_interrupt_composition));
            }
            seen
        };
        assert!(interrupts(false).contains(&Some(true)), "egui asks for it: {:?}", interrupts(false));
        let fixed = interrupts(true);
        assert!(fixed.last().is_some_and(Option::is_some), "the field keeps the keyboard: {fixed:?}");
        assert!(!fixed.contains(&Some(true)), "{fixed:?}");
    }

    /// The on-screen keyboard pushes the bottom of the content up; hidden, the home indicator's inset stays.
    #[test]
    fn content_stays_above_the_keyboard() {
        let safe = egui::SafeAreaInsets(egui::epaint::MarginF32 { left: 0.0, right: 0.0, top: 62.0, bottom: 34.0 });
        assert_eq!(above_keyboard(safe, 0.0).0.bottom, 34.0);
        assert_eq!(above_keyboard(safe, 336.0).0.bottom, 336.0);
        assert_eq!(above_keyboard(safe, 336.0).0.top, 62.0);
    }

    #[test]
    fn a_script_drives_the_app_and_keeps_the_replies() {
        let dir = temp("script");
        let script = dir.join("tour.jsonl");
        std::fs::write(&script, "{\"sleep\": 1}\n\nnot json\n{\"method\": \"ui.screenshot\", \"params\": {\"path\": \"shot.png\"}}\n").unwrap();
        let rx = run_script(script, egui::Context::default());
        let req = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(req.method, "ui.screenshot");
        assert_eq!(req.params["path"], json!(dir.join("shot.png").to_string_lossy()), "relative to the script");
        req.reply.send(json!({"ok": true, "result": 7})).unwrap();
        let mut out = String::new();
        for _ in 0..200 {
            out = std::fs::read_to_string(dir.join("tour.out")).unwrap_or_default();
            if out.ends_with('\n') && out.lines().count() == 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out}");
        assert!(lines[1].contains("not a request") && lines[2].contains("\"result\":7"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn staging_is_cleaned_at_launch() {
        let tmp = temp("staging");
        let (import, export) = staging(&tmp);
        std::fs::create_dir_all(import.join("pick-1")).unwrap();
        std::fs::create_dir_all(&export).unwrap();
        std::fs::write(export.join("old.jpg"), b"x").unwrap();
        clean_staging(&tmp);
        assert!(!import.exists() && !export.exists());
        clean_staging(&tmp); // nothing there: fine
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_library_is_kept_in_documents() {
        let dir = library_dir(None, Some("/var/mobile/Containers/Data/Application/X".into()));
        assert_eq!(dir, Some(PathBuf::from("/var/mobile/Containers/Data/Application/X/Documents/LightCraft Library")));
        assert_eq!(library_dir(Some("/tmp/lib".into()), Some("/home".into())), Some(PathBuf::from("/tmp/lib")));
        assert_eq!(library_dir(Some("".into()), None), None);
    }

    #[test]
    fn a_saved_library_can_sign_in_and_the_fallback_says_why_not() {
        let root = std::env::temp_dir().join(format!("lightcraft-ios-test-{}", std::process::id()));
        let home = root.join("home");
        let dir = library_dir(None, Some(home.into_os_string())).unwrap();
        let mut s = open_session(Some(&dir));
        assert_ne!(s.catalog.len(), 0, "a new library starts with the demo photos");
        assert!(dir.is_dir());
        s.sync_sign_in("photos.example.com", "ann", "a password", "iPhone").unwrap();
        drop(s);
        // reopened: the same library, still signed in to the same server
        let s = open_session(Some(&dir));
        assert_eq!(s.sync_state().map(|st| st.status()["server"].clone()), Some("https://photos.example.com".into()));
        drop(s);
        // a library that can't be opened (a file in the way) falls back to the demo, which can't sync
        let blocked = root.join("blocked");
        std::fs::write(&blocked, b"not a folder").unwrap();
        let mut s = open_session(Some(&blocked));
        assert_ne!(s.catalog.len(), 0);
        assert!(s.sync_sign_in("https://photos.example.com", "ann", "a password", "iPhone").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
