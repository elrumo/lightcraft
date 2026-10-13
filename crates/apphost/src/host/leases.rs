//! Buffers the engine hands to a frontend across a C ABI: each is a *lease*, kept here until the
//! frontend releases its token, so a pointer into it stays valid exactly that long.
//!
//! JSON answers ([`Leases::put_bytes`]) are the caller's from the start. Pictures
//! ([`Leases::put_image`]) are offered first: one nobody has taken yet is dropped when a newer one
//! for its slot arrives, or when untaken pictures pass [`MAX_UNTAKEN`] bytes (oldest first). Once
//! taken ([`Leases::take_image`]) a picture stays until released.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_raster::Rgba8;

/// Bytes of pictures offered but not taken that are kept (the frontend has fallen behind).
pub const MAX_UNTAKEN: usize = 256 << 20;

enum Lease {
    /// Kept only so the caller's pointer stays good.
    Bytes {
        _keep: Box<[u8]>,
    },
    Image {
        slot: String,
        image: Arc<Rgba8>,
        taken: bool,
    },
}

#[derive(Default)]
struct Inner {
    next: u64,
    map: HashMap<u64, Lease>,
    untaken_bytes: usize,
}

/// The lease registry (one per engine).
#[derive(Default)]
pub struct Leases {
    inner: Mutex<Inner>,
}

/// A byte buffer handed out: valid until [`Leases::release`] of `token`.
#[derive(Clone, Copy, Debug)]
pub struct BytesRef {
    pub token: u64,
    pub ptr: *const u8,
    pub len: usize,
}

/// A picture handed out: RGBA8 rows of `stride` bytes; valid until [`Leases::release`].
#[derive(Clone, Copy, Debug)]
pub struct ImageRef {
    pub ptr: *const u8,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

fn image_bytes(image: &Rgba8) -> usize {
    image.data.len().saturating_mul(4)
}

impl Leases {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keep `bytes` until `token` is released.
    pub fn put_bytes(&self, bytes: Vec<u8>) -> BytesRef {
        let bytes = bytes.into_boxed_slice();
        let (ptr, len) = (bytes.as_ptr(), bytes.len());
        let mut inner = self.lock();
        inner.next += 1;
        let token = inner.next;
        // (the heap buffer doesn't move when the map does: `ptr` stays good)
        inner.map.insert(token, Lease::Bytes { _keep: bytes });
        BytesRef { token, ptr, len }
    }

    /// Offer `image` for `slot`; returns its token. An untaken older picture of the same slot is
    /// dropped, and the oldest untaken ones beyond [`MAX_UNTAKEN`].
    pub fn put_image(&self, slot: &str, image: Arc<Rgba8>) -> u64 {
        let mut inner = self.lock();
        let stale: Vec<u64> =
            inner.map.iter().filter(|(_, l)| matches!(l, Lease::Image { slot: s, taken: false, .. } if s == slot)).map(|(t, _)| *t).collect();
        for t in stale {
            remove(&mut inner, t);
        }
        inner.next += 1;
        let token = inner.next;
        inner.untaken_bytes = inner.untaken_bytes.saturating_add(image_bytes(&image));
        inner.map.insert(token, Lease::Image { slot: slot.to_string(), image, taken: false });
        while inner.untaken_bytes > MAX_UNTAKEN {
            let oldest =
                inner.map.iter().filter_map(|(t, l)| matches!(l, Lease::Image { taken: false, .. }).then_some(*t)).filter(|t| *t != token).min();
            let Some(oldest) = oldest else { break };
            remove(&mut inner, oldest);
        }
        token
    }

    /// Take the picture of `token`: from now on it stays until released. `None`: no such picture
    /// (released, or dropped before it was taken).
    pub fn take_image(&self, token: u64) -> Option<ImageRef> {
        let mut inner = self.lock();
        let Some(Lease::Image { image, taken, .. }) = inner.map.get_mut(&token) else { return None };
        let first = !*taken;
        *taken = true;
        let r = ImageRef {
            ptr: image.data.as_ptr().cast::<u8>(),
            len: image_bytes(image),
            width: u32::try_from(image.width).unwrap_or(u32::MAX),
            height: u32::try_from(image.height).unwrap_or(u32::MAX),
            stride: u32::try_from(image.width.saturating_mul(4)).unwrap_or(u32::MAX),
        };
        if first {
            let bytes = r.len;
            inner.untaken_bytes = inner.untaken_bytes.saturating_sub(bytes);
        }
        Some(r)
    }

    /// The picture of `token`, shared (for Rust callers: tests, the script runner).
    pub fn image(&self, token: u64) -> Option<Arc<Rgba8>> {
        match self.lock().map.get(&token) {
            Some(Lease::Image { image, .. }) => Some(image.clone()),
            _ => None,
        }
    }

    /// Free `token`'s buffer (an unknown or already released token: nothing).
    pub fn release(&self, token: u64) {
        let mut inner = self.lock();
        remove(&mut inner, token);
    }

    /// Leases held, and bytes of untaken pictures (diagnostics).
    pub fn usage(&self) -> (usize, usize) {
        let inner = self.lock();
        (inner.map.len(), inner.untaken_bytes)
    }
}

fn remove(inner: &mut Inner, token: u64) {
    if let Some(Lease::Image { image, taken: false, .. }) = inner.map.remove(&token) {
        inner.untaken_bytes = inner.untaken_bytes.saturating_sub(image_bytes(&image));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: usize, h: usize, v: u8) -> Arc<Rgba8> {
        Arc::new(Rgba8 { width: w, height: h, data: vec![[v, v, v, 255]; w * h] })
    }

    #[test]
    fn bytes_stay_until_released_and_release_is_idempotent() {
        let l = Leases::default();
        let b = l.put_bytes(b"[1,2]".to_vec());
        assert_eq!(b.len, 5);
        assert_eq!(l.usage().0, 1);
        l.release(b.token);
        l.release(b.token);
        l.release(999);
        assert_eq!(l.usage().0, 0);
    }

    // A newer picture for a slot replaces one nobody took, never one being read
    #[test]
    fn untaken_pictures_are_superseded_and_taken_ones_pinned() {
        let l = Leases::default();
        let a = l.put_image("main", image(2, 2, 1));
        let b = l.put_image("main", image(2, 2, 2));
        assert!(l.take_image(a).is_none(), "superseded before it was taken");
        let taken = l.take_image(b).unwrap();
        assert_eq!((taken.width, taken.height, taken.stride, taken.len), (2, 2, 8, 16));
        let c = l.put_image("main", image(2, 2, 3));
        assert!(l.take_image(b).is_some(), "a taken picture stays until released");
        assert!(l.image(c).is_some());
        assert_eq!(l.usage(), (2, 16));
        l.release(b);
        l.release(c);
        assert_eq!(l.usage(), (0, 0));
    }

    // A frontend that stops taking pictures doesn't grow the process without bound
    #[test]
    fn untaken_pictures_are_capped() {
        let l = Leases::default();
        let side = 1024; // 4 MB each
        let tokens: Vec<u64> = (0..100).map(|i| l.put_image(&format!("thumb:{i}"), image(side, side, 0))).collect();
        assert!(l.usage().1 <= MAX_UNTAKEN);
        assert!(l.take_image(tokens[0]).is_none(), "the oldest went first");
        assert!(l.take_image(tokens[99]).is_some());
    }
}
