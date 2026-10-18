//! Background tasks in flight (issue #345): imports, exports, preview builds, downloads, the face scan… Each one
//! registers here while it runs, so every frontend can show them in one place (the desktop app's activity stack) and
//! cancel the ones that can be cancelled (`activity.list`, `activity.cancel`).
//!
//! A job holds a [`TaskGuard`] for as long as it runs; dropping it — when the job ends, fails or its thread unwinds
//! from a panic — removes the row, so a dead worker never leaves a stuck bar. Jobs keep the cancel flag they already
//! check between files or photos: the guard adopts it ([`Cancel::Flag`]), so the stack's ✕, `activity.cancel` and the
//! job's own Cancel all set the same atomic. Numbers are worded by the frontend from `done`, `total` and [`Unit`]; a
//! detail set here is a name (a file, a model), never a sentence, so the frontend can translate everything else.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;

/// The longest label or detail kept, in characters (a file name can be anything).
const MAX_TEXT: usize = 200;

/// The background tasks in flight. Cheap to clone (one `Arc`), so worker threads can hold it.
#[derive(Clone, Default)]
pub struct Activity {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Oldest first.
    tasks: Mutex<Vec<Arc<Entry>>>,
    next: AtomicU64,
    turns: Mutex<Turns>,
}

/// Who may run now in each pool of tasks limited to a few at once ([`Activity::try_turn`],
/// [`Activity::wait_turn`]): how many hold a turn, and the tickets waiting for one, in the order they asked.
#[derive(Default)]
struct Turns {
    running: HashMap<&'static str, usize>,
    waiting: HashMap<&'static str, VecDeque<u64>>,
    next: u64,
}

/// How a task can be cancelled.
pub enum Cancel {
    /// Not at all (no ✕): the face scan, AI Denoise.
    No,
    /// With a flag of its own ([`TaskGuard::cancel_flag`]).
    Yes,
    /// With the job's existing flag.
    Flag(Arc<AtomicBool>),
}

/// How the frontend words `done` / `total`: "3 of 25", "12 of 340 MB", "40 %".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    Count,
    Bytes,
    Percent,
}

impl Unit {
    fn from_u8(v: u8) -> Unit {
        match v {
            1 => Unit::Bytes,
            2 => Unit::Percent,
            _ => Unit::Count,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            Unit::Count => 0,
            Unit::Bytes => 1,
            Unit::Percent => 2,
        }
    }
}

/// One task as the frontends see it (`activity.list`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub id: u64,
    /// A stable id: `import`, `export`, `scan`, `previews`, `smartPreviews`, `lightroom`, `merge`, `download`,
    /// `faces`, `denoise`, `findMissing`, `applePhotos`, `cache`, `focus`, `renderDng`, `erase` (AI Remove painting a fill).
    pub kind: &'static str,
    /// English; the frontend translates it.
    pub label: String,
    pub done: u64,
    /// 0 while the amount of work isn't known yet (an indeterminate bar).
    pub total: u64,
    pub unit: Unit,
    /// The file, photo or model being worked on, if any.
    pub detail: String,
    pub cancellable: bool,
    /// Cancel was asked for; the job stops at its next check.
    pub cancelling: bool,
    /// Waiting for its turn (a model download while two others run): it starts by itself.
    pub queued: bool,
    /// How fast `done` grows, in units a second (0 while not known yet, or when it stalled).
    pub rate: u64,
    /// Time left at that rate, when it is known.
    pub eta_ms: Option<u64>,
    /// `rate` for a minute, for work that is slower than a unit a second (0 while not known yet).
    pub rate_per_min: u64,
    pub age_ms: u64,
}

/// How fast a task's `done` grows: an average over half-second steps. The first change after the task starts is
/// not counted, because a download that resumes, or skips the files it already has, jumps by what is on disk.
#[derive(Default)]
struct Meter {
    /// `done` when the meter started.
    first: Option<u64>,
    /// The last step's time and `done`.
    step: Option<(web_time::Instant, u64)>,
    rate: f64,
}

/// A rate not updated for this long is not trusted (the connection stalled).
const STALE_SECS: f64 = 3.0;

impl Meter {
    fn sample(&mut self, done: u64, now: web_time::Instant) {
        let Some(first) = self.first else {
            self.first = Some(done);
            return;
        };
        let Some((at, prev)) = self.step else {
            if done != first {
                self.step = Some((now, done));
            }
            return;
        };
        let dt = now.duration_since(at).as_secs_f64();
        if dt < 0.5 {
            return;
        }
        if let Some(grown) = done.checked_sub(prev) {
            let r = grown as f64 / dt;
            self.rate = if self.rate > 0.0 { self.rate * 0.7 + r * 0.3 } else { r };
        }
        self.step = Some((now, done));
    }

    /// The rate, or 0 while it isn't known or has gone stale.
    fn rate(&self, now: web_time::Instant) -> f64 {
        match self.step {
            Some((at, _)) if now.duration_since(at).as_secs_f64() < STALE_SECS && self.rate.is_finite() => self.rate,
            _ => 0.0,
        }
    }
}

struct Entry {
    id: u64,
    kind: &'static str,
    label: String,
    started: web_time::Instant,
    done: AtomicU64,
    total: AtomicU64,
    unit: AtomicU8,
    detail: Mutex<String>,
    cancel: Option<Arc<AtomicBool>>,
    cancellable: AtomicBool,
    queued: AtomicBool,
    meter: Mutex<Meter>,
    /// A rate the task measures itself, in units a second, instead of the meter's ([`TaskGuard::set_rate`]).
    rate_set: Mutex<Option<f64>>,
    /// Held while a cancel is decided ([`Activity::cancel`], [`Activity::cancel_all`]) and while
    /// cancelling is switched ([`TaskGuard::set_cancellable`], [`TaskHandle::set_cancellable`]): a
    /// step that turns it off and then reads the cancel flag either sees a cancel that was
    /// accepted, or makes the cancel fail. Without it a cancel could pass the check, the step
    /// start, and the flag be set after the step's last look at it: accepted but ignored.
    gate: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn capped(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

impl Entry {
    fn set_cancellable(&self, yes: bool) {
        let _gate = lock(&self.gate);
        self.cancellable.store(yes, Ordering::Relaxed);
    }

    /// Set the cancel flag if the task can be cancelled now (see [`Entry::gate`]).
    fn try_cancel(&self) -> bool {
        let _gate = lock(&self.gate);
        match &self.cancel {
            Some(flag) if self.is_cancellable() => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    fn is_cancellable(&self) -> bool {
        self.cancel.is_some() && self.cancellable.load(Ordering::Relaxed)
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
    }

    fn progress(&self, done: u64, total: u64) {
        let done = if total > 0 { done.min(total) } else { done };
        self.total.store(total, Ordering::Relaxed);
        self.done.store(done, Ordering::Relaxed);
        lock(&self.meter).sample(done, web_time::Instant::now());
    }

    fn set_queued(&self, yes: bool) {
        self.queued.store(yes, Ordering::Relaxed);
        // (the time spent waiting is not part of the rate)
        *lock(&self.meter) = Meter::default();
    }

    fn detail(&self, text: &str) {
        *lock(&self.detail) = capped(text);
    }

    fn info(&self) -> TaskInfo {
        let (done, total) = (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed));
        let queued = self.queued.load(Ordering::Relaxed);
        let rate = if queued {
            0.0
        } else {
            let own = *lock(&self.rate_set);
            own.filter(|r| r.is_finite() && *r > 0.0).unwrap_or_else(|| lock(&self.meter).rate(web_time::Instant::now()))
        };
        // (the float-to-int casts saturate; a NaN never gets here)
        let eta_ms = (rate > 0.0 && total > 0).then(|| (total.saturating_sub(done) as f64 / rate * 1000.0) as u64);
        TaskInfo {
            id: self.id,
            kind: self.kind,
            label: self.label.clone(),
            done,
            total,
            unit: Unit::from_u8(self.unit.load(Ordering::Relaxed)),
            detail: lock(&self.detail).clone(),
            cancellable: self.is_cancellable(),
            cancelling: self.is_cancelled(),
            queued,
            rate: rate.round() as u64,
            rate_per_min: (rate * 60.0).round() as u64,
            eta_ms,
            age_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    }
}

impl Activity {
    /// Register a task; it is listed until the returned guard is dropped. `label` is English (the frontend
    /// translates it).
    pub fn start(&self, kind: &'static str, label: &str, cancel: Cancel) -> TaskGuard {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let cancel = match cancel {
            Cancel::No => None,
            Cancel::Yes => Some(Arc::new(AtomicBool::new(false))),
            Cancel::Flag(flag) => Some(flag),
        };
        let entry = Arc::new(Entry {
            id,
            kind,
            label: capped(label),
            started: web_time::Instant::now(),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            unit: AtomicU8::new(Unit::Count.to_u8()),
            detail: Mutex::new(String::new()),
            cancellable: AtomicBool::new(cancel.is_some()),
            cancel,
            queued: AtomicBool::new(false),
            meter: Mutex::new(Meter::default()),
            rate_set: Mutex::new(None),
            gate: Mutex::new(()),
        });
        lock(&self.inner.tasks).push(entry.clone());
        TaskGuard { activity: self.clone(), entry }
    }

    /// Every task in flight, oldest first.
    pub fn list(&self) -> Vec<TaskInfo> {
        lock(&self.inner.tasks).iter().map(|e| e.info()).collect()
    }

    /// Ask task `id` to stop (it stops at its next check). An unknown id (the task may have just ended) or a task
    /// that can't be cancelled is an error.
    pub fn cancel(&self, id: u64) -> Result<(), String> {
        let tasks = lock(&self.inner.tasks);
        let entry = tasks.iter().find(|e| e.id == id).ok_or_else(|| format!("no task {id}"))?;
        if entry.try_cancel() { Ok(()) } else { Err(format!("{} can't be cancelled", entry.label)) }
    }

    /// Ask every cancellable task to stop; returns how many were asked.
    pub fn cancel_all(&self) -> usize {
        let tasks = lock(&self.inner.tasks);
        tasks.iter().filter(|e| e.try_cancel()).count()
    }

    /// Tasks that quitting would cut short: cancellable ones not already stopping.
    pub fn running_cancellable(&self) -> Vec<TaskInfo> {
        lock(&self.inner.tasks).iter().filter(|e| e.is_cancellable() && !e.is_cancelled()).map(|e| e.info()).collect()
    }

    /// A turn in `pool`, where at most `limit` tasks run at once, if one is free now and nobody waits for one.
    pub fn try_turn(&self, pool: &'static str, limit: usize) -> Option<Turn> {
        let mut t = lock(&self.inner.turns);
        let running = t.running.get(pool).copied().unwrap_or(0);
        if running >= limit || t.waiting.get(pool).is_some_and(|w| !w.is_empty()) {
            return None;
        }
        t.running.insert(pool, running.saturating_add(1));
        Some(Turn { activity: self.clone(), pool })
    }

    /// Wait for a turn in `pool` (at most `limit` tasks run at once; turns go in the order they were asked for).
    /// `None` when `cancel` is set while waiting. Blocks: call it on the task's own thread.
    pub fn wait_turn(&self, pool: &'static str, limit: usize, cancel: &AtomicBool) -> Option<Turn> {
        let ticket = {
            let mut t = lock(&self.inner.turns);
            t.next = t.next.wrapping_add(1);
            let ticket = t.next;
            t.waiting.entry(pool).or_default().push_back(ticket);
            ticket
        };
        loop {
            {
                let mut t = lock(&self.inner.turns);
                let running = t.running.get(pool).copied().unwrap_or(0);
                let first = t.waiting.get(pool).and_then(|w| w.front().copied());
                if cancel.load(Ordering::Relaxed) {
                    if let Some(w) = t.waiting.get_mut(pool) {
                        w.retain(|&x| x != ticket);
                    }
                    return None;
                }
                if running < limit && first == Some(ticket) {
                    if let Some(w) = t.waiting.get_mut(pool) {
                        w.pop_front();
                    }
                    t.running.insert(pool, running.saturating_add(1));
                    return Some(Turn { activity: self.clone(), pool });
                }
            }
            // ponytail: polls; a Condvar if many tasks ever wait at once
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// A turn to run in a limited pool ([`Activity::try_turn`]); dropping it frees the place for the next.
pub struct Turn {
    activity: Activity,
    pool: &'static str,
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut t = lock(&self.activity.inner.turns);
        if let Some(n) = t.running.get_mut(self.pool) {
            *n = n.saturating_sub(1);
        }
    }
}

/// A task's place in the list, held by whoever owns the job; dropping it removes the row.
pub struct TaskGuard {
    activity: Activity,
    entry: Arc<Entry>,
}

impl TaskGuard {
    pub fn id(&self) -> u64 {
        self.entry.id
    }

    /// `done` of `total` (`total` 0 = not known yet). `done` is kept at most `total`.
    pub fn progress(&self, done: u64, total: u64) {
        self.entry.progress(done, total);
    }

    /// The file, photo or model being worked on (a name, not a sentence).
    pub fn detail(&self, text: &str) {
        self.entry.detail(text);
    }

    /// How fast `done` grows, in units a second, as the task measures it (work that comes in slow batches: the
    /// meter, which wants a step every few seconds, reads it as stalled). `None`: the meter's.
    pub fn set_rate(&self, per_second: Option<f64>) {
        *lock(&self.entry.rate_set) = per_second;
    }

    pub fn set_unit(&self, unit: Unit) {
        self.entry.unit.store(unit.to_u8(), Ordering::Relaxed);
    }

    /// Waiting for its turn ([`Activity::wait_turn`]), or not any more.
    pub fn set_queued(&self, yes: bool) {
        self.entry.set_queued(yes);
    }

    /// A step that must not be cut short (the Lightroom import's commit) turns cancelling off for its duration.
    pub fn set_cancellable(&self, yes: bool) {
        self.entry.set_cancellable(yes);
    }

    pub fn cancel_flag(&self) -> Option<Arc<AtomicBool>> {
        self.entry.cancel.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.entry.is_cancelled()
    }

    /// For progress from another thread; it does not keep the row.
    pub fn handle(&self) -> TaskHandle {
        TaskHandle { entry: self.entry.clone() }
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        let id = self.entry.id;
        lock(&self.activity.inner.tasks).retain(|e| e.id != id);
    }
}

impl std::fmt::Debug for TaskGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskGuard").field("id", &self.entry.id).field("kind", &self.entry.kind).finish()
    }
}

/// Progress for a task from another thread (see [`TaskGuard::handle`]).
#[derive(Clone)]
pub struct TaskHandle {
    entry: Arc<Entry>,
}

impl TaskHandle {
    pub fn progress(&self, done: u64, total: u64) {
        self.entry.progress(done, total);
    }

    pub fn detail(&self, text: &str) {
        self.entry.detail(text);
    }

    pub fn is_cancelled(&self) -> bool {
        self.entry.is_cancelled()
    }

    /// The task as `activity.list` shows it (its row may be gone: the task ended).
    pub fn info(&self) -> TaskInfo {
        self.entry.info()
    }

    /// [`TaskGuard::set_cancellable`] from the worker, at the moment its step starts (an export
    /// handing its files to Apple Photos, which can't be stopped once asked). Read the cancel flag
    /// after turning it off: a cancel accepted before is seen then, any later one is refused.
    pub fn set_cancellable(&self, yes: bool) {
        self.entry.set_cancellable(yes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn guard_drop_removes_the_row() {
        let a = Activity::default();
        let g = a.start("export", "Exporting", Cancel::Yes);
        assert_eq!(a.list().len(), 1);
        drop(g);
        assert!(a.list().is_empty());
    }

    #[test]
    fn guard_drop_on_panic_unwind_removes_the_row() {
        let a = Activity::default();
        let a2 = a.clone();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _g = a2.start("import", "Importing", Cancel::Yes);
            panic!("synthetic worker panic");
        }));
        assert!(r.is_err());
        assert!(a.list().is_empty());
    }

    #[test]
    fn cancel_sets_the_adopted_flag() {
        let a = Activity::default();
        let flag = Arc::new(AtomicBool::new(false));
        let g = a.start("export", "Exporting", Cancel::Flag(flag.clone()));
        a.cancel(g.id()).unwrap();
        assert!(flag.load(Ordering::Relaxed) && g.is_cancelled());
        assert!(a.list()[0].cancelling);
    }

    #[test]
    fn cancel_errors_instead_of_panicking() {
        let a = Activity::default();
        assert!(a.cancel(42).is_err());
        let g = a.start("faces", "Finding faces", Cancel::No);
        assert!(a.cancel(g.id()).is_err());
        let h = a.start("lightroom", "Importing Lightroom catalog", Cancel::Yes);
        h.set_cancellable(false);
        assert!(a.cancel(h.id()).is_err());
        assert!(!h.is_cancelled());
    }

    #[test]
    fn cancel_all_counts_only_cancellable() {
        let a = Activity::default();
        let (_x, _y, _z) = (a.start("export", "E", Cancel::Yes), a.start("import", "I", Cancel::Yes), a.start("faces", "F", Cancel::No));
        assert_eq!(a.cancel_all(), 2);
        assert!(a.running_cancellable().is_empty());
    }

    #[test]
    fn progress_is_clamped_and_detail_capped() {
        let a = Activity::default();
        let g = a.start("export", "E", Cancel::Yes);
        g.progress(12, 10);
        g.detail(&"é".repeat(300));
        let t = &a.list()[0];
        assert_eq!((t.done, t.total), (10, 10));
        assert_eq!(t.detail.chars().count(), 200);
        g.progress(5, 0); // indeterminate keeps done as given
        assert_eq!(a.list()[0].total, 0);
        assert_eq!(a.list()[0].unit, Unit::Count);
        g.set_unit(Unit::Bytes);
        assert_eq!(a.list()[0].unit, Unit::Bytes);
    }

    #[test]
    fn ids_are_unique_and_list_is_oldest_first() {
        let a = Activity::default();
        let (g1, g2) = (a.start("export", "A", Cancel::Yes), a.start("import", "B", Cancel::Yes));
        assert!(g2.id() > g1.id());
        assert_eq!(a.list().iter().map(|t| t.id).collect::<Vec<_>>(), vec![g1.id(), g2.id()]);
    }

    #[test]
    fn poisoned_lock_still_works() {
        let a = Activity::default();
        let a2 = a.clone();
        let _ = std::thread::spawn(move || {
            let _held = a2.inner.tasks.lock().unwrap();
            panic!("synthetic panic while holding the task list");
        })
        .join();
        assert!(a.inner.tasks.is_poisoned());
        let _g = a.start("export", "E", Cancel::Yes);
        assert_eq!(a.list().len(), 1);
    }

    /// Codex review of #643: a worker that turns cancelling off and then reads the cancel flag
    /// (an export handing its files to Apple Photos) must either see a cancel that was accepted or
    /// make it fail, never let one be accepted and then ignored. Here the test holds the task's gate
    /// as a cancel does while it decides: the worker's switch waits for it, and then sees the cancel.
    #[test]
    fn a_step_that_turns_cancelling_off_sees_or_refuses_every_cancel() {
        let a = Activity::default();
        let flag = Arc::new(AtomicBool::new(false));
        let g = a.start("export", "Exporting", Cancel::Flag(flag.clone()));
        let handle = g.handle();
        let gate = lock(&g.entry.gate); // a cancel deciding
        let (tx, rx) = std::sync::mpsc::channel();
        let (h, f) = (handle.clone(), flag.clone());
        let worker = std::thread::spawn(move || {
            h.set_cancellable(false);
            tx.send(f.load(Ordering::Relaxed)).unwrap();
        });
        assert!(rx.recv_timeout(std::time::Duration::from_millis(100)).is_err(), "the switch waits for the cancel");
        flag.store(true, Ordering::Relaxed); // the cancel goes through...
        drop(gate);
        assert!(rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(), "...and the step sees it");
        worker.join().unwrap();
        // once it is off, cancels are refused
        let g2 = a.start("export", "Exporting", Cancel::Yes);
        g2.handle().set_cancellable(false);
        assert!(a.cancel(g2.id()).is_err());
        assert_eq!(a.cancel_all(), 0, "{:?}", a.list());
    }

    #[test]
    fn turns_go_in_order_and_a_cancel_leaves_the_line() {
        let a = Activity::default();
        let (t1, t2) = (a.try_turn("download", 2).unwrap(), a.try_turn("download", 2).unwrap());
        assert!(a.try_turn("download", 2).is_none());
        assert!(a.try_turn("other", 1).is_some(), "pools are separate");
        let (stop_b, stop_c) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (tx, rx) = std::sync::mpsc::channel();
        let wait = |name: &'static str, stop: Arc<AtomicBool>| {
            let (a, tx) = (a.clone(), tx.clone());
            std::thread::spawn(move || {
                let turn = a.wait_turn("download", 2, &stop);
                tx.send((name, turn.is_some())).unwrap();
                // (holds the turn a moment, then frees it)
                std::thread::sleep(std::time::Duration::from_millis(50));
            })
        };
        let b = wait("b", stop_b.clone());
        std::thread::sleep(std::time::Duration::from_millis(150));
        let c = wait("c", stop_c.clone());
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(rx.try_recv().is_err(), "both wait while two run");
        assert!(a.try_turn("download", 2).is_none(), "nobody jumps the line");
        // b gives up; when a place frees, c gets it
        stop_b.store(true, Ordering::Relaxed);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(), ("b", false));
        drop(t1);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(), ("c", true));
        b.join().unwrap();
        c.join().unwrap();
        drop(t2);
        assert!(a.try_turn("download", 2).is_some() && a.try_turn("download", 2).is_some());
    }

    #[test]
    fn the_rate_skips_the_first_jump_and_goes_stale() {
        let t0 = web_time::Instant::now();
        let at = |ms: u64| t0 + std::time::Duration::from_millis(ms);
        let mut m = Meter::default();
        m.sample(0, at(0));
        // a resumed download reports what is already on disk first
        m.sample(900_000, at(100));
        assert_eq!(m.rate(at(100)), 0.0);
        m.sample(1_000_000, at(1100));
        assert!((m.rate(at(1100)) - 100_000.0).abs() < 1.0, "{}", m.rate(at(1100)));
        // steps shorter than half a second aren't measured
        m.sample(1_000_100, at(1200));
        assert!((m.rate(at(1200)) - 100_000.0).abs() < 1.0);
        // nothing for a while: not trusted any more
        assert_eq!(m.rate(at(5000)), 0.0);
        // a row while it waits has no rate; once it runs, the ETA follows from the rate
        let a = Activity::default();
        let g = a.start("download", "Downloading model", Cancel::Yes);
        g.set_queued(true);
        g.progress(0, 1000);
        let r = &a.list()[0];
        assert!(r.queued && r.rate == 0 && r.eta_ms.is_none());
        g.set_queued(false);
        assert!(!a.list()[0].queued);
        g.entry.meter.lock().unwrap().rate = 100.0;
        g.entry.meter.lock().unwrap().step = Some((web_time::Instant::now(), 500));
        g.progress(500, 1000);
        let r = &a.list()[0];
        assert_eq!((r.rate, r.eta_ms.map(|ms| ms / 100)), (100, Some(50)));
    }

    #[test]
    fn handle_updates_from_another_thread() {
        let a = Activity::default();
        let g = a.start("previews", "Building previews", Cancel::Yes);
        let h = g.handle();
        std::thread::spawn(move || h.progress(3, 7)).join().unwrap();
        assert_eq!((a.list()[0].done, a.list()[0].total), (3, 7));
    }
}
