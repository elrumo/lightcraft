//! "Also open on <device>": this device tells the server now and then which photo it shows
//! (`POST /api/presence`), and is told which of the user's other devices show the same one. While one does, the
//! answer carries the sync log's head, so an edit made there is pulled at once instead of at the next poll.
//!
//! Presence is best-effort and kept cheap for the server (`apps/lightcraft-server/src/presence.rs` bounds it
//! on its side too):
//! - nothing is sent unless the account has two or more devices (`GET /api/me`, asked again every [`ME_EVERY`]);
//! - nothing is sent while the app is in the background (one last "none" is sent when it leaves), while no photo is
//!   shown, or during a slider drag;
//! - the server says when to speak next (`next`: a few seconds, sooner while a photo is shared); the answer is
//!   spread by up to ±20 % so devices don't line up, and never shorter than [`MIN_GAP`];
//! - a photo that stays shared without a change coming from the other device goes back to the slow rhythm after
//!   [`SHARED_FOR`];
//! - a refused or failed message backs off (doubling up to [`BACKOFF_MAX`]), and after [`FAILS_BEFORE_QUIET`] failures in a
//!   row presence goes quiet for [`QUIET`]; a server that doesn't know the route, or has it switched off, the same.
//!
//! Nothing here can hold up the library's own sync: the requests are side requests (like `usage`), never the
//! one control request, and every failure is swallowed.

use std::time::Duration;

use lightcraft_catalog::PhotoId;
use serde_json::Value;
use web_time::Instant;

use super::{Body, Done, SyncState, Task};
use crate::Session;

/// The account's devices are counted again this often.
const ME_EVERY: Duration = Duration::from_secs(5 * 60);
/// The least time between two messages.
const MIN_GAP: Duration = Duration::from_millis(1200);
/// What the server may ask us to wait between messages, at least and at most.
const NEXT_MIN: u64 = 1;
const NEXT_MAX: u64 = 60;
/// How long the fast rhythm lasts when the other device sends nothing new.
const SHARED_FOR: Duration = Duration::from_secs(60);
/// The slow rhythm (what the server asks for when the photo isn't shared).
const SLOW: Duration = Duration::from_secs(8);
/// After a failure: wait this long, doubling up to [`BACKOFF_MAX`].
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const FAILS_BEFORE_QUIET: u32 = 3;
/// How long presence stays quiet after repeated failures, a server without the route, or one that switched it off.
const QUIET: Duration = Duration::from_secs(5 * 60);

/// What a presence request in flight is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sent {
    /// `GET /api/me`: how many devices the account has.
    Me,
    /// `POST /api/presence`, saying this photo (`None`: none).
    Beat(Option<PhotoId>),
}

/// What the device knows about presence.
#[derive(Debug, Default)]
pub(super) struct Client {
    task: Option<(u64, Sent)>,
    /// Whether the account has other devices, and when that was asked.
    multi: Option<(bool, Instant)>,
    /// What was last said to the server and when (`None`: nothing yet).
    told: Option<(Option<PhotoId>, Instant)>,
    /// When to speak next.
    due: Option<Instant>,
    /// The names of the other devices showing the same photo.
    others: Vec<String>,
    /// Since when the photo has been shared without a change from the other device.
    shared_since: Option<Instant>,
    fails: u32,
    backoff: Option<Duration>,
    /// Not before then (after failures or a refusal).
    hold_until: Option<Instant>,
    /// The app is in front.
    foreground: bool,
    /// Counts the messages sent, to spread their timing.
    sent: u64,
    /// Bumped when an edit of the shown photo arrived from another device while it was shown there too.
    remote_edits: u64,
}

/// What to do next.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Nothing,
    AskMe,
    Say(Option<PhotoId>),
}

/// What [`Client::plan`] looks at.
struct Inputs {
    /// The photo shown here (the active photo).
    shown: Option<PhotoId>,
    /// A slider drag is in progress.
    dragging: bool,
}

impl Client {
    pub(super) fn set_foreground(&mut self, on: bool) {
        self.foreground = on;
    }

    /// The names of the other devices showing `shown`, the photo shown here: only while the server's last answer was
    /// about that photo (after a switch to another photo the badge goes at once, not at the next answer).
    pub(super) fn others_for(&self, shown: Option<PhotoId>) -> &[String] {
        match self.told {
            Some((p, _)) if p == shown && shown.is_some() => &self.others,
            _ => &[],
        }
    }

    pub(super) fn remote_edits(&self) -> u64 {
        self.remote_edits
    }

    /// An edit of the shown photo arrived from the server: it came from another device that shows it too, when
    /// there is one (the server doesn't say which device made it).
    pub(super) fn remote_edit(&mut self) {
        if !self.others.is_empty() {
            self.remote_edits += 1;
            // (news from the other device: the fast rhythm starts over)
            self.shared_since = Some(Instant::now());
        }
    }

    /// Forget everything (signed out, another account).
    pub(super) fn reset(&mut self) {
        *self = Client { foreground: self.foreground, ..Client::default() };
    }

    /// When presence next has something to do (the host comes back then), `None` when it has nothing to wait for.
    pub(super) fn due_in(&self, now: Instant) -> Option<Duration> {
        if self.task.is_some() {
            return None;
        }
        let at = [self.due, self.hold_until].into_iter().flatten().max()?;
        Some(at.saturating_duration_since(now))
    }

    fn plan(&self, i: &Inputs, now: Instant) -> Plan {
        if self.task.is_some() || self.hold_until.is_some_and(|t| now < t) {
            return Plan::Nothing;
        }
        // (in the background, with nothing shown: the one "none" is sent below, then silence)
        let shown = if self.foreground { i.shown } else { None };
        let multi = match self.multi {
            Some((m, at)) if now.saturating_duration_since(at) < ME_EVERY => m,
            _ if shown.is_none() && self.told.is_none_or(|(p, _)| p.is_none()) => return Plan::Nothing,
            _ => return Plan::AskMe,
        };
        if !multi {
            return Plan::Nothing;
        }
        let told = self.told.map(|(p, _)| p);
        let changed = told.is_some_and(|p| p != shown) || (told.is_none() && shown.is_some());
        if shown.is_none() && !changed {
            return Plan::Nothing;
        }
        if i.dragging || self.told.is_some_and(|(_, at)| now.saturating_duration_since(at) < MIN_GAP) {
            return Plan::Nothing;
        }
        if changed || self.due.is_none_or(|d| now >= d) { Plan::Say(shown) } else { Plan::Nothing }
    }

    /// `plan` after the answer, with the jitter that keeps devices from lining up.
    fn spread(&mut self, secs: Duration) -> Duration {
        // (a cheap spread, no random numbers needed: it only has to differ between devices and between messages)
        self.sent = self.sent.wrapping_add(1);
        let pm = self.sent.wrapping_mul(2_654_435_761) % 401; // 0..=400
        let factor = 800 + pm; // 800..=1200 permille
        let ms = u64::try_from(secs.as_millis()).unwrap_or(u64::MAX).saturating_mul(factor) / 1000;
        Duration::from_millis(ms).max(MIN_GAP)
    }

    fn failed(&mut self, now: Instant) {
        self.fails += 1;
        if self.fails >= FAILS_BEFORE_QUIET {
            self.hold_until = Some(now + QUIET);
            self.fails = 0;
            self.backoff = None;
            return;
        }
        let wait = self.backoff.map_or(BACKOFF_MIN, |b| (b * 2).min(BACKOFF_MAX));
        self.backoff = Some(wait);
        self.hold_until = Some(now + wait);
    }

    fn quiet(&mut self, now: Instant) {
        self.hold_until = Some(now + QUIET);
        self.others.clear();
        self.shared_since = None;
        self.fails = 0;
        self.backoff = None;
    }
}

impl Session {
    /// The app is in front (or not): presence says nothing from the background.
    pub fn sync_set_foreground(&mut self, on: bool) {
        if let Some(st) = self.sync.as_mut() {
            st.presence.set_foreground(on);
        }
    }

    /// The other devices of this account that show the photo shown here (empty: none, or presence is off).
    pub fn sync_also_open_on(&self) -> Vec<String> {
        self.sync.as_ref().map(|st| st.presence.others_for(self.active()).to_vec()).unwrap_or_default()
    }

    /// Bumped each time an edit of the shown photo arrived while another device showed it too: a frontend flashes
    /// "updated from another device" when it changes.
    pub fn sync_remote_edits(&self) -> u64 {
        self.sync.as_ref().map_or(0, |st| st.presence.remote_edits())
    }

    /// How long until presence next has something to say (`None`: nothing to wait for), for a host that comes back by timer.
    pub fn sync_presence_due_in(&self) -> Option<Duration> {
        let st = self.sync.as_ref()?;
        st.presence.due_in(Instant::now())
    }

    /// Presence's next request, if one is due (after the sync's own requests are decided).
    pub(super) fn presence_task(&mut self, st: &mut SyncState, now: Instant) -> Option<Task> {
        let inputs = Inputs { shown: self.active(), dragging: self.interaction.is_some() };
        match st.presence.plan(&inputs, now) {
            Plan::Nothing => None,
            Plan::AskMe => {
                let t = st.http("GET", "/api/me", Body::Empty, None);
                st.presence.task = Some((t.id(), Sent::Me));
                Some(t)
            }
            Plan::Say(photo) => {
                let body = match photo {
                    Some(id) => format!("{{\"photo\":\"p{}\"}}", id.0),
                    None => "{\"photo\":null}".to_string(),
                };
                let t = st.http("POST", "/api/presence", Body::Json(body), None);
                st.presence.task = Some((t.id(), Sent::Beat(photo)));
                // (counted from now: the next message waits MIN_GAP even if this one is slow to answer)
                st.presence.told = Some((photo, now));
                Some(t)
            }
        }
    }
}

impl SyncState {
    /// Whether `id` is a presence request in flight.
    pub(super) fn is_presence(&self, id: u64) -> bool {
        self.presence.task.is_some_and(|(t, _)| t == id)
    }

    /// The answer to a presence request.
    pub(super) fn presence_done(&mut self, d: &Done) {
        let now = Instant::now();
        let Some((_, sent)) = self.presence.task.take() else { return };
        match d.status {
            401 => return self.signed_out("the server signed this device out: sign in again"),
            // an older server, or one that switched presence off: nothing to show, and nothing to ask again soon
            404 | 405 => return self.presence.quiet(now),
            _ => {}
        }
        if !d.ok() {
            // (429 and 503 are the server asking for calm: the same back-off as a failure)
            self.presence.others.clear();
            self.presence.failed(now);
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(&d.body) else { return self.presence.failed(now) };
        self.presence.fails = 0;
        self.presence.backoff = None;
        match sent {
            Sent::Me => {
                let n = v["devices"].as_array().map_or(0, Vec::len);
                self.presence.multi = Some((n >= 2, now));
                if n < 2 {
                    self.presence.others.clear();
                }
            }
            Sent::Beat(_) => {
                if v["off"].as_bool() == Some(true) {
                    return self.presence.quiet(now);
                }
                let others: Vec<String> = v["others"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|o| o["name"].as_str()).map(|s| s.chars().take(80).collect()).take(16).collect())
                    .unwrap_or_default();
                // the other device's edit has landed on the server: pull it now instead of at the next poll
                let moved = v["head"].as_u64().is_some_and(|h| h > self.config.cursor);
                if moved && !others.is_empty() {
                    self.pull_at = None;
                    self.lively = now;
                }
                let p = &mut self.presence;
                let shared = !others.is_empty();
                if !shared || p.others.is_empty() || moved {
                    p.shared_since = shared.then_some(now);
                }
                p.others = others;
                let next = Duration::from_secs(v["next"].as_u64().unwrap_or(SLOW.as_secs()).clamp(NEXT_MIN, NEXT_MAX));
                // (a photo shared for a minute without news goes back to the slow rhythm, until the other device edits again)
                let calm = p.shared_since.is_some_and(|s| now.saturating_duration_since(s) >= SHARED_FOR);
                let wait = if calm { next.max(SLOW) } else { next };
                p.due = Some(now + p.spread(wait));
            }
        }
    }
}

#[cfg(test)]
impl Client {
    /// Make the next message due now (a test doesn't wait seconds).
    fn expedite(&mut self) {
        self.due = None;
        self.told = self.told.map(|(p, t)| (p, t.checked_sub(Duration::from_secs(10)).unwrap_or(t)));
    }
}

#[cfg(test)]
impl Session {
    pub(crate) fn presence_expedite(&mut self) {
        if let Some(st) = self.sync.as_mut() {
            st.presence.expedite();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client { foreground: true, ..Client::default() }
    }

    fn photo(n: u64) -> Option<PhotoId> {
        Some(PhotoId(n))
    }

    fn seen(c: &mut Client, multi: bool, now: Instant) {
        c.multi = Some((multi, now));
    }

    fn i(shown: Option<PhotoId>) -> Inputs {
        Inputs { shown, dragging: false }
    }

    #[test]
    fn it_asks_how_many_devices_there_are_before_saying_anything() {
        let c = client();
        let now = Instant::now();
        assert_eq!(c.plan(&i(photo(1)), now), Plan::AskMe);
        assert_eq!(c.plan(&i(None), now), Plan::Nothing, "nothing shown, nothing to ask about");
    }

    #[test]
    fn a_single_device_account_says_nothing() {
        let mut c = client();
        let now = Instant::now();
        seen(&mut c, false, now);
        assert_eq!(c.plan(&i(photo(1)), now), Plan::Nothing);
        // the count is asked again after a while (a second device may have signed in)
        assert_eq!(c.plan(&i(photo(1)), now + ME_EVERY + Duration::from_secs(1)), Plan::AskMe);
    }

    #[test]
    fn it_says_the_photo_at_once_then_on_the_servers_rhythm() {
        let mut c = client();
        let t0 = Instant::now();
        seen(&mut c, true, t0);
        assert_eq!(c.plan(&i(photo(1)), t0), Plan::Say(photo(1)));
        c.told = Some((photo(1), t0));
        c.due = Some(t0 + Duration::from_secs(8));
        assert_eq!(c.plan(&i(photo(1)), t0 + Duration::from_secs(3)), Plan::Nothing, "not due");
        assert_eq!(c.plan(&i(photo(1)), t0 + Duration::from_secs(9)), Plan::Say(photo(1)), "due");
        // another photo is said sooner, but never faster than MIN_GAP
        assert_eq!(c.plan(&i(photo(2)), t0 + Duration::from_millis(300)), Plan::Nothing);
        assert_eq!(c.plan(&i(photo(2)), t0 + MIN_GAP), Plan::Say(photo(2)));
    }

    #[test]
    fn it_says_none_once_when_the_photo_closes_or_the_app_leaves_then_stays_silent() {
        let mut c = client();
        let t0 = Instant::now();
        seen(&mut c, true, t0);
        c.told = Some((photo(1), t0));
        c.due = Some(t0 + Duration::from_secs(8));
        let later = t0 + Duration::from_secs(2);
        assert_eq!(c.plan(&i(None), later), Plan::Say(None), "the photo closed");
        c.told = Some((None, later));
        assert_eq!(c.plan(&i(None), later + Duration::from_secs(30)), Plan::Nothing, "then silence");
        // the app goes to the background while a photo is shown
        c.told = Some((photo(1), t0));
        c.set_foreground(false);
        assert_eq!(c.plan(&i(photo(1)), later), Plan::Say(None));
        c.told = Some((None, later));
        assert_eq!(c.plan(&i(photo(1)), later + Duration::from_secs(60)), Plan::Nothing, "silent in the background");
        c.set_foreground(true);
        assert_eq!(c.plan(&i(photo(1)), later + Duration::from_secs(61)), Plan::Say(photo(1)), "and again when it comes back");
    }

    #[test]
    fn it_says_nothing_during_a_slider_drag() {
        let mut c = client();
        let t0 = Instant::now();
        seen(&mut c, true, t0);
        assert_eq!(c.plan(&Inputs { shown: photo(1), dragging: true }, t0), Plan::Nothing);
    }

    #[test]
    fn failures_back_off_double_and_then_go_quiet() {
        let mut c = client();
        let t0 = Instant::now();
        seen(&mut c, true, t0);
        c.failed(t0);
        assert_eq!(c.plan(&i(photo(1)), t0 + Duration::from_secs(1)), Plan::Nothing, "held for the first back-off");
        assert_eq!(c.plan(&i(photo(1)), t0 + BACKOFF_MIN), Plan::Say(photo(1)));
        c.failed(t0 + BACKOFF_MIN);
        assert_eq!(c.backoff, Some(BACKOFF_MIN * 2), "doubles");
        c.failed(t0 + BACKOFF_MIN * 3);
        assert!(c.hold_until.is_some_and(|t| t >= t0 + QUIET), "after three in a row it goes quiet for five minutes");
        assert_eq!(c.plan(&i(photo(1)), t0 + Duration::from_secs(120)), Plan::Nothing);
        seen(&mut c, true, t0 + QUIET);
        assert_eq!(c.plan(&i(photo(1)), t0 + QUIET + Duration::from_secs(10)), Plan::Say(photo(1)), "and tries again afterwards");
    }

    #[test]
    fn the_rhythm_is_spread_but_never_faster_than_the_minimum() {
        let mut c = client();
        let spread: Vec<Duration> = (0..20).map(|_| c.spread(Duration::from_secs(2))).collect();
        assert!(spread.iter().all(|d| *d >= MIN_GAP && *d <= Duration::from_millis(2400)));
        assert!(spread.iter().collect::<std::collections::HashSet<_>>().len() > 3, "not all the same");
        assert_eq!(c.spread(Duration::from_millis(100)), MIN_GAP);
    }
}
