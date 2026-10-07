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
//! This is LightCraft's only crate allowed `unsafe` (two FFI calls into libSystem, in `apple`).
//! Its API is safe, and on failure or on other platforms it releases nothing / knows nothing.
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

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
mod apple {
    unsafe extern "C" {
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

    pub fn release() -> usize {
        // SAFETY: `malloc_zone_pressure_relief` is a documented, thread-safe libSystem function
        // (malloc/malloc.h). A null zone means "every zone" and a goal of 0 means "as much as
        // possible". It only returns already-free pages to the system: it reads and writes no
        // memory we own, takes no pointers we must keep alive, and has no failure mode beyond
        // releasing 0 bytes.
        unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) }
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

    #[test]
    fn concurrent_calls_are_safe() {
        let handles: Vec<_> = (0..4).map(|_| std::thread::spawn(release_free_memory)).collect();
        for h in handles {
            assert!(h.join().is_ok());
        }
    }
}
