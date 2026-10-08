//! Memory accounting: what each cache holds (`library.memory`, and `ui.inspect` → `memory` with the
//! frontend's own caches added).
//!
//! The numbers are the caches' own bookkeeping (pixels held), not the process's resident size: the
//! allocator keeps freed pages for reuse and the GPU driver maps device buffers, so `ps`/`time -l`
//! report more. A binary built with a heap profiler installs [`set_heap_stats`] (e.g.
//! `lightcraft-cli` with `--features dhat-heap`) to add live/peak heap bytes.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

use serde::Serialize;

/// The memory budget: one number from which every cache's share is derived.
///
/// - half for the engine's caches (decoded sources and rendered previews), evicted least recently
///   used *across* them ([`crate::media::MediaCache`]);
/// - a quarter for the working memory of decodes in flight ([`work_gate`]);
/// - an eighth for the GPU renderer's pool of recycled buffers (trimmed when the app is idle).
///
/// The rest is headroom for what isn't cached (renders in progress, textures, the UI). Default:
/// a quarter of the machine's RAM, at most 1.5 GiB (`LIGHTCRAFT_MEMORY_MB` overrides it); on iOS
/// see [`budget_for_limit`].
pub fn default_budget() -> usize {
    if let Some(mb) = std::env::var("LIGHTCRAFT_MEMORY_MB").ok().and_then(|v| v.trim().parse::<usize>().ok()).filter(|m| *m >= 64) {
        return mb << 20;
    }
    if cfg!(target_os = "ios") {
        // the host passes the real limit to `set_budget(budget_for_limit(..))` at launch
        return budget_for_limit(None);
    }
    let cap = 3usize << 29; // 1.5 GiB
    match total_ram() {
        Some(ram) => (ram / 4).clamp(256 << 20, cap),
        None => cap,
    }
}

/// The budget on iOS, where the system ends an app that goes over its memory limit (a fraction
/// of the device's RAM that depends on the model) instead of swapping: a third of what the app
/// may still allocate at launch (`available`, from `lightcraft_sysmem::available_memory`),
/// between 256 MiB and 1 GiB, leaving two thirds for decodes and renders in progress, textures
/// and the UI. Unknown (the simulator): 768 MiB.
pub fn budget_for_limit(available: Option<usize>) -> usize {
    const MIN: usize = 256 << 20;
    const MAX: usize = 1 << 30;
    available.filter(|a| *a > 0).map_or(768 << 20, |a| a / 3).clamp(MIN, MAX)
}

/// What an export needs at its peak: the source as floats (12 bytes per pixel) while the develop pipeline's stage
/// buffers, the output image and the encoder take about 48 bytes per output pixel. (Measured on a 12 MP and a 48 MP
/// ProRAW; deliberately a little high.)
pub fn export_need(source_px: usize, output_px: usize) -> usize {
    source_px.saturating_mul(12).saturating_add(output_px.saturating_mul(48))
}

/// What an export may take on this device, where going over ends the app: on iOS the system ends an app that
/// exceeds its limit, so two budgets (the budget is a third of what the app may allocate at launch); elsewhere memory
/// swaps instead, so there is no limit unless `LIGHTCRAFT_EXPORT_MEMORY_MB` sets one. A render over it is not tried
/// here: it goes to the sync server, or fails with a message ([`crate::export`]).
pub fn export_limit() -> Option<usize> {
    if let Some(mb) = std::env::var("LIGHTCRAFT_EXPORT_MEMORY_MB").ok().and_then(|v| v.trim().parse::<usize>().ok()).filter(|m| *m > 0) {
        return Some(mb << 20);
    }
    cfg!(target_os = "ios").then(|| budget().saturating_mul(2))
}

impl crate::Session {
    /// What an export from this session may take: what its host set ([`crate::Session::set_export_limit`]), else the
    /// platform's ([`export_limit`]).
    pub fn export_limit(&self) -> Option<usize> {
        self.export_limit.unwrap_or_else(export_limit)
    }

    /// Say what an export from this session may take (`None`: no limit). A host that knows the app's real limit (the
    /// iOS app) says so here; the render a sync server makes for a device has none of its own.
    pub fn set_export_limit(&mut self, bytes: Option<usize>) {
        self.export_limit = Some(bytes);
    }
}

/// Physical memory (bytes), when the platform tells us without native calls.
fn total_ram() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: usize = s.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(target_os = "freebsd")]
    {
        let out = std::process::Command::new("/sbin/sysctl").args(["-n", "hw.physmem"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
    {
        None
    }
}

static BUDGET: AtomicUsize = AtomicUsize::new(0);

/// The current budget (bytes).
pub fn budget() -> usize {
    match BUDGET.load(Ordering::Relaxed) {
        0 => {
            let b = default_budget();
            BUDGET.store(b, Ordering::Relaxed);
            apply(b);
            b
        }
        b => b,
    }
}

/// Change the budget (bytes, at least 64 MiB) for the process: the decode gate and the GPU pool
/// follow at once; sessions apply their cache share with [`crate::Session::set_memory_budget`].
pub fn set_budget(bytes: usize) -> usize {
    let b = bytes.max(64 << 20);
    BUDGET.store(b, Ordering::Relaxed);
    apply(b);
    b
}

fn apply(b: usize) {
    if let Some(g) = GATE.get() {
        g.set_limit(b / 4);
    }
    lightcraft_gpu::set_pool_limit((b / 8) as u64);
    // an eighth of the budget for the JPEG XL tiles (ProRAW, Adobe's JPEG XL DNGs) decoded at once, ~32 MB each
    lightcraft_raw::set_max_parallel_tiles((b / 8 / (32 << 20)).max(2));
}

/// Share of the budget for the engine's caches.
pub fn cache_share(budget: usize) -> usize {
    budget / 2
}

/// A weighted semaphore over bytes of working memory: [`WorkGate::acquire`] waits while other
/// holders already use the limit (one holder may always proceed, whatever its size).
pub struct WorkGate {
    state: Mutex<(usize, usize)>,
    cv: Condvar,
}

/// Bytes held from a [`WorkGate`] until dropped.
pub struct Permit<'a> {
    gate: &'a WorkGate,
    bytes: usize,
}

impl WorkGate {
    pub fn new(limit: usize) -> WorkGate {
        WorkGate { state: Mutex::new((0, limit)), cv: Condvar::new() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (usize, usize)> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_limit(&self, limit: usize) {
        self.lock().1 = limit;
        self.cv.notify_all();
    }

    /// Bytes held now and the limit.
    pub fn usage(&self) -> (usize, usize) {
        *self.lock()
    }

    /// Hold `bytes`, waiting until they fit (or nothing else is held).
    pub fn acquire(&self, bytes: usize) -> Permit<'_> {
        let mut g = self.lock();
        while g.0 > 0 && g.0 + bytes > g.1 {
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        g.0 += bytes;
        Permit { gate: self, bytes }
    }

    /// Hold `bytes` without waiting (interactive work): counted, so background work waits for it.
    pub fn acquire_urgent(&self, bytes: usize) -> Permit<'_> {
        self.lock().0 += bytes;
        Permit { gate: self, bytes }
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut g = self.gate.lock();
        g.0 = g.0.saturating_sub(self.bytes);
        drop(g);
        self.gate.cv.notify_all();
    }
}

thread_local! {
    static BACKGROUND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` as background work (grid thumbnails, neighbour prefetch): its decodes wait at the
/// [`work_gate`] while interactive work holds the memory.
pub fn in_background<R>(f: impl FnOnce() -> R) -> R {
    let was = BACKGROUND.with(|b| b.replace(true));
    let r = f();
    BACKGROUND.with(|b| b.set(was));
    r
}

/// Is this thread running background work ([`in_background`])?
pub fn is_background() -> bool {
    BACKGROUND.with(|b| b.get())
}

/// The process-wide gate for decodes and import probes (limit: a quarter of the budget).
pub fn work_gate() -> &'static WorkGate {
    GATE.get_or_init(|| WorkGate::new(budget() / 4))
}

static GATE: OnceLock<WorkGate> = OnceLock::new();

/// Entries and bytes held by one cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub count: usize,
    pub bytes: usize,
}

impl Usage {
    pub fn new(count: usize, bytes: usize) -> Usage {
        Usage { count, bytes }
    }
}

/// Heap bytes as counted by an instrumented allocator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeapUsage {
    /// Live heap bytes now.
    pub current: u64,
    /// Highest live heap bytes so far.
    pub peak: u64,
}

/// Device buffers of the GPU renderer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuUsage {
    /// Every device buffer that exists.
    pub allocated: u64,
    /// Of which recycled buffers waiting in the free pool.
    pub pooled: u64,
    /// Of which buffers released since their thread's last submit.
    pub retired: u64,
}

/// What the engine's caches hold.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReport {
    /// Decoded thumbnail-level sources (≤ 512 px, linear float).
    pub thumb_sources: Usage,
    /// Decoded preview-level sources (≤ 2560 px, linear float) of the current and prefetched photos.
    pub preview_sources: Usage,
    /// The last full-resolution original (exports, 1:1).
    pub full_source: Usage,
    /// Rendered thumbnails and view renders (8-bit) in memory.
    pub rendered: Usage,
    /// Sum of the above.
    pub engine_bytes: usize,
    pub gpu: GpuUsage,
    /// The memory budget (see [`budget`]) and the engine caches' share of it.
    pub budget: usize,
    pub cache_budget: usize,
    /// Working memory of decodes / probes in flight and its limit ([`work_gate`]).
    pub work_bytes: usize,
    pub work_limit: usize,
    /// Live/peak heap when the binary counts allocations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heap: Option<HeapUsage>,
}

static HEAP: OnceLock<fn() -> HeapUsage> = OnceLock::new();
static RELEASE: OnceLock<fn()> = OnceLock::new();

/// Install a hook that hands memory the allocator keeps after frees back to the system (first
/// call wins). The macOS allocator keeps freed large blocks resident (reusable, but counted in
/// the resident size) until the system runs short; the apps install
/// `malloc_zone_pressure_relief` so the resident size follows what is actually held.
pub fn set_release_hook(f: fn()) {
    let _ = RELEASE.set(f);
}

/// Return freed memory to the system now (after large temporaries were dropped: a decode, an
/// import). No-op without a hook.
pub fn release() {
    if let Some(f) = RELEASE.get() {
        f();
    }
}

/// Install the heap counter of an instrumented binary (first call wins).
pub fn set_heap_stats(f: fn() -> HeapUsage) {
    let _ = HEAP.set(f);
}

/// Live/peak heap bytes, when the binary installed a counter.
pub fn heap_stats() -> Option<HeapUsage> {
    HEAP.get().map(|f| f())
}

/// The GPU renderer's device buffers.
pub fn gpu_usage() -> GpuUsage {
    let g = lightcraft_gpu::memory();
    GpuUsage { allocated: g.allocated, pooled: g.pooled, retired: g.retired }
}

impl crate::Session {
    /// What the engine's caches hold now.
    pub fn memory_report(&self) -> MemoryReport {
        let (thumb_sources, preview_sources, full_source) = self.media.usage();
        let (n, b) = self.media.rendered.mem_usage();
        let rendered = Usage::new(n, b);
        MemoryReport {
            thumb_sources,
            preview_sources,
            full_source,
            rendered,
            engine_bytes: thumb_sources.bytes + preview_sources.bytes + full_source.bytes + rendered.bytes,
            gpu: gpu_usage(),
            budget: budget(),
            cache_budget: self.media.budget(),
            work_bytes: work_gate().usage().0,
            work_limit: work_gate().usage().1,
            heap: heap_stats(),
        }
    }

    /// Set the memory budget (bytes; process-wide) and apply this session's cache share.
    pub fn set_memory_budget(&mut self, bytes: usize) -> usize {
        let b = set_budget(bytes);
        self.media.set_budget(cache_share(b));
        b
    }

    /// The system is short of memory (an iOS memory warning; the app is ended if it doesn't give
    /// some back): forget the decoded sources (decoded again when next shown) and the GPU's
    /// pooled buffers, and hand freed pages back to the system. Returns the bytes freed from the
    /// caches. Rendered previews stay: they are small and redrawing the grid without them is slow.
    pub fn release_memory(&mut self) -> usize {
        let held = self.memory_report().engine_bytes;
        self.media.clear_sources();
        lightcraft_gpu::trim_pool(0);
        release();
        held.saturating_sub(self.memory_report().engine_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_needs_follow_the_pixels() {
        let (mp12, mp48) = (12_192_768usize, 48_771_072usize);
        let gib = 1usize << 30;
        // two iOS budgets of 1 GiB: a 12 MP export fits; a 48 MP one at full size does not, but a 48 MP photo exported
        // at half its size (read at 12 MP, written at 24 MP) does
        assert!(export_need(mp12, mp12) < 2 * gib, "{}", export_need(mp12, mp12));
        assert!(export_need(mp48, mp48) > 2 * gib, "{}", export_need(mp48, mp48));
        assert!(export_need(mp12, mp48 / 2) < 2 * gib, "{}", export_need(mp12, mp48 / 2));
        // never overflows
        assert_eq!(export_need(usize::MAX, usize::MAX), usize::MAX);
    }

    #[test]
    fn the_ios_budget_follows_the_app_memory_limit() {
        // a 4 GB limit (a recent iPhone Pro): 1 GiB at most
        assert_eq!(budget_for_limit(Some(4_000_000_000)), 1 << 30);
        // a 1.5 GB limit: a third
        assert_eq!(budget_for_limit(Some(1_500_000_000)), 500_000_000);
        // nearly nothing left: still enough for a preview
        assert_eq!(budget_for_limit(Some(100 << 20)), 256 << 20);
        // unknown (simulator) or none
        assert_eq!(budget_for_limit(None), 768 << 20);
        assert_eq!(budget_for_limit(Some(0)), 768 << 20);
    }

    /// A memory warning drops the decoded sources; photos still render afterwards.
    #[test]
    fn releasing_memory_drops_decoded_sources_and_keeps_working() {
        let mut s = crate::Session::with_demo();
        let ids: Vec<_> = s.catalog.photos().take(3).map(|p| p.id).collect();
        for id in &ids {
            s.render_now(*id, 256, 256).unwrap();
        }
        let before = s.memory_report();
        let sources = before.thumb_sources.bytes + before.preview_sources.bytes + before.full_source.bytes;
        assert!(sources > 0, "{before:?}");
        let freed = s.release_memory();
        assert!(freed >= sources, "freed {freed} of {sources}");
        let after = s.memory_report();
        assert_eq!(after.thumb_sources.bytes + after.preview_sources.bytes + after.full_source.bytes, 0, "{after:?}");
        assert!(s.render_now(ids[0], 256, 256).is_ok());
    }
}
