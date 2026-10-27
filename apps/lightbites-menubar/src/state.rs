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

/// A filled dot with a soft edge in the state's colour, as RGBA pixels. Drawn here, not taken from anywhere.
pub fn icon_rgba(state: State) -> Vec<u8> {
    let [r, g, b] = state.colour();
    let c = ICON_SIZE as f32 / 2.0;
    let radius = c - 3.0;
    let mut px = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let a = (radius + 0.5 - d).clamp(0.0, 1.0);
            px.extend_from_slice(&[r, g, b, (a * 255.0).round() as u8]);
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
    fn the_icon_is_a_dot_that_changes_colour() {
        let (on, off) = (icon_rgba(State::Running), icon_rgba(State::Stopped));
        assert_eq!(on.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        let mid = ((ICON_SIZE / 2 * ICON_SIZE + ICON_SIZE / 2) * 4) as usize;
        assert_eq!(on.get(mid + 3), Some(&255), "opaque in the middle");
        assert_eq!(on.get(3), Some(&0), "clear in the corner");
        assert_ne!(on.get(mid..mid + 3), off.get(mid..mid + 3));
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
