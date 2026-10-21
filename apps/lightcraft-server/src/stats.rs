//! What a user's library holds, by kind: photos and videos, the videos' own files, and the lighter copies the server
//! keeps of them. It feeds `GET /api/usage` and the admin pages (Overview, Users, Storage), so every frontend draws
//! numbers the server worked out once.

use std::collections::HashMap;
use std::path::Path;

use lightcraft_catalog::sync::proto::{Files, VideoUsage};
use lightcraft_catalog::{Catalog, MediaKind};

/// A library counted by kind. Photos in Recently Deleted, virtual copies (they share their original's file) and
/// photos only browsed on a disk are not in the library, so they are not counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    pub photos: usize,
    pub videos: usize,
    pub albums: usize,
    /// The videos' files by the sizes the catalog holds (a clip whose size isn't known adds no file).
    pub video_files: Files,
}

/// Count `c` (one pass over its photos; [`crate::api::UserLib::tally`] keeps the answer until the library changes).
pub(crate) fn tally(c: &Catalog) -> Tally {
    let mut t = Tally { albums: c.albums().count(), ..Tally::default() };
    for p in c.photos().filter(|p| !p.deleted && !p.local && p.copy_of.is_none()) {
        if p.kind != MediaKind::Video {
            t.photos += 1;
            continue;
        }
        t.videos += 1;
        if p.file_size > 0 {
            t.video_files = t.video_files.plus(Files { files: 1, bytes: p.file_size });
        }
    }
    t
}

/// The lighter copies of videos under `blobs/video/<xx>/<hash>`: each one's name (the content hash of the video it stands
/// for) and size. Only videos have any, so this reads few directories however big the library is.
pub(crate) fn copies(blobs: &Path) -> Vec<(String, u64)> {
    let Ok(prefixes) = std::fs::read_dir(blobs.join("video")) else { return Vec::new() };
    let mut out = Vec::new();
    for prefix in prefixes.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
        let Ok(files) = std::fs::read_dir(prefix.path()) else { continue };
        for f in files.flatten() {
            if let (Ok(m), Some(name)) = (f.metadata(), f.file_name().to_str())
                && m.is_file()
            {
                out.push((name.to_string(), m.len()));
            }
        }
    }
    out
}

/// What the videos take: the originals from the catalog's sizes, the lighter `copies` the server keeps, and what those
/// saved (a copy that came out larger than its original saves nothing; a copy of a video no longer in the library, not
/// collected yet, saves nothing either).
pub(crate) fn video_usage(c: &Catalog, t: &Tally, copies: &[(String, u64)]) -> VideoUsage {
    let encoded = copies.iter().fold(Files::default(), |all, (_, len)| all.plus(Files { files: 1, bytes: *len }));
    let mut saved = 0u64;
    if !copies.is_empty() {
        let sizes: HashMap<&str, u64> =
            c.photos().filter(|p| p.kind == MediaKind::Video).filter_map(|p| Some((p.content_hash.as_deref()?, p.file_size))).collect();
        for (name, len) in copies {
            if let Some(original) = sizes.get(name.as_str()) {
                saved = saved.saturating_add(original.saturating_sub(*len));
            }
        }
    }
    VideoUsage { originals: t.video_files, encoded, saved }
}

#[cfg(test)]
mod tests {
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};

    use super::*;

    fn photo(id: u64, name: &str, kind: MediaKind, size: u64) -> Photo {
        let mut p = Photo::new(PhotoId(id), Source::Demo { scene: 1 }, name, "JPEG", 60, 40, "2026-01-01T00:00:00");
        (p.kind, p.file_size, p.content_hash) = (kind, size, Some(format!("{id:032x}")));
        p
    }

    fn library() -> Catalog {
        let mut c = Catalog::new();
        for p in [
            photo(1, "a.jpg", MediaKind::Image, 1_000),
            photo(2, "b.dng", MediaKind::Raw, 2_000),
            photo(3, "clip.mp4", MediaKind::Video, 50_000),
            photo(4, "clip2.mov", MediaKind::Video, 30_000),
            photo(5, "unknown.mp4", MediaKind::Video, 0),
        ] {
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        c
    }

    /// Photos and videos are counted apart; the trash, virtual copies and photos only browsed are not in the library.
    #[test]
    fn the_tally_counts_photos_and_videos_apart_and_leaves_out_the_trash_and_copies() {
        let mut c = library();
        let t = tally(&c);
        assert_eq!((t.photos, t.videos), (2, 3), "the raw is a photo");
        assert_eq!(t.video_files, Files { files: 2, bytes: 80_000 }, "a clip of unknown size adds no file");

        // a virtual copy of a clip and of a photo, a clip in Recently Deleted, one only browsed
        let mut copy = photo(6, "clip copy", MediaKind::Video, 50_000);
        copy.copy_of = Some(PhotoId(3));
        let mut copy_of_photo = photo(7, "a copy", MediaKind::Image, 1_000);
        copy_of_photo.copy_of = Some(PhotoId(1));
        let mut gone = photo(8, "gone.mp4", MediaKind::Video, 9_000);
        gone.deleted = true;
        let mut browsed = photo(9, "browsed.mp4", MediaKind::Video, 7_000);
        browsed.local = true;
        for p in [copy, copy_of_photo, gone, browsed] {
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        assert_eq!(tally(&c), t, "none of those is in the library");
        // a hidden photo is still the user's
        let mut hidden = photo(10, "h.jpg", MediaKind::Image, 1);
        hidden.hidden = true;
        c.apply(Op::AddPhoto { photo: Box::new(hidden) }).unwrap();
        assert_eq!(tally(&c).photos, 3);
    }

    /// The lighter copies are read from `blobs/video`, matched to their videos by content hash, and what they saved is
    /// never negative.
    #[test]
    fn the_lighter_copies_are_counted_and_what_they_saved() {
        let c = library();
        let t = tally(&c);
        let dir = std::env::temp_dir().join(format!("lc-server-stats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(copies(&dir).is_empty(), "no video folder, no copies");
        assert_eq!(video_usage(&c, &t, &[]), VideoUsage { originals: t.video_files, encoded: Files::default(), saved: 0 });

        let write = |hash: &str, len: usize| {
            let d = dir.join("video").join(&hash[..2]);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(hash), vec![0u8; len]).unwrap();
        };
        write(&format!("{:032x}", 3), 12_000); // saves 38 000 of clip.mp4's 50 000
        write(&format!("{:032x}", 4), 31_000); // came out larger than its 30 000: saves nothing
        write(&format!("{:032x}", 99), 5_000); // the video it stood for is gone: counted as kept, saves nothing
        let found = copies(&dir);
        assert_eq!(found.len(), 3);
        let u = video_usage(&c, &t, &found);
        assert_eq!(u.encoded, Files { files: 3, bytes: 48_000 });
        assert_eq!((u.originals, u.saved), (Files { files: 2, bytes: 80_000 }, 38_000));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
