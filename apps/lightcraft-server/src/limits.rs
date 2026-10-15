//! How much of the machine the server's heavy work may take: CPU threads, previews built at once,
//! renders at once and requests answered at once.
//!
//! Each limit is, first to last: what the deployment fixed when the server started (`--threads`,
//! `LIGHTCRAFT_PREVIEW_THREADS`, …; the admin page shows it and can't change it), what an admin
//! saved on the admin page (`<data>/limits.json`), or automatic, from this machine's cores and
//! memory (a container's limits count). Nothing is dropped for want of a place: previews wait in
//! their queue, and renders and requests over their limit are told to try again (devices do).
//!
//! Previews, renders and requests follow a change at once. The CPU threads are the size of the
//! thread pool every decode, develop and model run shares (rayon's, and rten's for the text and face
//! models); a pool can't be resized, so a new number takes effect when the server restarts.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The file in the data folder.
pub const FILE: &str = "limits.json";

/// What one preview build may take at its peak (a big raw decoded, its previews made).
const PREVIEW_MEMORY: u64 = 3 << 29; // 1.5 GiB
/// What one render may take: a 48 MP export ([`lightcraft_engine::memory::export_need`]) and its source.
const RENDER_MEMORY: u64 = 4 << 30;
/// Preview builders at most (one thread each, waiting when the limit is lower).
const MAX_PREVIEWS: usize = 32;
const MAX_RENDERS: usize = 16;
/// Requests at most: the connections the HTTP server takes ([`crate::http::Limits::connections`]).
const MAX_REQUESTS: usize = 512;

/// One number per limit; `None`: not set at this level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Values {
    pub threads: Option<usize>,
    pub previews: Option<usize>,
    pub renders: Option<usize>,
    pub requests: Option<usize>,
}

/// The limits in effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effective {
    pub threads: usize,
    pub previews: usize,
    pub renders: usize,
    pub requests: usize,
}

/// What the automatic limits are worked out from.
#[derive(Clone, Copy, Debug)]
pub struct Machine {
    pub cores: usize,
    /// Bytes (`None`: the platform doesn't say).
    pub memory: Option<u64>,
}

impl Machine {
    pub fn this() -> Machine {
        Machine {
            cores: std::thread::available_parallelism().map_or(1, |n| n.get()),
            memory: lightcraft_engine::memory::total_ram().map(|b| b as u64),
        }
    }

    /// A quarter of the cores is left to the rest of the machine; previews and renders as many as
    /// memory holds (memory unknown: one render at a time).
    pub fn automatic(&self) -> Effective {
        let threads = self.cores.saturating_sub(self.cores / 4).max(1);
        let fit = |each: u64| self.memory.map_or(usize::MAX, |m| usize::try_from(m / each).unwrap_or(usize::MAX)).max(1);
        Effective {
            threads,
            previews: (threads / 2).clamp(1, 4).min(fit(PREVIEW_MEMORY)),
            renders: if self.memory.is_some() { fit(RENDER_MEMORY).clamp(1, 2) } else { 1 },
            requests: 64,
        }
    }

    /// The most each limit may be.
    pub fn max(&self) -> Effective {
        Effective { threads: self.cores.max(1), previews: self.cores.clamp(1, MAX_PREVIEWS), renders: MAX_RENDERS, requests: MAX_REQUESTS }
    }
}

/// The limits, where each comes from, and the numbers the HTTP server reads.
pub struct Limits {
    /// `<data>/limits.json` (`None`: nowhere to keep what an admin sets).
    file: Option<PathBuf>,
    pub machine: Machine,
    fixed: Values,
    saved: Mutex<Values>,
    /// The CPU threads the server started with (pools can't be resized).
    threads_now: usize,
    /// Requests answered at once: the HTTP server reads it for each request.
    pub(crate) requests: Arc<AtomicUsize>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the deployment sets each limit with (the admin page names it).
const FLAGS: [&str; 4] =
    ["--threads / LIGHTCRAFT_THREADS", "LIGHTCRAFT_PREVIEW_THREADS", "LIGHTCRAFT_RENDER_THREADS", "--max-requests / LIGHTCRAFT_MAX_REQUESTS"];

impl Limits {
    /// The limits for a server keeping its data in `data`, with what the deployment `fixed`. Sizes the
    /// thread pools: call it before anything runs on them.
    pub fn new(data: Option<&Path>, machine: Machine, fixed: Values) -> Limits {
        let file = data.map(|d| d.join(FILE));
        let saved = file.as_deref().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut l = Limits { file, machine, fixed, saved: Mutex::new(saved), threads_now: 0, requests: Arc::new(AtomicUsize::new(1)) };
        let e = l.effective();
        l.threads_now = e.threads;
        l.requests.store(e.requests, Ordering::SeqCst);
        // (fails when the pool already runs: a second server in one process, as tests start them)
        if rayon::ThreadPoolBuilder::new().num_threads(e.threads).build_global().is_err() {
            log::debug!("rayon's thread pool was already running: it keeps its size");
        }
        lightcraft_vision::set_model_threads(e.threads);
        l
    }

    /// What the limits are now (the CPU threads: what a restart would use, see [`Limits::threads_now`]).
    pub fn effective(&self) -> Effective {
        let saved = *lock(&self.saved);
        let (auto, max) = (self.machine.automatic(), self.machine.max());
        let pick = |fixed: Option<usize>, saved: Option<usize>, auto: usize, max: usize| fixed.or(saved).unwrap_or(auto).clamp(1, max);
        Effective {
            threads: pick(self.fixed.threads, saved.threads, auto.threads, max.threads),
            previews: pick(self.fixed.previews, saved.previews, auto.previews, max.previews),
            renders: pick(self.fixed.renders, saved.renders, auto.renders, max.renders),
            requests: pick(self.fixed.requests, saved.requests, auto.requests, max.requests),
        }
    }

    /// The CPU threads the pools have.
    pub fn threads_now(&self) -> usize {
        self.threads_now
    }

    /// Keep what an admin set (`None`s: automatic) and use it from now on (the CPU threads from the
    /// next start). A limit the deployment fixed can't be set here.
    pub fn set(&self, v: Values) -> Result<Effective, String> {
        let max = self.machine.max();
        let rows = [
            ("CPU threads", v.threads, self.fixed.threads, max.threads, FLAGS[0]),
            ("Previews at once", v.previews, self.fixed.previews, max.previews, FLAGS[1]),
            ("Renders at once", v.renders, self.fixed.renders, max.renders, FLAGS[2]),
            ("Requests at once", v.requests, self.fixed.requests, max.requests, FLAGS[3]),
        ];
        for (name, value, fixed, max, flag) in rows {
            match value {
                Some(_) if fixed.is_some() => return Err(format!("{name} is set by {flag} when the server starts")),
                Some(n) if !(1..=max).contains(&n) => return Err(format!("{name}: between 1 and {max}")),
                _ => {}
            }
        }
        let Some(file) = &self.file else { return Err("this server has no data folder to keep the setting in".into()) };
        crate::accounts::write_private_json(file, &v)?;
        *lock(&self.saved) = v;
        let e = self.effective();
        self.requests.store(e.requests, Ordering::SeqCst);
        Ok(e)
    }

    /// The admin page's view: per limit, the number in effect, the automatic one, what an admin saved,
    /// what fixed it (if the deployment did) and the most it may be; and the machine.
    pub fn status(&self) -> Value {
        let (e, auto, max, saved) = (self.effective(), self.machine.automatic(), self.machine.max(), *lock(&self.saved));
        let row = |value: usize, auto: usize, saved: Option<usize>, fixed: Option<usize>, max: usize, flag: &str| json!({"value": value, "automatic": auto, "saved": saved, "fixed": fixed.map(|_| flag), "max": max});
        json!({
            "cores": self.machine.cores,
            "memory": self.machine.memory,
            "threadsNow": self.threads_now,
            "threads": row(e.threads, auto.threads, saved.threads, self.fixed.threads, max.threads, FLAGS[0]),
            "previews": row(e.previews, auto.previews, saved.previews, self.fixed.previews, max.previews, FLAGS[1]),
            "renders": row(e.renders, auto.renders, saved.renders, self.fixed.renders, max.renders, FLAGS[2]),
            "requests": row(e.requests, auto.requests, saved.requests, self.fixed.requests, max.requests, FLAGS[3]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-server-limits-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn automatic_limits_leave_cores_free_and_fit_in_memory() {
        let nas = Machine { cores: 4, memory: Some(2 * GB) }.automatic();
        assert_eq!((nas.threads, nas.previews, nas.renders), (3, 1, 1), "a small NAS builds one preview at a time");
        let big = Machine { cores: 16, memory: Some(64 * GB) }.automatic();
        assert_eq!((big.threads, big.previews, big.renders), (12, 4, 2));
        let one = Machine { cores: 1, memory: None }.automatic();
        assert_eq!((one.threads, one.previews, one.renders, one.requests), (1, 1, 1, 64));
    }

    #[test]
    fn fixed_beats_saved_beats_automatic_and_saved_survives_a_restart() {
        let dir = temp("set");
        let m = Machine { cores: 8, memory: Some(16 * GB) };
        let fixed = Values { renders: Some(1), ..Values::default() };
        let l = Limits::new(Some(dir.as_path()), m, fixed);
        assert_eq!(l.effective().previews, 3, "automatic");
        let e = l.set(Values { previews: Some(2), requests: Some(10), ..Values::default() }).unwrap();
        assert_eq!((e.previews, e.requests), (2, 10));
        assert_eq!(l.requests.load(Ordering::SeqCst), 10, "the HTTP server sees it at once");
        assert!(l.set(Values { renders: Some(2), ..Values::default() }).unwrap_err().contains("LIGHTCRAFT_RENDER_THREADS"));
        assert!(l.set(Values { previews: Some(99), ..Values::default() }).unwrap_err().contains("between 1 and 8"));
        assert!(l.set(Values { threads: Some(0), ..Values::default() }).is_err());
        let again = Limits::new(Some(dir.as_path()), m, fixed);
        assert_eq!((again.effective().previews, again.effective().renders), (2, 1));
        assert!(l.set(Values::default()).is_ok());
        assert_eq!(l.effective().previews, 3, "back to automatic");
    }

    #[test]
    fn a_damaged_file_means_automatic() {
        let dir = temp("damaged");
        std::fs::write(dir.join(FILE), b"{not json").unwrap();
        let m = Machine { cores: 4, memory: Some(8 * GB) };
        assert_eq!(Limits::new(Some(dir.as_path()), m, Values::default()).effective(), m.automatic());
    }
}
