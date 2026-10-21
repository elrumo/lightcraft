//! What the server takes of the machine, and what each user's devices ask of the server: the admin page's Overview
//! (`GET /api/admin/usage`). Kept in memory since the server started (a restart starts the counts again), with a sample
//! of the machine every [`EVERY`] for the last 24 hours.
//!
//! The machine numbers are read from `/proc` and, in a container, its cgroup files, so they exist on Linux (and Docker)
//! only; elsewhere they are `None` and the page says so. Per-device counts are made everywhere: [`Usage::note`] is called
//! for every answered request that signed in with a device's token.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Time between samples.
pub const EVERY: Duration = Duration::from_secs(30);

/// Samples kept: 24 hours.
const KEPT: usize = 2880;

/// The page gets at most this many points (neighbours are averaged).
const POINTS: usize = 240;

/// Clock ticks a second in `/proc/<pid>/stat` (`USER_HZ`: 100 on every Linux).
const TICKS: f64 = 100.0;

/// What one device asked of the server since it started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    pub requests: u64,
    /// Bytes of request bodies it sent, and of answers (as declared: a streamed one without a length counts 0).
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// Time the server spent answering (until the answer is handed over: sending it isn't counted).
    pub busy_ms: u64,
}

/// The machine at one moment. `cpu` and `machine_cpu` need the sample before to compare with.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// Unix seconds.
    pub at: u64,
    /// The server's CPU use in cores (1.5: a core and a half), and the whole machine's, 0 to 1.
    pub cpu: Option<f64>,
    pub machine_cpu: Option<f64>,
    /// The server's resident memory, and what the machine (or the container) uses, in bytes.
    pub rss: Option<u64>,
    pub mem_used: Option<u64>,
    /// Requests of devices and bytes sent to them since the sample before.
    pub requests: u64,
    pub out: u64,
}

/// CPU counters as last read.
#[derive(Clone, Copy)]
struct Raw {
    when: Instant,
    process: Option<u64>,
    machine: Option<(u64, u64)>,
}

#[derive(Default)]
struct Inner {
    devices: HashMap<(String, u64), Activity>,
    history: VecDeque<Sample>,
    last: Option<Raw>,
    /// Requests and bytes sent counted at the last sample.
    seen: (u64, u64),
}

pub struct Usage {
    /// Unix seconds the counts began.
    pub since: u64,
    inner: Mutex<Inner>,
    stop: AtomicBool,
}

impl Usage {
    pub fn new() -> Usage {
        Usage { since: crate::accounts::now(), inner: Mutex::new(Inner::default()), stop: AtomicBool::new(false) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A device's request was answered.
    pub fn note(&self, user: &str, device: u64, bytes_in: u64, bytes_out: u64, busy: Duration) {
        let mut g = self.lock();
        let a = g.devices.entry((user.to_string(), device)).or_default();
        a.requests = a.requests.saturating_add(1);
        a.bytes_in = a.bytes_in.saturating_add(bytes_in);
        a.bytes_out = a.bytes_out.saturating_add(bytes_out);
        a.busy_ms = a.busy_ms.saturating_add(u64::try_from(busy.as_millis()).unwrap_or(u64::MAX));
    }

    /// What a device has asked since the server started.
    pub fn of(&self, user: &str, device: u64) -> Activity {
        self.lock().devices.get(&(user.to_string(), device)).copied().unwrap_or_default()
    }

    /// Read the machine now and add it to the history.
    pub fn sample(&self) {
        let r = read();
        let now = Instant::now();
        let mut g = self.lock();
        let requests: u64 = g.devices.values().map(|a| a.requests).sum();
        let out: u64 = g.devices.values().map(|a| a.bytes_out).sum();
        let (cpu, machine_cpu) = match g.last {
            Some(prev) => {
                let secs = now.duration_since(prev.when).as_secs_f64();
                let cpu = r.process.zip(prev.process).filter(|_| secs > 0.0).map(|(a, b)| a.saturating_sub(b) as f64 / TICKS / secs);
                let machine = r.machine.zip(prev.machine).and_then(|((busy, total), (busy0, total0))| {
                    let dt = total.saturating_sub(total0);
                    (dt > 0).then(|| busy.saturating_sub(busy0) as f64 / dt as f64)
                });
                (cpu, machine)
            }
            None => (None, None),
        };
        g.last = Some(Raw { when: now, process: r.process, machine: r.machine });
        let s = Sample {
            at: crate::accounts::now(),
            cpu,
            machine_cpu,
            rss: r.rss,
            mem_used: r.mem_used,
            requests: requests.saturating_sub(g.seen.0),
            out: out.saturating_sub(g.seen.1),
        };
        g.seen = (requests, out);
        if g.history.len() >= KEPT {
            g.history.pop_front();
        }
        g.history.push_back(s);
    }

    /// The history, as at most [`POINTS`] points.
    pub fn history(&self) -> Vec<Sample> {
        let all: Vec<Sample> = self.lock().history.iter().copied().collect();
        all.chunks(all.len().div_ceil(POINTS).max(1)).map(average).collect()
    }

    /// Stop [`Usage::run`].
    pub fn shut_down(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Sample every [`EVERY`] until [`Usage::shut_down`]. (The starter takes the first sample, so the history is never
    /// empty and the second has one to compare with.)
    pub fn run(&self) {
        loop {
            for _ in 0..EVERY.as_millis() / 250 {
                if self.stop.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            self.sample();
            // (decodes give memory back as they end, at most every few seconds: this catches the last one's)
            lightcraft_engine::memory::release();
        }
    }
}

impl Default for Usage {
    fn default() -> Usage {
        Usage::new()
    }
}

/// Neighbouring samples as one: the readings averaged, the counts added.
fn average(c: &[Sample]) -> Sample {
    let mean = |f: fn(&Sample) -> Option<f64>| {
        let v: Vec<f64> = c.iter().filter_map(f).collect();
        (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
    };
    Sample {
        at: c.last().map_or(0, |s| s.at),
        cpu: mean(|s| s.cpu),
        machine_cpu: mean(|s| s.machine_cpu),
        rss: mean(|s| s.rss.map(|x| x as f64)).map(|x| x as u64),
        mem_used: mean(|s| s.mem_used.map(|x| x as f64)).map(|x| x as u64),
        requests: c.iter().map(|s| s.requests).sum(),
        out: c.iter().map(|s| s.out).sum(),
    }
}

/// What the machine says about itself.
struct Reading {
    /// This process's CPU time (clock ticks), the machine's busy and total ticks.
    process: Option<u64>,
    machine: Option<(u64, u64)>,
    rss: Option<u64>,
    mem_used: Option<u64>,
}

#[cfg(target_os = "linux")]
fn read() -> Reading {
    let file = |p: &str| std::fs::read_to_string(p).ok();
    Reading {
        process: file("/proc/self/stat").and_then(|s| process_ticks(&s)),
        machine: file("/proc/stat").and_then(|s| machine_ticks(&s)),
        rss: file("/proc/self/status").and_then(|s| rss(&s)),
        mem_used: mem_used(&file),
    }
}

#[cfg(not(target_os = "linux"))]
fn read() -> Reading {
    Reading { process: None, machine: None, rss: None, mem_used: None }
}

/// `utime + stime` of `/proc/<pid>/stat`: fields 14 and 15, counted after the command name (in parentheses, and it may
/// hold spaces and parentheses), which leaves field 3 first.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn process_ticks(stat: &str) -> Option<u64> {
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    Some(f.get(11)?.parse::<u64>().ok()?.saturating_add(f.get(12)?.parse::<u64>().ok()?))
}

/// The machine's (busy, total) ticks from the first line of `/proc/stat`: user nice system idle iowait irq softirq steal
/// (guest time is in user's already); busy is all but idle and iowait.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn machine_ticks(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().next().filter(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    let total: u64 = v.iter().take(8).sum();
    let idle = v.get(3)?.saturating_add(v.get(4).copied().unwrap_or(0));
    Some((total.saturating_sub(idle), total))
}

/// `VmRSS` of `/proc/self/status`, in bytes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rss(status: &str) -> Option<u64> {
    let kb: u64 = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?.split_whitespace().next()?.parse().ok()?;
    Some(kb.saturating_mul(1024))
}

/// Memory in use, in bytes: a container with a memory limit holds what its cgroup says, less the file cache the system
/// takes back when it needs room; otherwise the machine's total less what is available.
#[cfg(target_os = "linux")]
fn mem_used(file: &dyn Fn(&str) -> Option<String>) -> Option<u64> {
    let number = |s: Option<String>| s.and_then(|s| s.trim().parse::<u64>().ok());
    if number(file("/sys/fs/cgroup/memory.max")).is_some()
        && let Some(current) = number(file("/sys/fs/cgroup/memory.current"))
    {
        let cache = file("/sys/fs/cgroup/memory.stat").and_then(|s| stat_value(&s, "inactive_file")).unwrap_or(0);
        return Some(current.saturating_sub(cache));
    }
    let meminfo = file("/proc/meminfo")?;
    let kb = |key: &str| meminfo.lines().find_map(|l| l.strip_prefix(key)?.split_whitespace().next()?.parse::<u64>().ok());
    Some(kb("MemTotal:")?.saturating_sub(kb("MemAvailable:")?).saturating_mul(1024))
}

/// `key value` lines of a cgroup `memory.stat`.
#[cfg(target_os = "linux")]
fn stat_value(stat: &str, key: &str) -> Option<u64> {
    stat.lines().find_map(|l| l.strip_prefix(key)?.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cpu_time_of_a_process_is_read_past_a_command_name_with_spaces_and_brackets() {
        let stat = "4242 (my (odd) name) S 1 4242 4242 0 -1 4194560 100 0 0 0 150 25 0 0 20 0 3 0 1000 1 2 3";
        assert_eq!(process_ticks(stat), Some(175));
        assert_eq!(process_ticks("garbage"), None);
        assert_eq!(process_ticks("1 (x) S 1 2"), None, "too short");
    }

    #[test]
    fn the_machines_busy_and_total_ticks_leave_out_idle_and_iowait() {
        let stat = "cpu  100 10 50 800 40 5 5 0 7 0\ncpu0 1 1 1 1 1 1 1 1 1 1\n";
        assert_eq!(machine_ticks(stat), Some((170, 1010)));
        assert_eq!(machine_ticks("cpu0 1 2 3"), None);
        assert_eq!(machine_ticks(""), None);
    }

    #[test]
    fn resident_memory_is_read_in_bytes() {
        assert_eq!(rss("Name:\tx\nVmRSS:\t  2048 kB\nThreads:\t4\n"), Some(2048 * 1024));
        assert_eq!(rss("Name:\tx\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn memory_in_use_follows_a_container_limit_else_the_machine() {
        let meminfo = "MemTotal:       16000 kB\nMemFree:  1000 kB\nMemAvailable:    6000 kB\n";
        let free = |p: &str| (p == "/proc/meminfo").then(|| meminfo.to_string());
        assert_eq!(mem_used(&free), Some(10_000 * 1024));
        // (no limit: the cgroup says `max`)
        let unlimited = |p: &str| match p {
            "/sys/fs/cgroup/memory.max" => Some("max\n".to_string()),
            "/sys/fs/cgroup/memory.current" => Some("999\n".to_string()),
            _ => free(p),
        };
        assert_eq!(mem_used(&unlimited), Some(10_000 * 1024));
        let limited = |p: &str| match p {
            "/sys/fs/cgroup/memory.max" => Some("1073741824\n".to_string()),
            "/sys/fs/cgroup/memory.current" => Some("500000\n".to_string()),
            "/sys/fs/cgroup/memory.stat" => Some("anon 1\ninactive_file 200000\nactive_file 5\n".to_string()),
            _ => free(p),
        };
        assert_eq!(mem_used(&limited), Some(300_000));
    }

    #[test]
    fn what_devices_ask_is_counted_per_device_and_sampled() {
        let u = Usage::new();
        u.note("ann", 1, 100, 1000, Duration::from_millis(30));
        u.note("ann", 1, 0, 500, Duration::from_millis(20));
        u.note("ann", 2, 5, 5, Duration::ZERO);
        u.note("bob", 1, 1, 1, Duration::ZERO);
        assert_eq!(u.of("ann", 1), Activity { requests: 2, bytes_in: 100, bytes_out: 1500, busy_ms: 50 });
        assert_eq!(u.of("ann", 3), Activity::default(), "never heard from");
        u.sample();
        u.note("ann", 1, 0, 10, Duration::ZERO);
        u.sample();
        let h = u.history();
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].requests, h[0].out), (4, 1506), "everything so far");
        assert_eq!((h[1].requests, h[1].out), (1, 10), "what came since");
    }

    #[test]
    fn a_long_history_is_averaged_into_a_few_points() {
        let u = Usage::new();
        {
            let mut g = u.lock();
            for i in 0..KEPT as u64 {
                g.history.push_back(Sample {
                    at: i,
                    cpu: Some(1.0),
                    machine_cpu: (i % 2 == 0).then_some(0.5),
                    rss: Some(100),
                    requests: 1,
                    ..Default::default()
                });
            }
        }
        let h = u.history();
        assert!(h.len() <= POINTS && h.len() > POINTS / 2, "{}", h.len());
        assert_eq!(h.iter().map(|s| s.requests).sum::<u64>(), KEPT as u64, "counts are added, none lost");
        assert!(
            h.iter().all(|s| s.cpu == Some(1.0) && s.rss == Some(100) && s.machine_cpu == Some(0.5)),
            "readings are averaged over the samples that have them"
        );
        assert_eq!(h.last().map(|s| s.at), Some(KEPT as u64 - 1));
        assert!(Usage::new().history().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_machine_can_be_read() {
        let u = Usage::new();
        u.sample();
        std::thread::sleep(Duration::from_millis(50));
        u.sample();
        let h = u.history();
        assert!(h[0].rss.is_some_and(|b| b > 0), "{h:?}");
        assert!(h[0].mem_used.is_some_and(|b| b > 0), "{h:?}");
        assert!(h[0].cpu.is_none(), "nothing to compare the first with");
        assert!(h.last().and_then(|s| s.cpu).is_some_and(|c| c >= 0.0), "{h:?}");
    }
}
