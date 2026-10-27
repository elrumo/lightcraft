//! The macOS menu-bar icon: a `tao` event loop (no window) with a `tray-icon` menu.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

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

fn icon(state: State) -> Result<Icon, String> {
    Icon::from_rgba(state::icon_rgba(state), state::ICON_SIZE, state::ICON_SIZE).map_err(|e| e.to_string())
}

pub fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let port = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()).unwrap_or(8080);
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty()).map(PathBuf::from);
    let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from));
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
