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

/// Runs the app; returns when the event loop ends (on iOS, normally never).
pub fn run() -> eframe::Result {
    let _ = log::set_logger(&FileLog).map(|()| log::set_max_level(log::LevelFilter::Info));
    lightcraft_engine::guard::install_hook(std::env::temp_dir().join("lightcraft-panics.log"));
    eframe::run_native(
        "LightCraft",
        eframe::NativeOptions { wgpu_options: wgpu_options(), ..Default::default() },
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
    let result = run();
    // the simulator shows no stderr for a bundled app; leave the outcome where `simctl get_app_container` finds it
    let msg = match result {
        Ok(()) => "lightcraft: event loop ended".to_string(),
        Err(e) => format!("lightcraft: {e}"),
    };
    log::error!("{msg}");
}
