//! A pull held open on the server (`GET /api/ops?since=N&limit=1&wait=S`, `docs/sync.md` "Held pulls"): while the app
//! is in front, the device leaves one request waiting at the server, which answers the moment another device's change
//! lands (or after `S` seconds). The answer only says that something moved: the library's own pull (the one control
//! request) then brings it, so nothing here changes the order in which the library is synced. While a held pull
//! waits, the timed pull ([`super::pull_gap`]) isn't sent, and another device's change arrives at once instead of at
//! the next poll. Held pulls keep the timer's rhythm: the next one goes when the timed pull would have
//! ([`super::pull_gap`] after the last one went, at least [`GAP`]), so a device that just saw a change asks about
//! every 26 s instead of every 5, and one quiet for minutes still asks every 30 s (with a 25 s wait, a change waits
//! at most 5 s then).
//!
//! - It is a side request (like presence): a push, a sign-in or a reload never waits for it.
//! - Only a server that says it holds pulls (`wait` in its answers to a pull) is asked; an older one answers at
//!   once and says nothing, and the device goes on pulling on its timer.
//! - Nothing is held from the background: the host asks for it ([`Session::sync_watch`]) only with the app in
//!   front, and only hosts that run requests side by side ask (never `sync_now`, which waits for each answer).
//! - A failed held pull waits [`AFTER_FAILURE`] before the next (the timer pulls meanwhile); one answered early with
//!   nothing new (a server that doesn't really hold) doubles the gap up to the same.

use std::time::Duration;

use web_time::Instant;

use super::{Body, Done, POLL_IDLE, SyncState, Task, decode, pull_gap};
use crate::Session;
use lightcraft_catalog::sync::proto;

/// The longest wait asked of the server, in seconds (it may say less).
const WAIT_MAX: u64 = 25;
/// The least time between two held pulls.
const GAP: Duration = Duration::from_secs(1);
/// After a failure (and the most the gap grows to): the timer pulls meanwhile.
const AFTER_FAILURE: Duration = POLL_IDLE;

#[derive(Debug, Default)]
pub(super) struct Watch {
    /// The held pull in flight and when it was sent.
    task: Option<(u64, Instant)>,
    /// The longest the server holds a pull (from its last answer to a pull; 0: it doesn't).
    pub(super) wait: u64,
    /// The app is in front.
    foreground: bool,
    /// Not before then.
    hold_until: Option<Instant>,
    gap: Duration,
}

impl Watch {
    pub(super) fn set_foreground(&mut self, on: bool) {
        self.foreground = on;
    }

    /// A held pull is waiting at the server (the timed pull isn't needed meanwhile).
    pub(super) fn waiting(&self) -> bool {
        self.task.is_some()
    }

    /// Forget the request in flight (signed out: its answer is for a token that is gone).
    pub(super) fn reset(&mut self) {
        *self = Watch { foreground: self.foreground, ..Watch::default() };
    }

    #[cfg(test)]
    pub(super) fn expedite(&mut self) {
        self.hold_until = None;
    }

    /// (tests) How long until the next held pull may go.
    #[cfg(test)]
    pub(super) fn due_in(&self) -> Option<Duration> {
        self.hold_until.map(|t| t.saturating_duration_since(Instant::now()))
    }

    /// (tests) As if the held pull in flight had been sent `ago`.
    #[cfg(test)]
    pub(super) fn sent_ago(&mut self, ago: Duration) {
        if let Some((_, sent)) = self.task.as_mut() {
            *sent = Instant::now().checked_sub(ago).unwrap_or(*sent);
        }
    }

    fn pause(&mut self, now: Instant, gap: Duration) {
        self.gap = gap.clamp(GAP, AFTER_FAILURE);
        self.hold_until = Some(now + self.gap);
    }
}

impl Session {
    /// The pull to hold open at the server now, if one is due: for a host that runs requests side by side (its answer
    /// comes back through [`Session::sync_done`] like any other). `None` in the background, while the library is being
    /// read, signing in or out, paused, after a failure, or with a server that doesn't hold pulls.
    pub fn sync_watch(&mut self) -> Option<Task> {
        let st = self.sync.as_mut()?;
        let now = Instant::now();
        let w = &st.watch;
        let ready = w.wait > 0 && w.foreground && w.task.is_none() && w.hold_until.is_none_or(|t| now >= t);
        let idle_enough = !st.config.paused
            && !st.config.token.is_empty()
            && !st.config.library.is_empty()
            && !st.config.leaving
            && st.login.is_none()
            && st.conflict.is_none()
            && !st.needs_snapshot
            && st.retry_at.is_none_or(|t| now >= t);
        if !ready || !idle_enough {
            return None;
        }
        let path = format!("/api/ops?since={}&limit=1&wait={}", st.config.cursor, st.watch.wait.min(WAIT_MAX));
        let t = st.http("GET", &path, Body::Empty, None);
        st.watch.task = Some((t.id(), now));
        Some(t)
    }

    /// A held pull is waiting at the server: it doesn't make the sync busy (a host that comes back sooner while
    /// requests are in flight leaves this one out).
    pub fn sync_watching(&self) -> bool {
        self.sync.as_ref().is_some_and(|st| st.watch.waiting())
    }
}

impl SyncState {
    /// Whether `id` is the held pull.
    pub(super) fn is_watch(&self, id: u64) -> bool {
        self.watch.task.is_some_and(|(t, _)| t == id)
    }

    /// The answer to the held pull: pull now if the server has moved past this device, else not before the timer
    /// would have (counted from now: the answer said nothing is new).
    pub(super) fn watch_done(&mut self, d: &Done) {
        let now = Instant::now();
        let Some((_, sent)) = self.watch.task.take() else { return };
        match d.status {
            401 => return self.signed_out("the server signed this device out: sign in again"),
            // (the log no longer goes back that far: the library's own pull reloads it)
            410 => {
                self.pull_at = None;
                return self.watch.pause(now, GAP);
            }
            _ => {}
        }
        let ops = match (d.ok(), decode::<proto::Ops>(d)) {
            (true, Ok(ops)) => ops,
            _ => return self.watch.pause(now, AFTER_FAILURE),
        };
        self.watch.wait = ops.wait;
        let news = ops.head > self.config.cursor || ops.presets > self.presets_remote;
        self.presets_remote = self.presets_remote.max(ops.presets);
        self.note_docs(&ops.docs);
        if news {
            self.pull_at = None;
            self.lively = now;
        } else if let Some(t) = self.pull_at {
            self.pull_at = Some(t.max(now + pull_gap(now.saturating_duration_since(self.lively))));
        }
        // back so soon with nothing new: a server that doesn't really hold, or this device's own push; wait longer
        let held = now.saturating_duration_since(sent);
        let early = !news && held < Duration::from_secs(ops.wait.min(WAIT_MAX) / 2);
        let gap = if news {
            GAP
        } else if early {
            self.watch.gap * 2
        } else {
            // (the wait ran out: the next goes when the timed pull would have)
            pull_gap(now.saturating_duration_since(self.lively)).saturating_sub(held)
        };
        self.watch.pause(now, gap);
    }
}
