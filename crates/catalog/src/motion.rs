//! Live Photo clips that came into the library as videos of their own: a phone's export (or a folder of them) holds
//! `IMG_0001.HEIC` and `IMG_0001.MOV`, and the second is the first's motion, not a video to browse. The catalog
//! pairs them by what they are called and when they were taken, and views leave the clip out
//! ([`Filter::motion_clips`] shows them).
//!
//! The pairs are derived, never saved: they are worked out from the photos' names, kinds, durations and capture times
//! on first use and again after any op that is not develop-only ([`Catalog::library_revision`]), so a clip or a still
//! that arrives later by sync pairs when both are here, and renaming either unpairs them.
//!
//! A clip pairs with a still when it is short ([`MAX_SECONDS`]), has the same name apart from the extension (any case),
//! was taken within [`MAX_GAP`] seconds of it (a missing capture time on either side counts as a match), and is the
//! only clip and the only still of that name that fit. Anything less certain stays what it is: a video.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::dates::iso_seconds;
use crate::{Catalog, MediaKind, Photo, PhotoId};

/// The longest clip taken as a Live Photo's motion (a phone's is about 3 s).
pub const MAX_SECONDS: f64 = 4.5;
/// Most seconds between the still's capture time and the clip's.
pub const MAX_GAP: i64 = 5;

/// Which clips are motion of which stills.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MotionPairs {
    still_of: HashMap<PhotoId, PhotoId>,
    clip_of: HashMap<PhotoId, PhotoId>,
}

impl MotionPairs {
    /// The still `clip` is the motion of.
    pub fn still_of(&self, clip: PhotoId) -> Option<PhotoId> {
        self.still_of.get(&clip).copied()
    }

    /// The clip that is `still`'s motion.
    pub fn clip_of(&self, still: PhotoId) -> Option<PhotoId> {
        self.clip_of.get(&still).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.still_of.is_empty()
    }

    pub fn len(&self) -> usize {
        self.still_of.len()
    }
}

/// The cache of the pairs, with the library revision they were worked out at.
#[derive(Default)]
pub(crate) struct MotionCache(Mutex<Option<(u64, Arc<MotionPairs>)>>);

/// A copy works them out again when asked.
impl Clone for MotionCache {
    fn clone(&self) -> Self {
        MotionCache::default()
    }
}

impl PartialEq for MotionCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for MotionCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MotionCache")
    }
}

/// `name` without its extension, lower case: what a still and its clip share.
fn stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cut = base.rfind('.').filter(|i| *i > 0).unwrap_or(base.len());
    base.get(..cut).unwrap_or(base).to_lowercase()
}

/// Whether two capture times are close enough to be one moment (unknown times don't rule a pair out).
fn near(a: &Option<String>, b: &Option<String>) -> bool {
    match (a.as_deref().and_then(iso_seconds), b.as_deref().and_then(iso_seconds)) {
        (Some(x), Some(y)) => x.abs_diff(y) <= MAX_GAP.unsigned_abs(),
        _ => true,
    }
}

/// The pairs among `photos` (those not deleted, not only browsed; hidden ones pair with hidden ones only).
pub fn pair<'a>(photos: impl Iterator<Item = &'a Arc<Photo>>) -> MotionPairs {
    let mut clips: HashMap<String, Vec<&Photo>> = HashMap::new();
    let mut stills: Vec<&Photo> = Vec::new();
    for p in photos.map(|p| p.as_ref()).filter(|p| p.in_library()) {
        if p.kind == MediaKind::Video {
            if p.duration.is_some_and(|d| d.is_finite() && d > 0.0 && d <= MAX_SECONDS) {
                clips.entry(stem(&p.file_name)).or_default().push(p);
            }
        } else {
            stills.push(p);
        }
    }
    let mut out = MotionPairs::default();
    if clips.is_empty() {
        return out;
    }
    // the stills that fit each clip's name: a pair needs exactly one each way
    let mut fits: HashMap<PhotoId, Vec<&Photo>> = HashMap::new();
    let mut wanted: HashMap<PhotoId, Vec<&Photo>> = HashMap::new();
    for still in stills {
        let Some(group) = clips.get(&stem(&still.file_name)) else { continue };
        for clip in group.iter().filter(|c| c.hidden == still.hidden && near(&c.captured, &still.captured)) {
            fits.entry(clip.id).or_default().push(still);
            wanted.entry(still.id).or_default().push(clip);
        }
    }
    for (clip, candidates) in &fits {
        let [still] = candidates.as_slice() else { continue };
        if wanted.get(&still.id).is_some_and(|c| c.len() == 1) {
            out.still_of.insert(*clip, still.id);
            out.clip_of.insert(still.id, *clip);
        }
    }
    out
}

impl Catalog {
    /// The Live Photo pairs of this catalog (worked out again after any op that is not develop-only).
    pub fn motion_pairs(&self) -> Arc<MotionPairs> {
        let mut cache = self.motion.0.lock().unwrap_or_else(PoisonError::into_inner);
        match cache.as_ref() {
            Some((revision, pairs)) if *revision == self.library_revision => Arc::clone(pairs),
            _ => {
                let pairs = Arc::new(pair(self.photos()));
                *cache = Some((self.library_revision, Arc::clone(&pairs)));
                pairs
            }
        }
    }

    /// `photo` is the motion clip of a still (views leave it out).
    pub fn is_motion_clip(&self, photo: PhotoId) -> bool {
        // (most libraries have none: a cheap answer for the usual query)
        self.motion_pairs().still_of(photo).is_some()
    }
}
