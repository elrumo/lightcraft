//! Photos that visitors add to a shared album: an album link made with **Let people add photos**
//! ([`proto::NewShare::upload`](lightcraft_catalog::sync::proto::NewShare)) shows a form on its page, and what it
//! sends (`POST /s/<token>/upload`, `multipart/form-data`) becomes the owner's photos, in that album, on every
//! one of their devices — the way a shared album of a cloud photo service collects everyone's pictures of a trip.
//!
//! Each file is streamed to the owner's store as it arrives ([`crate::multipart`]), hashed on the way, read for
//! what it is ([`lightcraft_engine::files::probe_bytes`], under a panic guard: a visitor's file is untrusted), and
//! kept by its content like a device's upload: `blobs/original/<hash>`, or the owner's imports folder if they
//! have one ([`crate::writeback`]). The server adds the photos to the library in its own id space, puts them in
//! the album, and builds their previews (then search indexes them, as any photo). A file the library has already
//! is the same photo, put in the album; a file that isn't a photo or video the server reads is left out, and
//! the page says how many were.
//!
//! Limits: [`FILE_MAX`] a file and [`FORM_MAX`] a form, [`FILES_MAX`] files a form, and the owner's storage limit
//! ([`crate::accounts::User::quota`]): an upload that would go over it is refused before a byte is kept. A visitor's
//! name, if they give one, becomes the photo's Creator when the file doesn't name one.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError};

use lightcraft_catalog::sync::original_path;
use lightcraft_catalog::{AlbumId, MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_engine::sidecar;

use crate::State;
use crate::accounts;
use crate::api::{blob_path, header_value, lib};
use crate::http::Request;
use crate::multipart::{Multipart, boundary};
use crate::shares::{Link, PAGE_CSP, STYLE, esc, html, see_other};

/// The largest file a visitor may add (as large as the server reads from a library folder).
pub const FILE_MAX: u64 = crate::folders::FILE_MAX;
/// The largest form (all its files).
pub const FORM_MAX: u64 = 8 << 30;
/// Files in one form at most.
pub const FILES_MAX: usize = 500;
/// The longest name a visitor may give.
const NAME_MAX: usize = 64;

/// A file that arrived: where it is, its name, content hash and size.
struct Received {
    tmp: PathBuf,
    name: String,
    hash: String,
    size: u64,
}

/// Remove the temporary files when the request ends, whatever happened.
struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// A page that says why the photos weren't added, with the way back to the album.
fn failed(status: u16, token: &str, why: &str) -> crate::api::Resp {
    html(
        status,
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <meta name=\"robots\" content=\"noindex,nofollow\"><title>Photos not added</title><style>{STYLE}</style></head><body>\
             <main class=\"lock\"><h1>Photos not added</h1><p class=\"note bad\" role=\"alert\">{}</p>\
             <p><a class=\"btn\" href=\"/s/{token}\">Back to the album</a></p></main></body></html>",
            esc(why)
        ),
        PAGE_CSP,
    )
}

/// A file name a visitor's browser sent: its last part, without control characters (`photo` if nothing is left).
fn clean_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let n: String = last.chars().filter(|c| !c.is_control()).take(200).collect();
    let n = n.trim().to_string();
    if n.is_empty() || n == "." || n == ".." { "photo".into() } else { n }
}

/// `POST /s/<token>/upload`.
pub fn upload(st: &State, req: &mut Request, token: &str, user: &str, link: &Link) -> crate::api::Resp {
    let share = &link.share;
    if !share.upload {
        return failed(403, token, "This album doesn't take photos from visitors.");
    }
    let Some(b) = header_value(req, "Content-Type").and_then(boundary) else {
        return failed(400, token, "Choose photos with the form on the album's page.");
    };
    let len = req.body_length().map_or(0, |n| n as u64);
    if len > FORM_MAX {
        return failed(413, token, &format!("That is more than {} GB at once: add fewer photos at a time.", FORM_MAX >> 30));
    }
    if crate::api::over_quota(st, user, len).is_some() {
        return failed(507, token, "The album's owner has no room left on their server for these photos.");
    }
    let tmp_dir = accounts::user_dir(&st.data, user).join("blobs").join("tmp");
    if let Err(e) = std::fs::create_dir_all(&tmp_dir) {
        log::error!("{user}: {}: {e}", tmp_dir.display());
        return failed(500, token, "The server couldn't keep the photos.");
    }
    let mut cleanup = Cleanup(Vec::new());
    let mut files: Vec<Received> = Vec::new();
    let mut who = String::new();
    let mut skipped = 0usize;
    {
        let reader = req.as_reader();
        let mut form = Multipart::new(reader, &b);
        loop {
            let part = match form.next_part() {
                Ok(Some(p)) => p,
                Ok(None) => break,
                Err(e) => return failed(400, token, &format!("The upload broke off ({e}). Try again.")),
            };
            match part.filename {
                None if part.name == "name" => match form.read_text(NAME_MAX * 4) {
                    Ok(t) => who = t.chars().filter(|c| !c.is_control()).take(NAME_MAX).collect::<String>().trim().to_string(),
                    Err(e) => return failed(400, token, &e),
                },
                // (an empty file input sends a part without a file)
                Some(f) if !f.is_empty() => {
                    if files.len() >= FILES_MAX {
                        return failed(413, token, &format!("That is more than {FILES_MAX} files at once: add fewer at a time."));
                    }
                    let tmp = match accounts::random_hex(8) {
                        Ok(r) => tmp_dir.join(format!("guest-{r}.part")),
                        Err(e) => return failed(500, token, &e),
                    };
                    cleanup.0.push(tmp.clone());
                    let mut out = match std::fs::File::create(&tmp) {
                        Ok(f) => std::io::BufWriter::with_capacity(1 << 20, f),
                        Err(e) => {
                            log::error!("{user}: {}: {e}", tmp.display());
                            return failed(500, token, "The server couldn't keep the photos.");
                        }
                    };
                    let mut hasher = lightcraft_preview::Hasher128::new();
                    let mut size = 0u64;
                    let r = form.read_body(&mut |chunk| {
                        size += chunk.len() as u64;
                        if size > FILE_MAX {
                            return Err(format!("{} is larger than {} GB", clean_name(&f), FILE_MAX >> 30));
                        }
                        hasher.update(chunk);
                        out.write_all(chunk).map_err(|e| format!("the server couldn't keep the photos: {e}"))
                    });
                    let r = r.and_then(|()| out.into_inner().map_err(|e| e.to_string())).and_then(|f| f.sync_all().map_err(|e| e.to_string()));
                    if let Err(e) = r {
                        return failed(400, token, &format!("Not added: {e}."));
                    }
                    if size == 0 {
                        skipped += 1;
                        continue;
                    }
                    files.push(Received { tmp, name: clean_name(&f), hash: hasher.finish().to_string(), size });
                }
                _ => {
                    if let Err(e) = form.read_body(&mut |_| Ok(())) {
                        return failed(400, token, &format!("The upload broke off ({e}). Try again."));
                    }
                }
            }
        }
    }
    if files.is_empty() {
        return see_other(&format!("/s/{token}?skipped={skipped}"), None);
    }
    // what each file is (untrusted: under a guard, its memory counted)
    let now = lightcraft_engine::import::system_clock();
    let mut probed: Vec<(Received, Photo)> = Vec::new();
    for f in files {
        let described = {
            let _permit = lightcraft_engine::memory::work_gate().acquire(usize::try_from(f.size).unwrap_or(usize::MAX));
            std::fs::read(&f.tmp)
                .map_err(|e| e.to_string())
                .and_then(|bytes| lightcraft_engine::guard::catch("reading a visitor's photo", || describe(&f, &bytes, &now, &who))?)
        };
        lightcraft_engine::memory::release();
        match described {
            Ok(p) => probed.push((f, p)),
            Err(e) => {
                log::info!("{user}: a visitor's {} isn't a photo the server reads: {e}", f.name);
                skipped += 1;
            }
        }
    }
    // kept by content, as a device's upload is, before the library names them (a device that sees the
    // photo can fetch its file at once)
    let blobs = accounts::user_dir(&st.data, user).join("blobs");
    let l = match lib(st, user) {
        Ok(l) => l,
        Err(e) => {
            log::error!("{e}");
            return failed(500, token, "The server couldn't open the album.");
        }
    };
    let mut kept: Vec<(Received, Photo, PathBuf)> = Vec::new();
    for (f, p) in probed {
        let Some(dest) = blob_path(&blobs, "original", &f.hash) else { continue };
        let in_folder = l.lock().unwrap_or_else(PoisonError::into_inner).index.has(&f.hash);
        if !dest.is_file() && !in_folder {
            let moved = dest.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::rename(&f.tmp, &dest));
            if let Err(e) = moved {
                log::error!("{user}: keeping {}: {e}", dest.display());
                skipped += 1;
                continue;
            }
            st.sizes.add(user, f.size);
        }
        kept.push((f, p, dest));
    }
    // the library: new photos in the server's id space, then the album with all of them
    let (added, previews) = {
        let mut l = l.lock().unwrap_or_else(PoisonError::into_inner);
        let album = AlbumId(share.album);
        match l.core.catalog().album(album) {
            Some(a) if !a.is_smart() && !a.folder => {}
            _ => return failed(404, token, "This album is gone."),
        }
        let wanted: HashSet<&str> = kept.iter().map(|(f, ..)| f.hash.as_str()).collect();
        let mut have: HashMap<String, PhotoId> = HashMap::new();
        for q in l.core.catalog().photos().filter(|q| !q.deleted && !q.local) {
            if let Some(k) = lightcraft_catalog::sync::content_key(q).filter(|k| wanted.contains(k.as_str())) {
                have.entry(k).or_insert(q.id);
            }
        }
        let mut ids: Vec<Option<PhotoId>> = Vec::with_capacity(kept.len());
        let mut ops = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (i, (f, p, _)) in kept.iter().enumerate() {
            match have.get(&f.hash).copied() {
                Some(id) => ids.push(Some(id)),
                // (the same file twice in one form is one photo)
                None if !seen.insert(f.hash.clone()) => ids.push(None),
                None => {
                    let mut p = p.clone();
                    p.id = l.core.alloc_photo_id(crate::folders::SPACE);
                    ids.push(Some(p.id));
                    ops.push((Op::AddPhoto { photo: Box::new(p) }, i));
                }
            }
        }
        let names: Vec<String> = kept.iter().map(|(f, ..)| f.name.clone()).collect();
        let Some(refused) = crate::folders::push(&mut l, &ops, &|i| names.get(i).cloned().unwrap_or_default()) else {
            return failed(500, token, "The server couldn't save the album.");
        };
        // (a new photo the library refused isn't put in the album)
        let new_ids: HashSet<PhotoId> =
            ops.iter().filter(|(_, i)| !refused.contains(i)).filter_map(|(_, i)| ids.get(*i).copied().flatten()).collect();
        let mut members = l.core.catalog().album(album).map(|a| a.photos.clone()).unwrap_or_default();
        let before = members.len();
        let mut added = 0usize;
        for (i, id) in ids.iter().enumerate() {
            let Some(id) = id else { continue };
            let is_new = ops.iter().any(|(op, j)| *j == i && matches!(op, Op::AddPhoto { .. }));
            if (is_new && !new_ids.contains(id)) || members.contains(id) {
                continue;
            }
            members.push(*id);
            added += 1;
        }
        if members.len() > before
            && crate::folders::push(&mut l, &[(Op::SetAlbumPhotos { id: album, photos: members }, 0)], &|_| "the album".into())
                .is_none_or(|r| !r.is_empty())
        {
            return failed(500, token, "The server couldn't put the photos in the album.");
        }
        let previews: Vec<(String, PathBuf)> = kept.iter().map(|(f, _, dest)| (f.hash.clone(), dest.clone())).collect();
        (added, previews)
    };
    for (hash, dest) in previews {
        if dest.is_file() {
            st.folders.queue(&blobs, user, hash, dest);
        }
    }
    // (an imports folder takes the new files)
    st.writer.file_uploads(user);
    log::info!(
        "{user}: a visitor added {added} photo(s) to album {} through link {}{}",
        share.album,
        share.id,
        if skipped > 0 { format!(" ({skipped} left out)") } else { String::new() }
    );
    drop(cleanup);
    see_other(&format!("/s/{token}?added={added}&skipped={skipped}"), None)
}

/// The photo for a visitor's file (its id is given when it goes into the library).
fn describe(f: &Received, bytes: &[u8], now: &str, who: &str) -> Result<Photo, String> {
    let info = lightcraft_engine::files::probe_bytes(&f.name, bytes)?;
    let mut p = Photo::new(PhotoId(0), Source::File { path: original_path(&f.hash, &f.name) }, &f.name, &info.format, info.width, info.height, now);
    p.kind = info.kind;
    p.file_size = info.file_size;
    p.captured = info.captured;
    p.meta = info.meta;
    p.as_shot_wb = info.as_shot_wb;
    p.content_hash = Some(f.hash.clone());
    p.embedded_lens = info.embedded_lens;
    p.preview_only = info.preview_only;
    p.develop = Arc::new(p.camera_defaults());
    // a raw's own XMP (its edits from the camera's app)
    if p.kind == MediaKind::Raw
        && let Some(x) = info.xmp
    {
        let target = lightcraft_engine::crs::Target::for_photo(&p);
        if let Ok(sc) = sidecar::parse_sidecar(&x, target) {
            sidecar::merge_into(&mut p, &sc, now);
        }
    }
    if p.meta.creator.trim().is_empty() && !who.is_empty() {
        p.meta.creator = who.to_string();
    }
    p.imported = now.to_string();
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_browsers_are_file_names() {
        assert_eq!(clean_name("IMG_1.JPG"), "IMG_1.JPG");
        assert_eq!(clean_name("C:\\Users\\ann\\Pictures\\IMG_2.CR3"), "IMG_2.CR3");
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("a\r\nb.jpg"), "ab.jpg");
        assert_eq!(clean_name(".."), "photo");
        assert_eq!(clean_name("dir/"), "photo");
        assert_eq!(clean_name(""), "photo");
    }
}
