//! What the menu-bar app shows, without any window system: parsing `lightbites-server status --porcelain`, the menu
//! labels, the icon's pixels and where the server program and its log are. Everything here runs (and is tested) on any system.

use std::path::{Path, PathBuf};

/// The installed server's state, as `lightbites-server status --porcelain` says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Stopped,
    NotInstalled,
    /// The status command could not be run or said something else.
    Unknown,
}

impl State {
    pub fn parse(output: &str) -> State {
        match output.split_whitespace().next() {
            Some("running") => State::Running,
            Some("stopped") => State::Stopped,
            Some("not-installed") => State::NotInstalled,
            _ => State::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            State::Running => "LightBites Server: running",
            State::Stopped => "LightBites Server: stopped",
            State::NotInstalled => "LightBites Server: not installed",
            State::Unknown => "LightBites Server: status unknown",
        }
    }

    pub fn can_start(self) -> bool {
        matches!(self, State::Stopped)
    }

    pub fn can_stop(self) -> bool {
        matches!(self, State::Running)
    }

    /// Dot colour (RGB): green running, grey stopped, orange for the rest.
    fn colour(self) -> [u8; 3] {
        match self {
            State::Running => [52, 199, 89],
            State::Stopped => [142, 142, 147],
            State::NotInstalled | State::Unknown => [255, 159, 10],
        }
    }
}

/// Side of the menu-bar icon in pixels (drawn at 2x for Retina; macOS scales it down).
pub const ICON_SIZE: u32 = 36;

/// The LightBites app icon at 36 x 36, raw RGB (scaled from `apps/lightcraft-server/admin/lightbites-icon.png`).
const APP_ICON: &[u8] = include_bytes!("../assets/icon-36.rgb");

/// The LightBites icon as a 256 x 256 PNG (a rounded tile with transparent margin) for the Dock.
pub const DOCK_ICON_PNG: &[u8] = include_bytes!("../assets/dock-256.png");

/// The LightBites icon as a rounded tile, with a status dot (the state's colour, ringed in white) at its bottom right,
/// as RGBA pixels.
pub fn icon_rgba(state: State) -> Vec<u8> {
    let [r, g, b] = state.colour();
    let size = ICON_SIZE as f32;
    let corner = 8.0_f32;
    let dot = (size - 8.5, size - 8.5); // centre of the dot
    let mut px = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // distance outside the rounded square (negative inside)
            let (qx, qy) = ((fx - size / 2.0).abs() - (size / 2.0 - corner), (fy - size / 2.0).abs() - (size / 2.0 - corner));
            let out = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - corner;
            let mut rgba = match APP_ICON.get((y * ICON_SIZE + x) as usize * 3..(y * ICON_SIZE + x) as usize * 3 + 3) {
                Some(&[r, g, b]) => [r as f32, g as f32, b as f32, (0.5 - out).clamp(0.0, 1.0)],
                _ => [0.0; 4],
            };
            let d = ((fx - dot.0).powi(2) + (fy - dot.1).powi(2)).sqrt();
            for (radius, c) in [(7.0, [255.0, 255.0, 255.0]), (5.0, [r as f32, g as f32, b as f32])] {
                let a = (radius + 0.5 - d).clamp(0.0, 1.0);
                for i in 0..3 {
                    rgba[i] = rgba[i] * (1.0 - a) + c[i] * a;
                }
                rgba[3] = rgba[3] * (1.0 - a) + a;
            }
            px.extend(rgba.map(|v| v.round().clamp(0.0, 255.0) as u8).iter().take(3));
            px.push((rgba[3] * 255.0).round().clamp(0.0, 255.0) as u8);
        }
    }
    px
}

/// Where `install` puts the server program (`lightbites-server install`).
pub fn installed_program(home: &Path) -> PathBuf {
    home.join("Library/Application Support/LightCraft Server/program/lightbites-server")
}

pub fn log_path(home: &Path) -> PathBuf {
    home.join("Library/Logs/LightCraft Server/server.log")
}

/// The server program to run: the installed copy, else one beside this app, else whatever `PATH` finds.
pub fn server_program(home: Option<&Path>, beside: Option<&Path>) -> PathBuf {
    let candidates = [home.map(installed_program), beside.map(|d| d.join("lightbites-server"))];
    candidates.into_iter().flatten().find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("lightbites-server"))
}

/// The admin page's address for a listen port.
pub fn admin_url(port: u16) -> String {
    format!("http://localhost:{port}/admin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_word_is_read_and_anything_else_is_unknown() {
        assert_eq!(State::parse("running\n"), State::Running);
        assert_eq!(State::parse("stopped"), State::Stopped);
        assert_eq!(State::parse("not-installed\n"), State::NotInstalled);
        assert_eq!(State::parse(""), State::Unknown);
        assert_eq!(State::parse("error: nope"), State::Unknown);
    }

    #[test]
    fn only_the_state_that_can_use_a_button_enables_it() {
        assert!(State::Stopped.can_start() && !State::Stopped.can_stop());
        assert!(State::Running.can_stop() && !State::Running.can_start());
        for s in [State::NotInstalled, State::Unknown] {
            assert!(!s.can_start() && !s.can_stop());
        }
    }

    #[test]
    fn the_icon_is_the_app_icon_with_a_dot_that_changes_colour() {
        assert_eq!(DOCK_ICON_PNG.get(..4), Some(&[0x89, b'P', b'N', b'G'][..]), "a PNG");
        assert_eq!(APP_ICON.len(), (ICON_SIZE * ICON_SIZE * 3) as usize);
        let (on, off) = (icon_rgba(State::Running), icon_rgba(State::Stopped));
        assert_eq!(on.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        let at = |x: u32, y: u32| ((y * ICON_SIZE + x) * 4) as usize;
        assert_eq!(on.get(at(0, 0) + 3), Some(&0), "the tile's corner is clear");
        assert_eq!(on.get(at(8, 8) + 3), Some(&255), "opaque inside the tile");
        assert_eq!(on.get(at(8, 8)..at(8, 8) + 4), off.get(at(8, 8)..at(8, 8) + 4), "the tile does not change");
        let dot = at(ICON_SIZE - 9, ICON_SIZE - 9);
        assert_eq!(on.get(dot..dot + 3), Some(&[52, 199, 89][..]), "a green dot when running");
        assert_ne!(on.get(dot..dot + 3), off.get(dot..dot + 3));
    }

    #[test]
    fn the_server_program_falls_back_to_the_path() {
        let dir = std::env::temp_dir().join(format!("lb-menubar-{}", std::process::id()));
        assert_eq!(server_program(Some(&dir), Some(&dir)), PathBuf::from("lightbites-server"));
        let beside = dir.join("beside");
        std::fs::create_dir_all(&beside).unwrap();
        std::fs::write(beside.join("lightbites-server"), b"").unwrap();
        assert_eq!(server_program(Some(&dir), Some(&beside)), beside.join("lightbites-server"));
        let installed = installed_program(&dir);
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::write(&installed, b"").unwrap();
        assert_eq!(server_program(Some(&dir), Some(&beside)), installed);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_admin_address_names_the_port() {
        assert_eq!(admin_url(9000), "http://localhost:9000/admin");
    }
}
