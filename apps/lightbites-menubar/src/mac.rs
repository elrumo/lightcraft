//! The macOS menu-bar icon: a `tao` event loop (no window) with a `tray-icon` menu.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage, NSImageView};
use objc2_foundation::NSData;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::state::{self, State};

/// How often the icon looks at the server again.
const POLL: Duration = Duration::from_secs(3);

enum Msg {
    State(State),
    Menu(tray_icon::menu::MenuId),
}

struct Server {
    program: PathBuf,
}

impl Server {
    fn state(&self) -> State {
        Command::new(&self.program)
            .args(["status", "--porcelain"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|o| State::parse(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or(State::Unknown)
    }

    fn run(&self, arg: &str) {
        let _ = Command::new(&self.program).arg(arg).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

fn open(target: &str) {
    let _ = Command::new("open").arg(target).stdin(Stdio::null()).status();
}

/// A dialog from the app (no window of its own). The text goes in through the environment, never into the script.
fn alert(title: &str, text: &str) {
    let script = format!(
        "display dialog (system attribute \"LB_TEXT\") with title \"{}\" buttons {{\"OK\"}} default button 1 with icon note",
        title.replace(['"', '\\'], "")
    );
    let _ = Command::new("osascript").args(["-e", &script]).env("LB_TEXT", text).stdin(Stdio::null()).output();
}

fn version(program: &Path) -> Option<String> {
    let out = Command::new(program).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// LightBites Server.app: installs the server it carries (`Contents/MacOS/lightbites-server`, the web app in
/// `Contents/Resources/web`) the way the one-line installer does, when none is installed or it is another version;
/// that copy then runs at every login with its own menu-bar icon. Otherwise it starts the installed server. Either way
/// it opens the admin page and quits.
fn run_app(bin: &Path, resources: &Path, home: Option<&Path>, port: u16) -> Result<(), String> {
    let bundled = bin.join("lightbites-server");
    let Some(bundled_version) = version(&bundled) else {
        alert("LightBites Server", "The server program in this app does not start. Download the app again from lightbites.app/server.");
        return Err(format!("{} does not run", bundled.display()));
    };
    let installed = home.map(state::installed_program).filter(|p| p.is_file());
    if state::needs_install(installed.as_deref().and_then(version).as_deref(), &bundled_version) {
        let out =
            Command::new(&bundled).arg("install").arg("--web").arg(resources.join("web")).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
            alert("LightBites Server could not be installed", &why);
            return Err(why);
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let code = state::setup_code_line(&stdout).map(|l| format!("\n\n{l}")).unwrap_or_default();
        alert(
            "LightBites Server is running",
            &format!("It starts at every login, and its icon in the menu bar starts and stops it.{code}\n\nThe admin page opens next."),
        );
    } else if let Some(installed) = installed {
        let _ = Command::new(installed).arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    open(&state::admin_url(port));
    Ok(())
}

/// The LightBites icon for the Dock (the app is a bare executable, so macOS would show a generic "exec" tile).
fn set_dock_icon() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let data = NSData::with_bytes(state::DOCK_ICON_PNG);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        // the Dock tile shows a view; `setApplicationIconImage` is an unsafe call in objc2 and this crate has none
        let view = NSImageView::imageViewWithImage(&image, mtm);
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        tile.setContentView(Some(&view));
        tile.display();
    }
}

fn icon(state: State) -> Result<Icon, String> {
    Icon::from_rgba(state::icon_rgba(state), state::ICON_SIZE, state::ICON_SIZE).map_err(|e| e.to_string())
}

pub fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let port = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()).unwrap_or(8080);
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty()).map(PathBuf::from);
    let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from));
    // opened as LightBites Server.app (the DMG): install or update the server it carries, then hand over to that copy
    if let Some((bin, resources)) = beside.as_deref().and_then(|b| state::app_resources(b).map(|r| (b, r))) {
        return run_app(bin, &resources, home.as_deref(), port);
    }
    let server = std::sync::Arc::new(Server { program: state::server_program(home.as_deref(), beside.as_deref()) });
    let log = home.as_deref().map(state::log_path);

    let status = MenuItem::new(State::Unknown.label(), false, None);
    let start = MenuItem::new("Start Server", false, None);
    let stop = MenuItem::new("Stop Server", false, None);
    let admin = MenuItem::new("Open Admin Page", true, None);
    let show_log = MenuItem::new("Show Log", log.is_some(), None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &start,
        &stop,
        &PredefinedMenuItem::separator(),
        &admin,
        &show_log,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|e| e.to_string())?;

    let event_loop = EventLoopBuilder::<Msg>::with_user_event().build();
    set_dock_icon();
    let proxy: EventLoopProxy<Msg> = event_loop.create_proxy();
    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = menu_proxy.send_event(Msg::Menu(e.id));
    }));
    let poller = server.clone();
    let poll_proxy = proxy.clone();
    std::thread::spawn(move || {
        loop {
            if poll_proxy.send_event(Msg::State(poller.state())).is_err() {
                break;
            }
            std::thread::sleep(POLL);
        }
    });

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon(State::Unknown)?)
        .with_tooltip(State::Unknown.label())
        .build()
        .map_err(|e| e.to_string())?;
    let mut shown = State::Unknown;
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        let Event::UserEvent(msg) = event else { return };
        match msg {
            Msg::State(s) => {
                if s != shown {
                    shown = s;
                    if let Ok(i) = icon(s) {
                        let _ = tray.set_icon(Some(i));
                    }
                    let _ = tray.set_tooltip(Some(s.label()));
                    status.set_text(s.label());
                    start.set_enabled(s.can_start());
                    stop.set_enabled(s.can_stop());
                }
            }
            Msg::Menu(id) => {
                let (server, proxy) = (server.clone(), proxy.clone());
                // starting and stopping take a moment: off the event loop, then look again at once
                let act = |arg: &'static str| {
                    std::thread::spawn(move || {
                        server.run(arg);
                        let _ = proxy.send_event(Msg::State(server.state()));
                    });
                };
                if id == *start.id() {
                    act("start");
                } else if id == *stop.id() {
                    act("stop");
                } else if id == *admin.id() {
                    open(&state::admin_url(port));
                } else if id == *show_log.id() {
                    if let Some(l) = &log {
                        open(&l.to_string_lossy());
                    }
                } else if id == *quit.id() {
                    *flow = ControlFlow::Exit;
                }
            }
        }
    });
}
