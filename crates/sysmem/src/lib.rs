//! Give memory the system allocator keeps after frees back to the operating system.
//!
//! macOS's allocator keeps freed large blocks mapped as "reusable": they no longer count in the
//! process's memory footprint but stay in its resident size until the system runs short. After a
//! raw decode (hundreds of MB of temporaries) that made the resident size climb towards the sum of
//! everything ever decoded. `malloc_zone_pressure_relief` (libSystem) returns those pages now.
//! Elsewhere (glibc, Windows) blocks this large are unmapped when freed: nothing to do.
//!
//! On iOS the system ends an app that goes over its memory limit, which depends on the device and
//! is far below its RAM: [`available_memory`] says how much is left (`os_proc_available_memory`),
//! so the caches can be sized to fit.
//!
//! macOS and iOS 27's allocator keep freed large blocks in a cache instead, dirty and counted in the
//! footprint, and `malloc_zone_pressure_relief` releases none of it: memory freed after every decode
//! and render piled up (a 12 MP ProRAW opened: 250 MB in use, 1 GB of footprint), and iOS ends an app
//! by its footprint. [`ReturningAlloc`], the apps' global allocator, gives large blocks pages of their
//! own that go back to the system the moment they are freed.
//!
//! [`fsync`] is the plain POSIX `fsync`, which std doesn't offer on macOS: there `File::sync_all`
//! and `sync_data` are `F_FULLFSYNC`, which SMB shares refuse with ENOTSUP while `fsync` works
//! (issue #1407).
//!
//! This is LightCraft's only crate allowed `unsafe` (FFI calls into libSystem, in `apple`: the ones
//! above, and `mmap` / `munmap` for the allocator). Its API is safe, and on failure or on other
//! platforms it releases nothing / knows nothing / allocates as the system does.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Return free allocator pages to the system. Returns the number of bytes released (always 0 on
/// platforms where freed large blocks are already unmapped). Safe to call at any time, from any
/// thread.
pub fn release_free_memory() -> usize {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        apple::release()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
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

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
mod returning {
    use super::{LARGE, ReturningAlloc};
    use std::alloc::{GlobalAlloc, Layout, System};

    /// Mapped pages are aligned to at least this (4 KiB on Intel, 16 KiB on Apple silicon).
    const PAGE: usize = 4096;

    fn own_pages(layout: Layout) -> bool {
        layout.size() >= LARGE && layout.align() <= PAGE
    }

    // SAFETY: every block is either the system allocator's or a mapping of its own, and which one
    // is decided by `own_pages` of its layout alone, which `dealloc` and `realloc` are given exactly
    // as it was allocated (GlobalAlloc's contract): a block always goes back where it came from.
    // Mappings are page-aligned, which covers every alignment `own_pages` accepts.
    unsafe impl GlobalAlloc for ReturningAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if own_pages(layout) {
                crate::apple::map(layout.size())
            } else {
                // SAFETY: the caller's layout, passed on (GlobalAlloc's contract)
                unsafe { System.alloc(layout) }
            }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if own_pages(layout) {
                // (fresh anonymous pages are zero)
                crate::apple::map(layout.size())
            } else {
                // SAFETY: as in `alloc`
                unsafe { System.alloc_zeroed(layout) }
            }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            if own_pages(layout) {
                crate::apple::unmap(ptr, layout.size());
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

#[cfg(test)]
mod tests {
    use super::release_free_memory;

    #[test]
    fn releasing_with_nothing_freed_is_harmless() {
        // Twice in a row: the second call usually has nothing left to release.
        let _ = release_free_memory();
        let _ = release_free_memory();
    }

    #[test]
    fn freed_large_blocks_can_be_released_and_memory_stays_usable() {
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
        let handles: Vec<_> = (0..4).map(|_| std::thread::spawn(release_free_memory)).collect();
        for h in handles {
            assert!(h.join().is_ok());
        }
    }
}
