//! Pictures a frontend hands an export in memory while the export runs: `app.export`'s `hostPixels` entries that name a `memory`
//! key instead of a file ([`super::PixelSource::Memory`]). The frontend's renderer draws them one after another as the batch goes
//! on, and each photo's export waits for its own picture and encodes it as soon as it comes ([`take`]). So the first files are
//! written while the later pictures are still drawn, and no picture goes through a file (a full-size RGBA picture is 4 to 8 bytes a
//! pixel: 1.4 GB written and read again for a batch of 20 raws, all of it before the first file).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use web_time::Instant;

/// What a frontend says of a picture it promised an export.
#[derive(Debug, PartialEq, Eq)]
pub enum HostPicture {
    /// The pixels, laid out as a `hostPixels` file holds them, in groups of 4 bytes (an 8 bit pixel, half of a 16 bit one): an
    /// 8 bit picture is the export's image as it is, with no copy (card #514).
    Pixels(Vec<[u8; 4]>),
    /// It does not draw this photo after all: the export renders it itself.
    Render,
    /// The photo cannot be exported as asked (its error, said to the person).
    Fail(String),
}

struct Slots {
    pictures: BTreeMap<String, HostPicture>,
    /// Bytes of pixels handed over and not taken yet.
    bytes: usize,
}

static SLOTS: Mutex<Slots> = Mutex::new(Slots { pictures: BTreeMap::new(), bytes: 0 });
static CHANGED: Condvar = Condvar::new();

fn slots() -> MutexGuard<'static, Slots> {
    SLOTS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn size(p: &HostPicture) -> usize {
    match p {
        HostPicture::Pixels(b) => b.len().saturating_mul(4),
        _ => 0,
    }
}

/// Hand over what became of the picture promised under `key` (one that came before it under the same key is dropped).
/// Answers the bytes of pixels handed over and not taken yet, every export's together: a frontend draws ahead only so far.
pub fn put(key: &str, picture: HostPicture) -> usize {
    let mut s = slots();
    s.bytes = s.bytes.saturating_add(size(&picture));
    if let Some(old) = s.pictures.insert(key.to_string(), picture) {
        s.bytes = s.bytes.saturating_sub(size(&old));
    }
    let held = s.bytes;
    drop(s);
    CHANGED.notify_all();
    held
}

/// The bytes of pixels handed over and not taken yet.
pub fn held() -> usize {
    slots().bytes
}

/// Drop every picture whose key starts with `prefix` that no export took (the export they were for has ended).
pub fn forget(prefix: &str) {
    let mut s = slots();
    let keys: Vec<String> = s.pictures.range(prefix.to_string()..).map(|(k, _)| k).take_while(|k| k.starts_with(prefix)).cloned().collect();
    for k in keys {
        if let Some(old) = s.pictures.remove(&k) {
            s.bytes = s.bytes.saturating_sub(size(&old));
        }
    }
}

/// Whether something was said of the picture under `key` (pixels, render it, or an error), so taking it does not wait.
pub(super) fn has(key: &str) -> bool {
    slots().pictures.contains_key(key)
}

/// Whether the frontend said it does not draw the picture under `key` after all (the export renders the photo).
pub(super) fn is_render(key: &str) -> bool {
    matches!(slots().pictures.get(key), Some(HostPicture::Render))
}

/// The picture under `key`, waiting for it up to `wait`; `None` when it did not come by then, or `cancel` was set meanwhile.
pub(super) fn take(key: &str, cancel: Option<&AtomicBool>, wait: Duration) -> Option<HostPicture> {
    let mut s = slots();
    let mut until = None;
    loop {
        if let Some(p) = s.pictures.remove(key) {
            s.bytes = s.bytes.saturating_sub(size(&p));
            return Some(p);
        }
        // (a browser has no threads: nothing can come while this waits)
        if cfg!(target_arch = "wasm32") || cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return None;
        }
        let now = Instant::now();
        let end = *until.get_or_insert_with(|| now.checked_add(wait).unwrap_or(now));
        if now >= end {
            return None;
        }
        // (the cancel flag is set without a notify: looked at again at least this often)
        let step = (end - now).min(Duration::from_millis(100));
        s = CHANGED.wait_timeout(s, step).unwrap_or_else(PoisonError::into_inner).0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // A picture handed over later is waited for and taken once; the bytes held go with it; a cancel or the wait's end gives up;
    // forget drops what an export left, and only its own
    #[test]
    fn a_picture_is_waited_for_taken_once_and_forgotten_with_its_export() {
        let key = format!("hp-test-{}/1", std::process::id());
        let late = {
            let key = key.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                put(&key, HostPicture::Pixels(vec![[7; 4]; 4]))
            })
        };
        assert!(!has(&key));
        let got = take(&key, None, Duration::from_secs(30));
        assert!(late.join().unwrap() >= 16, "the bytes held are answered");
        assert_eq!(got, Some(HostPicture::Pixels(vec![[7; 4]; 4])));
        assert_eq!(take(&key, None, Duration::ZERO), None, "taken once");
        put(&key, HostPicture::Render);
        assert!(has(&key) && is_render(&key), "a picture said of is there to take");
        assert_eq!(take(&key, None, Duration::ZERO), Some(HostPicture::Render));

        let cancel = Arc::new(AtomicBool::new(true));
        let t = Instant::now();
        assert_eq!(take(&format!("{key}-never"), Some(&cancel), Duration::from_secs(30)), None);
        assert!(t.elapsed() < Duration::from_secs(5), "a cancel stops the wait");

        let mine = format!("hp-test-{}-forget/", std::process::id());
        let other = format!("hp-test-{}-kept/", std::process::id());
        put(&format!("{mine}1"), HostPicture::Pixels(vec![[0; 4]; 2]));
        put(&format!("{mine}2"), HostPicture::Render);
        put(&format!("{other}1"), HostPicture::Fail("no".into()));
        forget(&mine);
        assert_eq!(take(&format!("{mine}1"), None, Duration::ZERO), None);
        assert_eq!(take(&format!("{mine}2"), None, Duration::ZERO), None);
        assert_eq!(take(&format!("{other}1"), None, Duration::ZERO), Some(HostPicture::Fail("no".into())));
    }
}
