//! The page's life as the browser sees it: a tab sent to the background and brought back, and the
//! graphics the browser takes away from a tab and may give back (`webglcontextlost` /
//! `webglcontextrestored`).
//!
//! eframe draws with one WebGL context and has no answer to losing it: every texture and shader
//! is gone, the canvas stays empty or stale, and the first new texture panics inside the painter.
//! The page then looks frozen while the app behind it still runs. Browsers do this to tabs in the
//! background (memory pressure, a GPU process restart, the machine sleeping), so this is what
//! "the web app froze after I switched tabs" looks like.
//!
//! What happens now: the loss is noted (and the event cancelled, so the browser may restore the
//! context); the app stops painting (`raw_input_hook` marks the window hidden, so eframe runs the
//! app's logic and no frame, and the painter is not called); a message covers the canvas; and
//! once the tab is in front and the library is in storage, the page reloads itself, which gives it
//! new graphics. A page that keeps losing them is not reloaded again and again
//! ([`crate::health::reload_budget`]); the message has a Reload button instead.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::files::Files;
use crate::health::{Loss, Next, after_loss, reload_budget};
use crate::safety;

/// How often a page that lost its graphics looks at itself (ms).
const TICK_MS: i32 = 500;

/// Where the reloads of this tab are written down.
const RELOADS_KEY: &str = "lightcraft-reloads";

/// What the page keeps in storage, as far as the recovery is concerned.
#[derive(Clone)]
pub struct Saved {
    pub files: Files,
    /// A flush to storage is running.
    pub flushing: Rc<Cell<bool>>,
}

#[derive(Default)]
struct State {
    lost: Cell<bool>,
    restored: Cell<bool>,
    /// When the graphics were lost, or the tab last came back to the front while they were (ms).
    since: Cell<f64>,
    gave_up: Cell<bool>,
    saved: RefCell<Option<Saved>>,
}

thread_local! {
    static STATE: State = State::default();
}

fn window() -> Option<web_sys::Window> {
    web_sys::window()
}

fn hidden() -> bool {
    window().and_then(|w| w.document()).is_none_or(|d| d.hidden())
}

/// Are the graphics gone? The app paints nothing meanwhile.
pub fn graphics_lost() -> bool {
    STATE.with(|s| s.lost.get())
}

/// Tell the recovery what it must wait for before it reloads (from the app, once it exists).
pub fn set_saved(saved: Saved) {
    STATE.with(|s| *s.saved.borrow_mut() = Some(saved));
}

fn listen(target: &web_sys::EventTarget, name: &str, mut f: impl FnMut(web_sys::Event) + 'static) {
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| f(e));
    if let Err(e) = target.add_event_listener_with_callback(name, closure.as_ref().unchecked_ref()) {
        log::warn!("lifecycle: can't listen to {name}: {e:?}");
    }
    // (for the life of the page)
    closure.forget();
}

/// Has the browser taken the canvas's WebGL context away? (Asked when the tab comes back, in case
/// the event was missed.)
fn context_is_lost(canvas: &web_sys::HtmlCanvasElement) -> bool {
    if let Some(gl) = canvas.get_context("webgl2").ok().flatten().and_then(|c| c.dyn_into::<web_sys::WebGl2RenderingContext>().ok()) {
        return gl.is_context_lost();
    }
    canvas
        .get_context("webgl")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<web_sys::WebGlRenderingContext>().ok())
        .is_some_and(|gl| gl.is_context_lost())
}

fn note_lost() {
    let first = STATE.with(|s| {
        let first = !s.lost.replace(true);
        if first {
            s.restored.set(false);
            s.gave_up.set(false);
            s.since.set(js_sys::Date::now());
        }
        first
    });
    if first {
        log::warn!("lightcraft: the browser took the graphics away (the tab was probably in the background)");
        safety::show_blocking(
            "The browser took this tab's graphics away, as it does to tabs in the background.\n\nLightCraft starts again in a moment; your library is kept.",
        );
    }
    crate::web::request_repaint();
}

fn note_visible(canvas: &web_sys::HtmlCanvasElement) {
    if hidden() {
        return;
    }
    // (a missed event, or a context lost while the page could not run)
    if !graphics_lost() && context_is_lost(canvas) {
        note_lost();
    }
    STATE.with(|s| {
        if s.lost.get() {
            s.since.set(js_sys::Date::now());
        }
    });
    // the canvas may come back empty, and an idle app draws nothing by itself
    crate::web::request_repaint();
}

fn session_storage() -> Option<web_sys::Storage> {
    window()?.session_storage().ok().flatten()
}

/// One look at a page that has lost its graphics: reload, wait, or stop trying.
fn tick() {
    let (lost, restored, since, gave_up, saved) =
        STATE.with(|s| (s.lost.get(), s.restored.get(), s.since.get(), s.gave_up.get(), s.saved.borrow().clone()));
    if !lost || gave_up {
        return;
    }
    let now = js_sys::Date::now();
    let history = session_storage().and_then(|s| s.get_item(RELOADS_KEY).ok().flatten()).unwrap_or_default();
    let (allowed, history) = reload_budget(&history, now);
    let (is_saved, save_failed) = match &saved {
        Some(s) => (!s.files.is_dirty() && !s.flushing.get(), s.files.error().is_some()),
        None => (true, false),
    };
    let loss = Loss { visible: !hidden(), visible_ms: (now - since).max(0.0), restored, saved: is_saved, save_failed, reload_allowed: allowed };
    match after_loss(loss) {
        Next::Wait => {}
        Next::Reload => {
            log::info!("lightcraft: reloading to get graphics back");
            if let Some(s) = session_storage() {
                let _ = s.set_item(RELOADS_KEY, &history);
            }
            if let Some(w) = window() {
                let _ = w.location().reload();
            }
        }
        Next::GiveUp => {
            STATE.with(|s| s.gave_up.set(true));
            let why = if save_failed {
                "The library could not be written to this browser's storage, so reloading would lose the changes made since."
            } else if !allowed {
                "It keeps happening."
            } else {
                "The library is still being written to this browser's storage."
            };
            safety::show_blocking(&format!(
                "The browser took this tab's graphics away and LightCraft could not get them back.\n\n{why}\n\nReload the page when you are ready; the library saved in this browser is kept."
            ));
        }
    }
}

/// Watch the page's lifecycle: call once, with the canvas the app draws on, before eframe starts.
pub fn install(canvas: &web_sys::HtmlCanvasElement) {
    let Some(window) = window() else { return };
    let Some(document) = window.document() else { return };
    // Cancelling the event is what lets the browser restore the context later.
    listen(canvas, "webglcontextlost", |e| {
        e.prevent_default();
        note_lost();
    });
    listen(canvas, "webglcontextrestored", |_| {
        log::info!("lightcraft: the browser gave the graphics back");
        STATE.with(|s| s.restored.set(true));
        crate::web::request_repaint();
    });
    let c = canvas.clone();
    listen(&document, "visibilitychange", move |_| note_visible(&c));
    let c = canvas.clone();
    listen(&window, "pageshow", move |_| note_visible(&c));
    // (Page Lifecycle: a frozen page that thaws)
    let c = canvas.clone();
    listen(&document, "resume", move |_| note_visible(&c));
    let timer = Closure::<dyn FnMut()>::new(tick);
    if let Err(e) = window.set_interval_with_callback_and_timeout_and_arguments_0(timer.as_ref().unchecked_ref(), TICK_MS) {
        log::warn!("lifecycle: no timer: {e:?}");
    }
    timer.forget();
}
