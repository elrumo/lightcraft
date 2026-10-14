//! HDR display output (LR-VIEW-HDR-DISPLAY): the loupe of an HDR edit shows the range above SDR
//! white when the window's screen can.
//!
//! What it takes, and what each part does:
//! - **A surface that can carry it.** eframe picks an 8-bit window surface; `third_party/egui-wgpu`
//!   (patched) makes it a 16-bit float one when [`surface_wanted`] says some screen has headroom
//!   (`SurfaceConfig::hdr_surface`). A float surface presents as plain sRGB until `set_hdr_active`.
//! - **HDR output only while it is used** ([`HdrOutput::frame`]): the surface goes to extended
//!   Display P3 (macOS: Extended Dynamic Range, which raises the screen's backlight headroom) while
//!   the loupe draws an HDR picture, and back to plain sRGB when it doesn't.
//! - **The picture.** The scheduler renders an HDR edit's loupe as a float HDR render when the
//!   window's screen has headroom ([`HdrOutput::frame`] tells it, and again when the window moves to
//!   another screen); [`WgpuSink`](crate::hdr_sink::WgpuSink) makes its texture.
//!
//! macOS only so far: Windows (scRGB / HDR10 swap chains) and Wayland (the colour-management
//! protocol) are not driven, and report SDR with the reason. Anything missing falls back to the SDR
//! rendition, so an HDR edit is never shown wrong.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::wgpu::TextureFormat;
use lightcraft_ui_egui::LightcraftApp;

/// Windowing platform, as far as HDR output is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Wayland,
    X11,
    Web,
    Other,
}

impl Platform {
    /// The platform this build runs on (`wayland` from the session environment on Linux / BSD).
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_arch = "wasm32") {
            Self::Web
        } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            Self::Wayland
        } else if std::env::var_os("DISPLAY").is_some() {
            Self::X11
        } else {
            Self::Other
        }
    }

    /// Whether LightCraft drives an HDR compositing path there: macOS (a float Metal layer in extended
    /// Display P3). Windows and Wayland have one wgpu can reach, not wired yet.
    fn has_hdr_path(self) -> bool {
        matches!(self, Self::MacOs)
    }
}

/// The display output decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayOutput {
    Sdr {
        reason: &'static str,
    },
    /// HDR through a float surface of `format`.
    Hdr {
        format: TextureFormat,
    },
}

impl DisplayOutput {
    pub fn is_hdr(self) -> bool {
        matches!(self, Self::Hdr { .. })
    }
}

/// Is HDR display switched off (`LIGHTCRAFT_HDR_DISPLAY=0`)?
pub fn disabled() -> bool {
    std::env::var("LIGHTCRAFT_HDR_DISPLAY").ok().is_some_and(|v| matches!(v.trim(), "0" | "off" | "false" | "no"))
}

/// The decision for the window: `target` is the format the renderer configured (`None`: the glow
/// renderer or no wgpu state), `headroom` the window's screen's peak over SDR white as the OS reports
/// it (`None`: unknown, `Some(1.0)`: an SDR display), `disabled` the user's opt-out.
pub fn choose(target: Option<TextureFormat>, platform: Platform, headroom: Option<f32>, disabled: bool) -> DisplayOutput {
    if disabled {
        return DisplayOutput::Sdr { reason: "HDR display is switched off" };
    }
    if !platform.has_hdr_path() {
        return DisplayOutput::Sdr { reason: "HDR display is not wired for this windowing system" };
    }
    let Some(format) = target else {
        return DisplayOutput::Sdr { reason: "the window renderer is not wgpu" };
    };
    if format != TextureFormat::Rgba16Float {
        return DisplayOutput::Sdr { reason: "the window surface is 8-bit" };
    }
    match headroom {
        Some(h) if h.is_finite() && h > 1.0 => DisplayOutput::Hdr { format },
        Some(_) => DisplayOutput::Sdr { reason: "the display is in SDR mode" },
        None => DisplayOutput::Sdr { reason: "the display's HDR headroom is unknown" },
    }
}

/// Should the window ask for a surface that can show HDR: not switched off, a platform with an HDR
/// path, and some screen with headroom (a window can be dragged to it later). Read before the window
/// opens.
pub fn surface_wanted() -> bool {
    !disabled() && Platform::current().has_hdr_path() && screens::best_headroom().is_some_and(|h| h > 1.0)
}

/// What the desktop app keeps for HDR output: when the window's screen was last asked, what it said,
/// and until when the surface stays in HDR output after the last HDR picture.
pub struct HdrOutput {
    next_check: Instant,
    screen: Debounce,
    held_until: Option<Instant>,
}

/// How often the window's screen is asked what headroom it has (it changes when the window moves).
const CHECK_EVERY: Duration = Duration::from_millis(500);

/// How long the surface stays in HDR output after the loupe stopped drawing an HDR picture. A preset's
/// hover preview, the Before view or the next photo are gaps of a moment: each flip of the surface is a
/// backlight ramp and a reconfigure, so it waits to see whether another HDR picture follows.
const HOLD: Duration = Duration::from_secs(1);

impl Default for HdrOutput {
    fn default() -> Self {
        HdrOutput { next_check: Instant::now(), screen: Debounce::default(), held_until: None }
    }
}

/// Steadies what the window's screen says about its headroom. AppKit's answer can flicker while the
/// screen ramps into or out of EDR (the window's screen is momentarily none, or another one): HDR
/// pictures go on at once, and off only after several readings in a row without headroom; a reading
/// that isn't known changes nothing.
#[derive(Default)]
struct Debounce {
    off_streak: u32,
}

impl Debounce {
    /// Readings without headroom, in a row, that turn HDR pictures off (two seconds).
    const OFF_AFTER: u32 = 4;

    /// Whether the screen has headroom, given the answer so far (`current`) and a new reading.
    fn update(&mut self, current: bool, reading: Option<f32>) -> bool {
        match reading {
            None => current,
            Some(h) if h.is_finite() && h > 1.0 => {
                self.off_streak = 0;
                true
            }
            Some(_) => {
                self.off_streak = self.off_streak.saturating_add(1);
                current && self.off_streak < Self::OFF_AFTER
            }
        }
    }
}

impl HdrOutput {
    /// Give the app a way to draw HDR when its surface can (and say what was decided).
    pub fn install(app: &mut LightcraftApp, render_state: Option<&eframe::egui_wgpu::RenderState>) -> Self {
        let target = render_state.map(|r| r.target_format);
        let out = choose(target, Platform::current(), screens::window_headroom(), disabled());
        match out {
            DisplayOutput::Sdr { reason } => log::info!("display: SDR ({reason})"),
            DisplayOutput::Hdr { format } => log::info!("display: HDR ({format:?}, extended Display P3)"),
        }
        // a float surface can draw HDR pictures even where the window's screen has no headroom yet
        // (another screen has): they are only asked for where there is
        if let (Some(rs), Some(TextureFormat::Rgba16Float)) = (render_state, target)
            && Platform::current().has_hdr_path()
            && !disabled()
        {
            app.renderer.set_hdr_sink(Some(Arc::new(crate::hdr_sink::WgpuSink::new(rs.clone()))));
            app.renderer.set_hdr_display(out.is_hdr());
        }
        HdrOutput::default()
    }

    /// Once a frame: ask the scheduler for HDR pictures on a screen with headroom, and put the surface
    /// in HDR output while one is on screen (and for [`HOLD`] after).
    pub fn frame(&mut self, app: &mut LightcraftApp, ctx: &egui::Context) {
        if !app.renderer.can_draw_hdr() {
            return;
        }
        let now = Instant::now();
        if now >= self.next_check {
            self.next_check = now + CHECK_EVERY;
            let reading = screens::window_headroom();
            let was = app.renderer.hdr_display();
            let on = self.screen.update(was, reading);
            if on != was {
                log::info!(
                    "display: HDR pictures {} (headroom {reading:?})",
                    if on { "on: the screen has headroom" } else { "off: the screen has none" }
                );
                app.renderer.set_hdr_display(on);
            }
            // (asked again while waiting for the screen's answer to settle)
            if reading.is_some_and(|h| h <= 1.0) && on || reading.is_some_and(|h| h > 1.0) && !on {
                ctx.request_repaint_after(CHECK_EVERY);
            }
        }
        if app.hdr_on_screen {
            self.held_until = now.checked_add(HOLD);
        }
        let held = self.held_until.filter(|t| now < *t);
        if let Some(t) = held.filter(|_| !app.hdr_on_screen) {
            ctx.request_repaint_after(t.saturating_duration_since(now));
        }
        // (a no-op for the painter unless the surface is a float one: `hdr_surface`)
        eframe::egui_wgpu::set_hdr_active(app.hdr_on_screen || held.is_some());
    }
}

/// Screens and their headroom (the peak over SDR white they can reach: macOS' EDR values).
#[cfg(target_os = "macos")]
mod screens {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSScreen};

    /// What `screen` can reach, once HDR content asks for it (`1.0`: nothing above SDR white). This
    /// is the potential, not the current value: macOS only raises the current one while something
    /// is drawing HDR.
    fn headroom(screen: &NSScreen) -> Option<f32> {
        if !objc2::available!(macos = 10.15) {
            return None;
        }
        let h = screen.maximumPotentialExtendedDynamicRangeColorComponentValue();
        h.is_finite().then_some(h as f32)
    }

    /// The headroom of the screen the window is mostly on (the main screen before there is a window);
    /// `None` when it isn't known: off the main thread, or the window has no screen at this moment (it
    /// does for a moment while the screen ramps its EDR: not the main screen's answer then).
    pub fn window_headroom() -> Option<f32> {
        let mtm = MainThreadMarker::new()?;
        let app = NSApplication::sharedApplication(mtm);
        match app.mainWindow().or_else(|| app.keyWindow()).or_else(|| app.windows().firstObject()) {
            Some(window) => window.screen().and_then(|s| headroom(&s)),
            None => NSScreen::mainScreen(mtm).and_then(|s| headroom(&s)),
        }
    }

    /// The most headroom any connected screen has.
    pub fn best_headroom() -> Option<f32> {
        let mtm = MainThreadMarker::new()?;
        NSScreen::screens(mtm).iter().filter_map(|s| headroom(&s)).max_by(f32::total_cmp)
    }
}

#[cfg(not(target_os = "macos"))]
mod screens {
    pub fn window_headroom() -> Option<f32> {
        None
    }
    pub fn best_headroom() -> Option<f32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TextureFormat as F;

    #[test]
    fn eight_bit_surfaces_are_sdr() {
        for f in [F::Bgra8Unorm, F::Bgra8UnormSrgb, F::Rgba8Unorm, F::Rgb10a2Unorm] {
            assert!(!choose(Some(f), Platform::MacOs, Some(4.0), false).is_hdr(), "{f:?}");
        }
    }

    #[test]
    fn hdr_needs_platform_format_and_headroom() {
        let hdr = DisplayOutput::Hdr { format: F::Rgba16Float };
        assert_eq!(choose(Some(F::Rgba16Float), Platform::MacOs, Some(2.0), false), hdr);
        // any missing condition falls back to SDR
        for p in [Platform::Windows, Platform::Wayland, Platform::X11, Platform::Web, Platform::Other] {
            assert!(!choose(Some(F::Rgba16Float), p, Some(4.0), false).is_hdr(), "{p:?}");
        }
        assert!(!choose(Some(F::Rgba16Float), Platform::MacOs, Some(1.0), false).is_hdr());
        assert!(!choose(Some(F::Rgba16Float), Platform::MacOs, None, false).is_hdr());
        assert!(!choose(Some(F::Rgba16Float), Platform::MacOs, Some(f32::NAN), false).is_hdr());
        assert!(!choose(Some(F::Rgba16Float), Platform::MacOs, Some(2.0), true).is_hdr());
        assert!(!choose(None, Platform::MacOs, Some(4.0), false).is_hdr());
    }

    #[test]
    fn the_screens_answer_is_steadied() {
        let mut d = Debounce::default();
        // headroom turns HDR pictures on at once; unknown readings change nothing
        assert!(d.update(false, Some(16.0)));
        assert!(d.update(true, None));
        assert!(!d.update(false, None));
        // a flicker without headroom is not enough to turn them off…
        for _ in 0..Debounce::OFF_AFTER - 1 {
            assert!(d.update(true, Some(1.0)));
        }
        assert!(d.update(true, Some(2.0)), "…and headroom again starts the count over");
        for _ in 0..Debounce::OFF_AFTER - 1 {
            assert!(d.update(true, Some(1.0)));
        }
        // …several in a row are
        assert!(!d.update(true, Some(1.0)));
        assert!(!d.update(false, Some(f32::NAN)), "a hostile reading is no headroom");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn screens_are_unknown_off_the_main_thread() {
        // AppKit is main-thread only; a test thread gets nothing rather than undefined behaviour
        assert_eq!(std::thread::spawn(screens::window_headroom).join().unwrap(), None);
        assert_eq!(std::thread::spawn(screens::best_headroom).join().unwrap(), None);
    }
}
