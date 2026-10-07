//! LightCraft on iOS, phase 0 spike (`docs/ios.md`): the desktop egui UI, started from the C `main`
//! of the Xcode project in `xcode/`. The library is saved in the app's Documents folder (new ones
//! start with the demo photos) and can sync with a self-hosted server (Settings ▸ Sync,
//! `docs/sync.md`). No pickers and no touch layout yet.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use lightcraft_engine::Session;
use lightcraft_ui_egui::{LightcraftApp, Services};

struct App(LightcraftApp);

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.logic(ctx);
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // the root ui spans the whole screen; keep the panels out of the status bar, Dynamic Island and
        // home indicator (egui-winit reads the insets from UIKit into `content_rect`)
        let safe = ui.ctx().content_rect();
        ui.scope_builder(egui::UiBuilder::new().max_rect(safe), |ui| self.0.ui(ui));
    }
}

/// Appends every log record to `lightcraft-ios.log` in the app's tmp dir: a bundled simulator app's
/// stderr isn't shown anywhere, and winit exits the process on iOS when eframe fails to start.
struct FileLog;

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("lightcraft-ios.log")) {
            let _ = writeln!(f, "{} {}: {}", r.level(), r.target(), r.args());
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

/// What the app can do on iOS: sync requests on worker threads (pure-Rust TLS, Mozilla roots).
fn services() -> Services {
    Services { sync_exec: Some(lightcraft_ui_egui::sync_ui::native_exec()), ..Services::default() }
}

/// Runs the app; returns when the event loop ends (on iOS, normally never).
pub fn run() -> eframe::Result {
    let _ = log::set_logger(&FileLog).map(|()| log::set_max_level(log::LevelFilter::Info));
    lightcraft_engine::guard::install_hook(std::env::temp_dir().join("lightcraft-panics.log"));
    eframe::run_native(
        "LightCraft",
        eframe::NativeOptions { wgpu_options: wgpu_options(), ..Default::default() },
        Box::new(|_cc| {
            let dir = library_dir(std::env::var_os("LIGHTCRAFT_LIBRARY"), std::env::var_os("HOME"));
            Ok(Box::new(App(LightcraftApp::new(open_session(dir.as_deref()), services()))))
        }),
    )
}

/// C entry point called from `main.m`.
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
