//! LightCraft on iOS, phase 0 spike (`docs/ios.md`): the desktop egui UI on a throwaway in-memory
//! demo library, started from the C `main` of the Xcode project in `xcode/`. No pickers, no
//! persistence, no touch layout yet; the point is to see it run on the simulator and a device.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

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
        self.0.ui(ui);
    }
}

/// Runs the app; returns when the event loop ends (on iOS, normally never).
pub fn run() -> eframe::Result {
    lightcraft_engine::guard::install_hook(std::env::temp_dir().join("lightcraft-panics.log"));
    eframe::run_native(
        "LightCraft",
        eframe::NativeOptions::default(),
        Box::new(|_cc| {
            let session = Session::with_demo().with_fs().with_system_clock();
            Ok(Box::new(App(LightcraftApp::new(session, Services::default()))))
        }),
    )
}

/// C entry point called from `main.m`.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn lightcraft_ios_main() {
    if let Err(e) = run() {
        eprintln!("lightcraft: {e}");
    }
}
