//! Give memory the system allocator keeps after frees back to the operating system.
//!
//! macOS's allocator keeps freed large blocks mapped as "reusable": they no longer count in the
//! process's memory footprint but stay in its resident size until the system runs short. After a
//! raw decode (hundreds of MB of temporaries) that made the resident size climb towards the sum of
//! everything ever decoded. `malloc_zone_pressure_relief` (libSystem) returns those pages now.
//! Elsewhere (glibc, Windows) blocks this large are unmapped when freed. On glibc the smaller blocks
//! that fill its arenas are not: a long-running server's resident size stays at its peak, so
//! [`release_free_memory`] calls `malloc_trim` there.
//!
//! On iOS the system ends an app that goes over its memory limit, which depends on the device and
//! is far below its RAM: [`available_memory`] says how much is left (`os_proc_available_memory`),
//! so the caches can be sized to fit.
//!
//! macOS and iOS 27's allocator keep freed large blocks in a cache instead, dirty and counted in the
//! footprint, and `malloc_zone_pressure_relief` releases none of it: memory freed after every decode
//! and render piled up (a 12 MP ProRAW opened: 250 MB in use, 1 GB of footprint), and iOS ends an app
//! by its footprint. [`ReturningAlloc`], the apps' global allocator, gives large blocks pages of their
//! own that go back to the system when they are freed, except a few kept for the next allocation of
//! the same size (a slider drag's pictures), which [`release_free_memory`] gives back.
//!
//! [`fsync`] is the plain POSIX `fsync`, which std doesn't offer on macOS: there `File::sync_all`
//! and `sync_data` are `F_FULLFSYNC`, which SMB shares refuse with ENOTSUP while `fsync` works
//! (issue #1407).
//!
//! [`lower_thread_priority`] gives a background thread (the sync server's video encoder) the lowest priority:
//! `setpriority` on Linux, the background quality of service on macOS.
//!
//! This is LightCraft's only crate allowed `unsafe` (FFI calls into libSystem, in `apple`: the ones
//! above, and `mmap` / `munmap` for the allocator). Its API is safe, and on failure or on other
//! platforms it releases nothing / knows nothing / allocates as the system does.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Return free allocator pages to the system, the large blocks [`ReturningAlloc`] keeps for reuse
/// among them. Returns the number of bytes released (0 where the platform doesn't say, or where freed
/// large blocks are already unmapped). Safe to call at any time, from any thread.
pub fn release_free_memory() -> usize {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        returning::release() + apple::release()
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        glibc::trim()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", all(target_os = "linux", target_env = "gnu"))))]
    {
        0
    }
}

/// Flush `file` to the storage device with a plain `fsync`. On macOS that is the libSystem call
/// (std's `sync_all` is `F_FULLFSYNC`, which some volumes refuse); elsewhere it is `sync_all`.
pub fn fsync(file: &std::fs::File) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        apple::fsync(file)
    }
    #[cfg(not(target_os = "macos"))]
    {
        file.sync_all()
    }
}

/// Give the calling thread the lowest scheduling priority, for work that should only use what everything else leaves
/// (a server encoding videos for its library in the background): nice 19 on Linux, the background quality of service
/// on macOS. Only this thread: a thread it starts later is not lowered. Returns whether the system took it (`false`
/// elsewhere, or when it refused).
pub fn lower_thread_priority() -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        priority::lower()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

/// Bytes this process can still allocate before iOS ends it for using too much memory, or `None`
/// where the system sets no such limit (desktops) or doesn't report it (the iOS simulator says 0).
/// Safe to call at any time, from any thread.
pub fn available_memory() -> Option<usize> {
    #[cfg(target_os = "ios")]
    {
        Some(apple::available()).filter(|n| *n > 0)
    }
    #[cfg(not(target_os = "ios"))]
    {
        None
    }
}

/// The global allocator of LightCraft's apps on Apple platforms (elsewhere it is the system's):
/// blocks of [`LARGE`] bytes or more are mapped for themselves and unmapped when freed, so freed
/// image planes leave the footprint at once; smaller ones are the system allocator's. Large
/// blocks come zeroed from the kernel (a zeroed allocation costs no writes), and growing one
/// copies it, as the system's large blocks do.
pub struct ReturningAlloc;

/// Bytes from which a block gets pages of its own.
pub const LARGE: usize = 1 << 20;

/// Whether [`ReturningAlloc`] keeps a few freed large blocks for the next allocation of the same size (on by
/// default; off, every large block is unmapped when freed). For A/B measurements.
pub fn set_block_reuse(on: bool) {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        returning::set_reuse(on);
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = on;
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
mod returning {
    use super::{LARGE, ReturningAlloc};
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicUsize, Ordering};

    /// Mapped pages are aligned to at least this (4 KiB on Intel, 16 KiB on Apple silicon).
    const PAGE: usize = 4096;

    fn own_pages(layout: Layout) -> bool {
        layout.size() >= LARGE && layout.align() <= PAGE
    }

    // Freed large blocks kept for the next allocation of exactly their size. A slider drag allocates the
    // same buffers for every picture (the GPU readback, the picture handed to the frontend, a CPU render's
    // planes), and fresh pages fault in as they are first written: filling a new 15 MB picture took 2.5 ms
    // on an M1 Max against 0.56 ms for a block written before. A few blocks and bytes at most, so the
    // footprint stays what the blocks in use need; [`release`] (idle trims, memory warnings, after decodes)
    // unmaps them.
    const KEPT_BLOCKS: usize = 8;
    /// The largest block kept: a picture or a plane, not a decode's buffers.
    const KEPT_BLOCK_MAX: usize = 64 << 20;
    #[cfg(target_os = "ios")]
    const KEPT_MAX: usize = 64 << 20;
    #[cfg(not(target_os = "ios"))]
    const KEPT_MAX: usize = 256 << 20;
    const EMPTY: u8 = 0;
    const BUSY: u8 = 1;
    const FULL: u8 = 2;

    struct Kept {
        state: AtomicU8,
        ptr: AtomicPtr<u8>,
        len: AtomicUsize,
    }

    #[allow(clippy::declare_interior_mutable_const)]
    const NONE: Kept = Kept { state: AtomicU8::new(EMPTY), ptr: AtomicPtr::new(std::ptr::null_mut()), len: AtomicUsize::new(0) };
    static KEPT: [Kept; KEPT_BLOCKS] = [NONE; KEPT_BLOCKS];
    static KEPT_BYTES: AtomicUsize = AtomicUsize::new(0);
    static REUSE: AtomicBool = AtomicBool::new(true);

    pub fn set_reuse(on: bool) {
        REUSE.store(on, Ordering::Relaxed);
        if !on {
            release();
        }
    }

    /// A kept block of exactly `len` bytes, or null.
    fn take(len: usize) -> *mut u8 {
        if KEPT_BYTES.load(Ordering::Relaxed) == 0 {
            return std::ptr::null_mut();
        }
        for k in &KEPT {
            // (`len` and `ptr` only change while the slot is BUSY, which its owner alone sets)
            if k.state.load(Ordering::Acquire) == FULL
                && k.len.load(Ordering::Relaxed) == len
                && k.state.compare_exchange(FULL, BUSY, Ordering::Acquire, Ordering::Relaxed).is_ok()
            {
                let p = k.ptr.swap(std::ptr::null_mut(), Ordering::Relaxed);
                KEPT_BYTES.fetch_sub(len, Ordering::Relaxed);
                k.state.store(EMPTY, Ordering::Release);
                return p;
            }
        }
        std::ptr::null_mut()
    }

    /// Keep the freed block `ptr` of `len` bytes for a later [`take`]; false when there is no room.
    fn keep(ptr: *mut u8, len: usize) -> bool {
        if len > KEPT_BLOCK_MAX || !REUSE.load(Ordering::Relaxed) {
            return false;
        }
        if KEPT_BYTES.fetch_add(len, Ordering::Relaxed).saturating_add(len) > KEPT_MAX {
            KEPT_BYTES.fetch_sub(len, Ordering::Relaxed);
            return false;
        }
        for k in &KEPT {
            if k.state.compare_exchange(EMPTY, BUSY, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                k.len.store(len, Ordering::Relaxed);
                k.ptr.store(ptr, Ordering::Relaxed);
                k.state.store(FULL, Ordering::Release);
                return true;
            }
        }
        KEPT_BYTES.fetch_sub(len, Ordering::Relaxed);
        false
    }

    /// Unmap every kept block; the bytes given back.
    pub fn release() -> usize {
        let mut freed = 0;
        for k in &KEPT {
            if k.state.compare_exchange(FULL, BUSY, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                let (p, len) = (k.ptr.swap(std::ptr::null_mut(), Ordering::Relaxed), k.len.load(Ordering::Relaxed));
                KEPT_BYTES.fetch_sub(len, Ordering::Relaxed);
                k.state.store(EMPTY, Ordering::Release);
                crate::apple::unmap(p, len);
                freed += len;
            }
        }
        freed
    }

    // SAFETY: every block is either the system allocator's or a mapping of its own, and which one
    // is decided by `own_pages` of its layout alone, which `dealloc` and `realloc` are given exactly
    // as it was allocated (GlobalAlloc's contract): a block always goes back where it came from.
    // Mappings are page-aligned, which covers every alignment `own_pages` accepts. A kept block is a
    // mapping of exactly the size asked for, no longer used by anyone (it was freed), and handed to
    // one caller only (the slot's state is claimed with a compare-exchange).
    unsafe impl GlobalAlloc for ReturningAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if own_pages(layout) {
                let kept = take(layout.size());
                if kept.is_null() { crate::apple::map(layout.size()) } else { kept }
            } else {
                // SAFETY: the caller's layout, passed on (GlobalAlloc's contract)
                unsafe { System.alloc(layout) }
            }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if own_pages(layout) {
                let kept = take(layout.size());
                if kept.is_null() {
                    // (fresh anonymous pages are zero)
                    return crate::apple::map(layout.size());
                }
                // SAFETY: a kept block is `layout.size()` writable bytes nobody else holds
                unsafe { std::ptr::write_bytes(kept, 0, layout.size()) };
                kept
            } else {
                // SAFETY: as in `alloc`
                unsafe { System.alloc_zeroed(layout) }
            }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            if own_pages(layout) {
                if !keep(ptr, layout.size()) {
                    crate::apple::unmap(ptr, layout.size());
                }
            } else {
                // SAFETY: `ptr` came from `System` with this layout (`own_pages` was false then too)
                unsafe { System.dealloc(ptr, layout) }
            }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // SAFETY: GlobalAlloc's contract: `new_size`, rounded up to `layout.align()`, does not overflow
            let new = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
            if !own_pages(layout) && !own_pages(new) {
                // SAFETY: a system block, staying one
                return unsafe { System.realloc(ptr, layout, new_size) };
            }
            // SAFETY: allocated with `new`, a valid layout (see above)
            let moved = unsafe { self.alloc(new) };
            if !moved.is_null() {
                // SAFETY: both blocks are live and distinct, and at least `min` bytes long
                unsafe { std::ptr::copy_nonoverlapping(ptr, moved, layout.size().min(new_size)) };
                // SAFETY: `ptr` was allocated with `layout` and is not used again
                unsafe { self.dealloc(ptr, layout) };
            }
            moved
        }
    }
}

/// glibc keeps the free pages inside its arenas; `malloc_trim` hands them back.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
mod glibc {
    use std::ffi::c_int;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// It walks every arena under its lock: not more often than this (a decode ends several times a second).
    const EVERY: Duration = Duration::from_secs(2);
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);

    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> c_int;
    }

    pub(super) fn trim() -> usize {
        {
            let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if last.is_some_and(|t| t.elapsed() < EVERY) {
                return 0;
            }
            *last = Some(Instant::now());
        }
        // SAFETY: `malloc_trim` takes no pointers and is safe to call from any thread at any time.
        unsafe { malloc_trim(0) };
        0
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
#[allow(unsafe_code)]
mod returning {
    use super::ReturningAlloc;
    use std::alloc::{GlobalAlloc, Layout, System};

    // SAFETY: the system allocator, every call passed on unchanged.
    unsafe impl GlobalAlloc for ReturningAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: passed on (GlobalAlloc's contract)
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            // SAFETY: passed on
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: passed on
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // SAFETY: passed on
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
mod apple {
    use std::ffi::{c_int, c_void};
    #[cfg(target_os = "macos")]
    use std::os::fd::AsRawFd;

    const PROT_READ: c_int = 1;
    const PROT_WRITE: c_int = 2;
    const MAP_PRIVATE: c_int = 0x0002;
    const MAP_ANON: c_int = 0x1000;

    unsafe extern "C" {
        /// libSystem (`sys/mman.h`): map `len` bytes of fresh, zeroed, page-aligned memory.
        fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, offset: i64) -> *mut c_void;
        /// libSystem (`sys/mman.h`): give the pages of `[addr, addr + len)` back to the system.
        fn munmap(addr: *mut c_void, len: usize) -> c_int;
        /// libSystem (unistd.h): flush the open file `fd` to the device. 0, or -1 and `errno`.
        #[cfg(target_os = "macos")]
        #[link_name = "fsync"]
        fn c_fsync(fd: c_int) -> c_int;
        /// libSystem malloc: release free memory of `zone` (all zones when null) back to the
        /// system, up to `goal` bytes (0 = as much as possible). Returns the bytes released.
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        /// libSystem (`os/proc.h`, iOS 13+): bytes left before the process reaches its memory
        /// limit; 0 when it has none or can't tell.
        #[cfg(target_os = "ios")]
        fn os_proc_available_memory() -> usize;
    }

    #[cfg(target_os = "ios")]
    pub fn available() -> usize {
        // SAFETY: `os_proc_available_memory` is a documented, thread-safe libSystem function
        // (os/proc.h, iOS 13+; the app targets iOS 16) that takes no arguments, touches no memory
        // of ours and returns a plain integer (0 when there is no limit or it is unknown).
        unsafe { os_proc_available_memory() }
    }

    /// `len` bytes of pages of their own, or null when the system has none to give.
    pub fn map(len: usize) -> *mut u8 {
        // SAFETY: an anonymous private mapping anywhere: it touches no existing memory, and the
        // kernel either returns a fresh page-aligned range or MAP_FAILED (-1), checked here.
        let p = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0) };
        if p as isize == -1 { std::ptr::null_mut() } else { p.cast() }
    }

    /// Unmap a block `map(len)` returned.
    pub fn unmap(ptr: *mut u8, len: usize) {
        // SAFETY: `ptr` is a mapping of `len` bytes made by `map` and no longer used (the
        // allocator's caller freed it); unmapping it removes only those pages. A failure leaves the
        // pages mapped (a leak, never a fault), so the result is not needed.
        let _ = unsafe { munmap(ptr.cast(), len) };
    }

    pub fn release() -> usize {
        // SAFETY: `malloc_zone_pressure_relief` is a documented, thread-safe libSystem function
        // (malloc/malloc.h). A null zone means "every zone" and a goal of 0 means "as much as
        // possible". It only returns already-free pages to the system: it reads and writes no
        // memory we own, takes no pointers we must keep alive, and has no failure mode beyond
        // releasing 0 bytes.
        unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) }
    }

    #[cfg(target_os = "macos")]
    pub fn fsync(file: &std::fs::File) -> std::io::Result<()> {
        loop {
            // SAFETY: `fsync` (libSystem, unistd.h) takes a file descriptor by value and touches no
            // memory of ours. The descriptor is open for the whole call: `file` is borrowed.
            if unsafe { c_fsync(file.as_raw_fd()) } == 0 {
                return Ok(());
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(unsafe_code)]
mod priority {
    use std::ffi::c_int;

    #[cfg(target_os = "linux")]
    unsafe extern "C" {
        /// libc (`sys/resource.h`): the nice value of a process, or on Linux of the thread `who` names (0: the caller).
        fn setpriority(which: c_int, who: u32, prio: c_int) -> c_int;
    }

    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        /// libSystem (`pthread/qos.h`): the quality of service of the calling thread.
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: c_int) -> c_int;
    }

    pub(super) fn lower() -> bool {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: `setpriority` takes three integers and touches no memory of ours; `PRIO_PROCESS` (0) with
            // `who` 0 means the calling thread on Linux.
            unsafe { setpriority(0, 0, 19) == 0 }
        }
        #[cfg(target_os = "macos")]
        {
            // `QOS_CLASS_BACKGROUND`
            const BACKGROUND: u32 = 0x09;
            // SAFETY: `pthread_set_qos_class_self_np` takes two integers and touches no memory of ours; it
            // changes only the calling thread.
            unsafe { pthread_set_qos_class_self_np(BACKGROUND, 0) == 0 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::release_free_memory;

    /// A thread can lower its own priority (and nothing else's), and doing it twice is harmless.
    #[test]
    fn a_thread_lowers_its_own_priority() {
        let took = std::thread::spawn(|| super::lower_thread_priority() && super::lower_thread_priority()).join().unwrap();
        assert_eq!(took, cfg!(any(target_os = "linux", target_os = "macos")));
    }

    /// Held by the tests that give kept blocks back or switch reuse: the kept blocks are the process's.
    static KEPT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn releasing_with_nothing_freed_is_harmless() {
        let _serial = serial();
        // Twice in a row: the second call usually has nothing left to release.
        let _ = release_free_memory();
        let _ = release_free_memory();
    }

    #[test]
    fn freed_large_blocks_can_be_released_and_memory_stays_usable() {
        let _serial = serial();
        // Allocate and free well over the allocator's large-block threshold, then release.
        let blocks: Vec<Vec<u8>> = (0..8).map(|i| vec![i as u8; 16 << 20]).collect();
        let sum: u64 = blocks.iter().map(|b| u64::from(b[b.len() - 1])).sum();
        assert_eq!(sum, (0..8u64).sum());
        drop(blocks);
        let released = release_free_memory();
        if cfg!(not(any(target_os = "macos", target_os = "ios"))) {
            assert_eq!(released, 0);
        }
        // The allocator must still work normally afterwards.
        let again = vec![7u8; 16 << 20];
        assert_eq!(again[again.len() - 1], 7);
    }

    #[test]
    fn available_memory_is_known_only_where_the_system_limits_it() {
        let a = super::available_memory();
        if cfg!(target_os = "ios") {
            assert!(a.is_none_or(|n| n > 0));
        } else {
            assert_eq!(a, None);
        }
    }

    // Large blocks round-trip through the allocator: zeroed when asked, contents kept when they
    // grow and shrink across the large-block threshold, alignments honoured
    #[test]
    #[allow(unsafe_code)]
    fn large_blocks_keep_their_contents_and_alignment() {
        let _serial = serial();
        use super::{LARGE, ReturningAlloc};
        use std::alloc::{GlobalAlloc, Layout};
        let a = ReturningAlloc;
        let big = Layout::from_size_align(LARGE * 3 + 5, 64).unwrap();
        // SAFETY: valid layouts; each block is freed once with the layout it has at that moment
        unsafe {
            let p = a.alloc_zeroed(big);
            assert!(!p.is_null() && p.align_offset(64) == 0);
            let s = std::slice::from_raw_parts_mut(p, big.size());
            assert!(s.iter().all(|&b| b == 0));
            for (i, b) in s.iter_mut().enumerate() {
                *b = (i % 251) as u8;
            }
            // grows (a large block again), then shrinks below the threshold (a system block)
            let q = a.realloc(p, big, LARGE * 5);
            assert!(!q.is_null());
            assert!(std::slice::from_raw_parts(q, big.size()).iter().enumerate().all(|(i, &b)| b == (i % 251) as u8));
            let grown = Layout::from_size_align(LARGE * 5, 64).unwrap();
            let r = a.realloc(q, grown, 1000);
            assert!(!r.is_null());
            assert!(std::slice::from_raw_parts(r, 1000).iter().enumerate().all(|(i, &b)| b == (i % 251) as u8));
            // and back up from a system block to a large one
            let small = Layout::from_size_align(1000, 64).unwrap();
            let t = a.realloc(r, small, LARGE * 2);
            assert!(!t.is_null() && std::slice::from_raw_parts(t, 1000).iter().enumerate().all(|(i, &b)| b == (i % 251) as u8));
            a.dealloc(t, Layout::from_size_align(LARGE * 2, 64).unwrap());
            // an alignment above a page goes to the system allocator, and still holds
            let wide = Layout::from_size_align(LARGE * 2, 1 << 16).unwrap();
            let w = a.alloc(wide);
            assert!(!w.is_null() && w.align_offset(1 << 16) == 0);
            a.dealloc(w, wide);
        }
        // and as a Vec would use it, from many threads at once
        let handles: Vec<_> = (0..4)
            .map(|t| {
                std::thread::spawn(move || {
                    for i in 0..16usize {
                        let layout = Layout::from_size_align(LARGE + i * 4097, 8).unwrap();
                        // SAFETY: as above
                        unsafe {
                            let p = ReturningAlloc.alloc(layout);
                            assert!(!p.is_null());
                            p.write(t as u8);
                            p.add(layout.size() - 1).write(i as u8);
                            assert_eq!((p.read(), p.add(layout.size() - 1).read()), (t as u8, i as u8));
                            ReturningAlloc.dealloc(p, layout);
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            assert!(h.join().is_ok());
        }
    }

    // A freed large block comes back for the next block of its size (no fresh pages to fault in),
    // zeroed when asked, and release gives the kept ones back
    #[test]
    #[allow(unsafe_code)]
    fn freed_large_blocks_are_reused_for_the_same_size_and_released() {
        let _serial = serial();
        use super::{LARGE, ReturningAlloc};
        use std::alloc::{GlobalAlloc, Layout};
        // (a size no other test asks for: the kept blocks are the whole process's)
        let layout = Layout::from_size_align(LARGE * 7 + 4099, 16).unwrap();
        let _ = release_free_memory();
        // SAFETY: valid layouts; each block is freed once, with the layout it was allocated with
        unsafe {
            let p = ReturningAlloc.alloc(layout);
            assert!(!p.is_null());
            std::ptr::write_bytes(p, 9, layout.size());
            ReturningAlloc.dealloc(p, layout);
            // (the same pages, still written: fresh ones would be zero)
            let q = ReturningAlloc.alloc(layout);
            assert!(!q.is_null());
            if cfg!(any(target_os = "macos", target_os = "ios")) {
                assert!(
                    p == q && std::slice::from_raw_parts(q, layout.size()).iter().all(|&b| b == 9),
                    "the freed block is the next one of its size"
                );
            }
            ReturningAlloc.dealloc(q, layout);
            let q = ReturningAlloc.alloc_zeroed(layout);
            assert!(!q.is_null());
            assert!(std::slice::from_raw_parts(q, layout.size()).iter().all(|&b| b == 0), "zeroed when asked");
            ReturningAlloc.dealloc(q, layout);
            // another size does not take it
            let other = Layout::from_size_align(LARGE * 7 + 8195, 16).unwrap();
            let r = ReturningAlloc.alloc(other);
            assert!(!r.is_null() && r != q);
            ReturningAlloc.dealloc(r, other);
        }
        if cfg!(any(target_os = "macos", target_os = "ios")) {
            assert!(super::release_free_memory() >= layout.size(), "the kept blocks are given back");
        }
        // with reuse off nothing is kept, and the allocator still works
        super::set_block_reuse(false);
        let v = vec![3u8; LARGE * 2];
        assert_eq!(v[v.len() - 1], 3);
        drop(v);
        super::set_block_reuse(true);
    }

    #[test]
    fn fsync_flushes_an_open_file_and_reports_errors() {
        use std::io::Write;
        let p = std::env::temp_dir().join(format!("lc-sysmem-fsync-{}", std::process::id()));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"data").unwrap();
        super::fsync(&f).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"data");
        let _ = std::fs::remove_file(&p);
        // a directory opened read-only can be synced too; what fails is an error, not a panic
        if let Ok(d) = std::fs::File::open(std::env::temp_dir()) {
            let _ = super::fsync(&d);
        }
    }

    #[test]
    fn concurrent_calls_are_safe() {
        let _serial = serial();
        let handles: Vec<_> = (0..4).map(|_| std::thread::spawn(release_free_memory)).collect();
        for h in handles {
            assert!(h.join().is_ok());
        }
    }
}
