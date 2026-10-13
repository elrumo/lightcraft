//! The C functions (`LightCraft/Sources/CLightCraftBridge/include/lightcraft_bridge.h`). The only
//! `unsafe` of the SwiftUI app's Rust side (`CLAUDE.md`, *Never crash*):
//!
//! - one helper, [`input`], makes a byte slice of a caller's `(pointer, length)` after refusing a
//!   null pointer and anything over [`MAX_INPUT`];
//! - the `#[unsafe(no_mangle)]` names the Swift side links against.
//!
//! Engines are handles and buffers are tokens (registry keys, never pointers turned back into
//! Rust values), so nothing the caller passes besides input bytes is dereferenced. Every function
//! runs under `catch_unwind`: a panic comes back as `LC_PANIC` (unwinding into Swift would abort).

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::{Answer, status};

/// The largest request or configuration accepted (a request is a few kilobytes at most).
pub const MAX_INPUT: usize = 16 << 20;

/// JSON bytes owned by the library until `lc_release(token)` (header: `LcBuf`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LcBuf {
    pub ptr: *const u8,
    pub len: usize,
    pub token: u64,
    pub status: i32,
}

/// An `image` event's pixels (header: `LcImage`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LcImage {
    pub pixels: *const u8,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub status: i32,
}

impl From<Answer> for LcBuf {
    fn from(a: Answer) -> Self {
        LcBuf { ptr: a.bytes.ptr, len: a.bytes.len, token: a.bytes.token, status: a.status }
    }
}

/// The caller's `len` bytes at `ptr`, or `LC_BAD_INPUT` for a null pointer or too many bytes.
///
/// # Safety
/// `ptr` must be null or point to `len` initialized bytes that stay valid and unchanged until the
/// returned slice is no longer used (the duration of the C call, as the header says).
unsafe fn input<'a>(ptr: *const u8, len: usize) -> Result<&'a [u8], i32> {
    if ptr.is_null() || len > MAX_INPUT {
        return Err(status::BAD_INPUT);
    }
    // SAFETY: `ptr` is not null and, by this function's contract, points to `len` readable,
    // initialized bytes that outlive the slice; `len` ≤ MAX_INPUT keeps it far below isize::MAX,
    // and `u8` has no alignment requirement.
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// Run `f`, turning a panic into `fallback()`.
fn guarded<T>(fallback: impl FnOnce() -> T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| fallback())
}

fn panicked() -> LcBuf {
    // (a second panic while answering for the first: an empty, failed buffer)
    catch_unwind(|| crate::error(status::PANIC, "panic", "LightCraft hit an internal error; see the log").into()).unwrap_or(LcBuf {
        ptr: std::ptr::null(),
        len: 0,
        token: 0,
        status: status::PANIC,
    })
}

fn bad_input(what: &str) -> LcBuf {
    crate::error(status::BAD_INPUT, "badInput", &format!("{what}: a null pointer or more than {MAX_INPUT} bytes")).into()
}

#[unsafe(no_mangle)]
pub extern "C" fn lc_abi_version() -> u32 {
    crate::ABI_VERSION
}

/// Start an engine from a JSON configuration.
///
/// # Safety
/// `config` must be null or point to `len` readable bytes for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lc_open(config: *const u8, len: usize) -> LcBuf {
    guarded(panicked, || {
        // SAFETY: the caller's contract above is `input`'s.
        match unsafe { input(config, len) } {
            Ok(bytes) => crate::open(bytes).into(),
            Err(_) => bad_input("the configuration"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lc_close(engine: u64) {
    guarded(|| (), || crate::close(engine));
}

/// Queue a JSON request.
///
/// # Safety
/// `request` must be null or point to `len` readable bytes for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lc_submit(engine: u64, request: *const u8, len: usize) -> i32 {
    guarded(
        || status::PANIC,
        || {
            // SAFETY: the caller's contract above is `input`'s.
            match unsafe { input(request, len) } {
                Ok(bytes) => crate::submit(engine, bytes),
                Err(s) => s,
            }
        },
    )
}

/// Run a JSON request and wait for its reply.
///
/// # Safety
/// `request` must be null or point to `len` readable bytes for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lc_call(engine: u64, request: *const u8, len: usize, timeout_ms: u32) -> LcBuf {
    guarded(panicked, || {
        // SAFETY: the caller's contract above is `input`'s.
        match unsafe { input(request, len) } {
            Ok(bytes) => crate::call(engine, bytes, timeout_ms).into(),
            Err(_) => bad_input("the request"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lc_poll(engine: u64, timeout_ms: u32) -> LcBuf {
    guarded(panicked, || crate::poll(engine, timeout_ms).into())
}

#[unsafe(no_mangle)]
pub extern "C" fn lc_wake(engine: u64) {
    guarded(|| (), || crate::wake(engine));
}

#[unsafe(no_mangle)]
pub extern "C" fn lc_image(token: u64) -> LcImage {
    let none = |status| LcImage { pixels: std::ptr::null(), len: 0, width: 0, height: 0, stride: 0, status };
    guarded(
        || none(status::PANIC),
        || match crate::image(token) {
            Ok(i) => LcImage { pixels: i.ptr, len: i.len, width: i.width, height: i.height, stride: i.stride, status: status::OK },
            Err(s) => none(s),
        },
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn lc_release(token: u64) {
    guarded(|| (), || crate::release(token));
}

#[cfg(test)]
mod tests;
