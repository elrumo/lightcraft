//! What a user's library holds: photos and albums. It feeds `GET /api/usage` and the admin pages (Overview, Users,
//! Storage), so every frontend draws numbers the server worked out once.

use lightcraft_catalog::Catalog;

/// A library counted. Photos in Recently Deleted, virtual copies (they share their original's file) and photos only
/// browsed on a disk are not in the library, so they are not counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    pub photos: usize,
    pub albums: usize,
}

/// Count `c` (one pass over its photos; [`crate::api::UserLib::tally`] keeps the answer until the library changes).
pub(crate) fn tally(c: &Catalog) -> Tally {
    Tally { photos: c.photos().filter(|p| !p.deleted && !p.local && p.copy_of.is_none()).count(), albums: c.albums().count() }
}

#[cfg(test)]
mod tests {
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};

    use super::*;

    fn photo(id: u64, name: &str) -> Photo {
        Photo::new(PhotoId(id), Source::Demo { scene: 1 }, name, "JPEG", 60, 40, "2026-01-01T00:00:00")
    }

    /// The trash, virtual copies and photos only browsed are not in the library.
    #[test]
    fn the_tally_leaves_out_the_trash_copies_and_browsed_photos() {
        let mut c = Catalog::new();
        for p in [photo(1, "a.jpg"), photo(2, "b.jpg")] {
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        let t = tally(&c);
        assert_eq!(t.photos, 2);

        // a virtual copy, a photo in Recently Deleted, one only browsed
        let mut copy = photo(3, "a copy");
        copy.copy_of = Some(PhotoId(1));
        let mut gone = photo(4, "gone.jpg");
        gone.deleted = true;
        let mut browsed = photo(5, "browsed.jpg");
        browsed.local = true;
        for p in [copy, gone, browsed] {
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        assert_eq!(tally(&c), t, "none of those is in the library");
    }
}
