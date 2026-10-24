//! Files that go beside a photo, sent by a device: an XMP sidecar, or an edit file such as
//! `IMG_0001-Edit.tif` (the Lightroom import sends them for photos the server holds).
//!
//! `PUT /api/companions/<content hash>?name=<file name>` keeps the file under
//! `blobs/companion/<hash>/<name>`, then [`place`] puts it beside the photo when a library folder the
//! server may write in holds that photo: never over a file that is there (a sidecar counts under either
//! naming), and the photo itself is never touched. A photo that lives only in the server's own store has no
//! folder: the file waits until the photo is filed into the imports folder
//! ([`crate::writeback`]), which places it too. The scan then finds the new file like any other.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use lightcraft_engine::sidecar::{self, SidecarNaming};
use serde_json::json;

use crate::State;
use crate::accounts;
use crate::api::{Resp, UserLib, error, json, lib, over_quota, query};
use crate::http::Request;

/// The largest companion file taken (a layered Photoshop edit can be a few GB).
const MAX_BYTES: u64 = 8 << 30;
/// What a device may send: an XMP sidecar, or what Edit in Photoshop leaves.
const EXTENSIONS: [&str; 8] = ["xmp", "tif", "tiff", "psb", "psd", "jpg", "jpeg", "png"];

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |k: usize| b.get(i + k).and_then(|h| char::from(*h).to_digit(16));
        match (c, hex(1), hex(2)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push(u8::try_from(hi * 16 + lo).unwrap_or(b'_'));
                i += 3;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn store_dir(data: &Path, user: &str, hash: &str) -> PathBuf {
    accounts::user_dir(data, user).join("blobs").join("companion").join(hash)
}

/// `PUT /api/companions/<hash>?name=<file name>`: keep the file, and place it if the photo is in a writable folder.
pub fn put(st: &State, req: &mut Request, user: &str, hash: &str, q: &str) -> Resp {
    let Some(hash) = lightcraft_preview::Hash128::parse(hash).map(|h| h.to_string()) else {
        return error(400, "not a content hash (32 hex digits)");
    };
    let name = crate::writeback::safe_file_name(&percent_decode(query(q, "name").unwrap_or("")));
    let ext = Path::new(&name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return error(400, "?name=<file name>: an .xmp sidecar, or a .tif, .tiff, .psb, .psd, .jpg, .jpeg or .png edit");
    }
    let incoming = req.body_length().map_or(0, |n| n as u64);
    if incoming > MAX_BYTES {
        return error(413, format!("larger than {MAX_BYTES} bytes"));
    }
    if let Some(r) = over_quota(st, user, incoming) {
        return r;
    }
    let dir = store_dir(&st.data, user, &hash);
    let stored = (|| -> Result<(), String> {
        let io = |e: std::io::Error| e.to_string();
        std::fs::create_dir_all(&dir).map_err(io)?;
        let part = dir.join(format!(".{}.part", accounts::random_hex(8)?));
        let mut f = std::fs::File::create(&part).map_err(io)?;
        let copied = std::io::copy(&mut req.as_reader().take(MAX_BYTES + 1), &mut f).map_err(io).and_then(|n| {
            if n > MAX_BYTES { Err("too large".to_string()) } else { f.flush().and_then(|()| f.sync_all()).map_err(io) }
        });
        match copied.and_then(|()| std::fs::rename(&part, dir.join(&name)).map_err(io)) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Err(e)
            }
        }
    })();
    if let Err(e) = stored {
        return error(500, e);
    }
    let placed = place(st, user, &hash);
    json(200, &json!({"placed": placed}))
}

/// Put the companion files kept for content `hash` beside the photo, where a library folder the server may write
/// in holds it. Returns how many were placed; a file whose place is taken is dropped, the rest stay kept.
pub(crate) fn place(st: &State, user: &str, hash: &str) -> usize {
    let dir = store_dir(&st.data, user, hash);
    let kept: Vec<(PathBuf, String)> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .filter_map(|e| Some((e.path(), e.file_name().to_str()?.to_string())))
                .filter(|(_, name)| !name.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    if kept.is_empty() {
        return 0;
    }
    let writable = crate::writeback::writable_folders(st, user);
    let Ok(l) = lib(st, user) else { return 0 };
    let mut guard = l.lock().unwrap_or_else(PoisonError::into_inner);
    let UserLib { core, index, dir: user_dir, .. } = &mut *guard;
    let Some(key) = index.keys_of(hash).iter().find(|k| k.split_once('/').is_some_and(|(root, _)| writable.contains_key(&root.to_lowercase()))).cloned()
    else {
        return 0;
    };
    let Some(original) = index.path_of(&key).filter(|p| p.is_file()) else { return 0 };
    let Some(folder) = original.parent() else { return 0 };
    let original = original.to_string_lossy().to_string();
    let naming = index
        .files
        .get(&key)
        .and_then(|e| e.photo)
        .and_then(|id| crate::writeback::naming_of(core.catalog()).get(&lightcraft_engine::catalog::PhotoId(id)).copied())
        .unwrap_or(SidecarNaming::Stem);
    let mut placed = 0;
    for (src, name) in kept {
        let is_sidecar = name.to_lowercase().ends_with(".xmp");
        let target = if is_sidecar {
            // (a sidecar there, under either name, is the photo's own: it stays)
            if sidecar::find_sidecar(&original, naming).is_some() {
                let _ = std::fs::remove_file(&src);
                continue;
            }
            sidecar::sidecar_path(&original, naming)
        } else {
            folder.join(&name)
        };
        if target.exists() {
            let _ = std::fs::remove_file(&src);
            continue;
        }
        let temp = folder.join(format!(".{name}.lightcraft-part"));
        let copied = std::fs::copy(&src, &temp).and_then(|_| std::fs::OpenOptions::new().write(true).open(&temp)?.sync_all());
        let moved = copied.and_then(|()| if target.exists() { Err(std::io::ErrorKind::AlreadyExists.into()) } else { std::fs::rename(&temp, &target) });
        match moved {
            Ok(()) => {
                let _ = std::fs::remove_file(&src);
                placed += 1;
                if is_sidecar && let Some(e) = index.files.get_mut(&key) {
                    // (written by the server itself: the scan needn't read it back)
                    e.sidecar = sidecar::modified(&target);
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                log::warn!("{user}: {name} beside {original}: {e}");
            }
        }
    }
    if placed > 0 {
        if let Err(e) = index.save(user_dir) {
            log::warn!("{user}: {e}");
        }
        drop(guard);
        // (a new edit file becomes a photo)
        st.folders.request(user);
    }
    let _ = std::fs::remove_dir(&dir);
    placed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_decoded() {
        assert_eq!(percent_decode("IMG%201-Edit.psb"), "IMG 1-Edit.psb");
        assert_eq!(percent_decode("%E6%97%A5.tif"), "日.tif");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
