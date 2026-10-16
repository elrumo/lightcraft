//! Decisions about the page's health, without the browser: when a render worker is started again,
//! and when the page may reload itself after the browser took its graphics away. `workers.rs` and
//! `lifecycle.rs` apply them; the tests here pin the rules.

/// How many times in a row a worker is started again before it is given up for the session.
pub const MAX_RESTARTS: u32 = 6;

/// Reloads the page may make by itself within [`RELOAD_WINDOW_MS`] (a graphics driver that fails
/// again at once must not make a reload loop).
pub const MAX_RELOADS: usize = 3;
pub const RELOAD_WINDOW_MS: f64 = 60_000.0;

/// How long (ms) the page waits, in front, for the browser to give its graphics back before it
/// asks for fresh ones by reloading.
pub const RESTORE_WAIT_MS: f64 = 2_000.0;

/// How long (ms), in front, the page waits for the library to be written to storage before it
/// stops waiting and leaves the decision to the user.
pub const SAVE_WAIT_MS: f64 = 20_000.0;

/// How long a worker waits to be started again after its `restarts`-th failure in a row (ms).
pub fn restart_delay_ms(restarts: u32) -> f64 {
    (500.0 * 2f64.powi(restarts.min(10) as i32)).min(30_000.0)
}

/// The reloads this page made lately, kept as text in the tab's session storage
/// (`"t1,t2,t3"`, times in ms since the epoch, oldest first).
///
/// Returns whether one more reload is allowed at `now`, and the history to store (with `now` added
/// when allowed). Text that does not parse counts as no history; times in the future (a clock
/// that went back) count as old.
pub fn reload_budget(history: &str, now: f64) -> (bool, String) {
    let mut recent: Vec<f64> = history
        .split(',')
        .filter_map(|t| t.trim().parse::<f64>().ok())
        .filter(|t| t.is_finite() && *t <= now && now - *t < RELOAD_WINDOW_MS)
        .collect();
    let allowed = recent.len() < MAX_RELOADS;
    if allowed {
        recent.push(now);
    }
    // (never more than the last few: the text is read back on every loss)
    let keep = recent.len().saturating_sub(MAX_RELOADS);
    let text = recent.get(keep..).unwrap_or_default().iter().map(|t| format!("{t:.0}")).collect::<Vec<_>>().join(",");
    (allowed, text)
}

/// What the page knows when the browser has taken its graphics away.
#[derive(Clone, Copy, Debug)]
pub struct Loss {
    /// The tab is in front.
    pub visible: bool,
    /// How long it has been in front since the loss, or since it came back to the front (ms).
    pub visible_ms: f64,
    /// The browser gave the graphics back.
    pub restored: bool,
    /// Everything the library changed is in browser storage.
    pub saved: bool,
    /// Saving to browser storage fails (full, cleared): the page in memory is the only copy.
    pub save_failed: bool,
    /// [`reload_budget`] allows one more reload.
    pub reload_allowed: bool,
}

/// What to do about lost graphics.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    /// Not yet.
    Wait,
    /// Reload the page: the library is in storage and a new page gets new graphics.
    Reload,
    /// Do not reload; tell the user (the message has a Reload button).
    GiveUp,
}

/// Graphics come back only to a page that starts again, so the page reloads once the tab is in
/// front and nothing would be lost by it: not while it is in the background, not while a change
/// is on its way to storage, and not at all when storage fails or the page keeps losing its
/// graphics.
pub fn after_loss(l: Loss) -> Next {
    if !l.visible {
        return Next::Wait;
    }
    if l.save_failed {
        return Next::GiveUp;
    }
    if !l.restored && l.visible_ms < RESTORE_WAIT_MS {
        return Next::Wait;
    }
    if !l.saved {
        return if l.visible_ms >= SAVE_WAIT_MS { Next::GiveUp } else { Next::Wait };
    }
    if l.reload_allowed { Next::Reload } else { Next::GiveUp }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loss() -> Loss {
        Loss { visible: true, visible_ms: 0.0, restored: false, saved: true, save_failed: false, reload_allowed: true }
    }

    #[test]
    fn the_wait_before_a_restart_grows_and_stops_growing() {
        assert_eq!(restart_delay_ms(0), 500.0);
        assert_eq!(restart_delay_ms(1), 1000.0);
        assert_eq!(restart_delay_ms(3), 4000.0);
        assert_eq!(restart_delay_ms(6), 30_000.0);
        assert_eq!(restart_delay_ms(u32::MAX), 30_000.0);
    }

    #[test]
    fn a_page_reloads_itself_a_few_times_a_minute_and_no_more() {
        let t0 = 1_000_000.0;
        let (ok, h) = reload_budget("", t0);
        assert!(ok);
        let (ok, h) = reload_budget(&h, t0 + 5_000.0);
        assert!(ok);
        let (ok, h) = reload_budget(&h, t0 + 10_000.0);
        assert!(ok);
        let (ok, h) = reload_budget(&h, t0 + 15_000.0);
        assert!(!ok, "the fourth within a minute is a loop");
        assert_eq!(h.split(',').count(), 3, "and is not written down");
        let (ok, _) = reload_budget(&h, t0 + RELOAD_WINDOW_MS + 6_000.0);
        assert!(ok, "the earliest ones are old by then");
    }

    #[test]
    fn a_history_that_cannot_be_read_allows_a_reload() {
        for text in ["", "nonsense", ",,,", "NaN,inf", "-1e400", "9999999999999999"] {
            let (ok, kept) = reload_budget(text, 1_000.0);
            assert!(ok, "{text:?}");
            assert!(kept.split(',').all(|t| t.parse::<f64>().is_ok()), "{text:?} gave {kept:?}");
        }
        // a clock set back: what is "in the future" counts as old, not as recent
        let (ok, _) = reload_budget("5000,5001,5002", 1_000.0);
        assert!(ok);
    }

    #[test]
    fn lost_graphics_wait_for_the_tab_the_browser_and_storage() {
        assert_eq!(after_loss(Loss { visible: false, restored: true, visible_ms: 99_000.0, ..loss() }), Next::Wait, "a tab in the background");
        assert_eq!(after_loss(loss()), Next::Wait, "the browser may still give them back");
        assert_eq!(after_loss(Loss { visible_ms: RESTORE_WAIT_MS, ..loss() }), Next::Reload, "it did not");
        assert_eq!(after_loss(Loss { restored: true, ..loss() }), Next::Reload, "it did, but they are empty");
        assert_eq!(after_loss(Loss { restored: true, saved: false, ..loss() }), Next::Wait, "a change is on its way to storage");
        assert_eq!(after_loss(Loss { restored: true, saved: false, visible_ms: SAVE_WAIT_MS, ..loss() }), Next::GiveUp, "and it does not arrive");
    }

    #[test]
    fn lost_graphics_never_cost_the_user_work_or_loop() {
        let l = Loss { restored: true, ..loss() };
        assert_eq!(after_loss(Loss { save_failed: true, ..l }), Next::GiveUp, "storage fails: memory holds the only copy");
        assert_eq!(after_loss(Loss { save_failed: true, saved: false, ..l }), Next::GiveUp);
        assert_eq!(after_loss(Loss { reload_allowed: false, ..l }), Next::GiveUp, "it keeps happening: the user decides");
    }
}
