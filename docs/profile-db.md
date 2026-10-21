# Camera and lens profiles from a server

LightCraft can keep a **profile database** up to date from a web server: camera colour profiles (the
[`calibrate`](camera-preview-colour.md) kind, one per camera model) and lens profiles (lensfun's XML format,
[lens-profiles.md](lens-profiles.md)). Whoever publishes the database decides what is in it and where it lives. The
app ships without one, and a frontend downloads it only from an address it is given. Adobe camera or lens profiles
(DCP, LCP) are never used.

## What a device does

- `profileDb.update {url}` checks the feed at `url` in the background, shown as an activity row ("Updating camera and
  lens profiles") that can be cancelled. It downloads `index.json`, then only the files this device lacks. Each file
  is checked against the size and SHA-256 the index gives. The new set is then installed in one step: the index is
  replaced by a single rename, so a crash or a lost connection leaves the old set in place. Without `url` it uses
  `LIGHTCRAFT_PROFILE_DB_URL`, else the address used last time. No photo data is sent, only plain GET requests.
- `profileDb.status` reports what is installed and what the last check did:
  - the number, publication date, camera models, lens and camera counts, and size;
  - the **sources with their licences and credits**, which a settings page must show (lensfun's licence requires it);
  - files that could not be used;
  - the last check's time and error.
- `profileDb.remove` deletes the downloaded database. Photos keep the lens corrections already recorded on them.
- `photo.lensProfile {id?}` reports a photo's lens correction and where it comes from:
  - `file`: the raw's own correction (DNG opcodes, or a camera's tables).
  - `measured`: a Canon CR3's vignetting, measured from its camera JPEG.
  - `database`: a lens profile, reported with its maker, model and mounts.

  It also reports whether **Enable Profile Corrections** is on.

The database is kept in `<settings>/profile-db/`, or in `LIGHTCRAFT_PROFILE_DB` when that is set. The folder holds
`index.json`, `files/<sha256>` and `feed.json` (the last check). It is read on first use and read again after an
update, without a restart.

**Where profiles come from, in order.** For camera colour:

1. a profile in the local folder (`LIGHTCRAFT_CAMERA_PROFILES` or `<settings>/camera-profiles`);
2. the database's profile;
3. the one built in (`assets/camera-profiles/`).

For lenses, the file's own correction always wins, and a raw that has one is never looked up. Without one, the lens
named in EXIF is looked up in `LIGHTCRAFT_LENSFUN_DIR` first, then in the database.

**When photos pick it up.**

- **Lens profiles.** A photo imported after an update gets its profile at import. A photo already in the library gets
  it the next time it is decoded (opened in the loupe, or rendered from its original):
  - the correction is recorded on the photo, so smart previews and synced devices use it;
  - for a photo still as import left it, the correction is switched on, as for a file's own lens data;
  - a database profile replaces a CR3's measured vignetting, and an older database's profile;
  - nothing replaces a file's own correction or a newer database's profile.

  The photo records the database number its profile came from (`LensOrigin::Database { serial }` on `EmbeddedLens`).
  So a synced device with an older database, or none, keeps the newer profile instead of taking it away.
- **Camera profiles.** These change the render cache key, so pictures are drawn again as they are next shown. The
  per-file colour fits are keyed by the database generation and are fitted again.

  A lens-only update leaves rendered pictures alone.
- **Smart previews keep the look they were built with.** Rebuild them from the originals to pick up a new camera
  profile.

## Feed format

A feed is a folder on any web server:

```
index.json            the manifest (format 1, at most 1 MB)
files/<sha256>        every file the manifest lists, named by its SHA-256 (lower-case hex)
```

```json
{
  "format": 1,
  "serial": 7,
  "published": "2026-10-21T09:30:00Z",
  "sources": [
    {"id": "lensfun", "title": "lensfun lens database", "licence": "CC-BY-SA-3.0",
     "url": "https://github.com/lensfun/lensfun", "credit": "Lens profiles © the lensfun contributors, CC BY-SA 3.0",
     "revision": "b98997f"},
    {"id": "own", "title": "Camera and lens profiles", "licence": "MIT OR Apache-2.0", "url": "", "credit": "…", "revision": ""}
  ],
  "files": [
    {"kind": "lens", "name": "mil-sony.xml", "source": "lensfun", "sha256": "…", "size": 120345},
    {"kind": "camera", "name": "ILCE-7M4.json", "model": "ILCE-7M4", "source": "own", "sha256": "…", "size": 61022}
  ]
}
```

**Kinds.**

- `lens`: a lensfun-format XML file.
- `camera`: a camera profile, named for its Exif `Model`. A manifest has at most one per model.
- Any other kind is skipped by this version, so a newer feed can add kinds.

A manifest of another `format` is refused with "update the app".

**Rules.**

- **The serial must grow** with every publication. A device refuses a feed whose serial is smaller than the one it
  has: remove the downloaded profiles to go back.
- **Limits.**
  - At most 4,000 files and 64 sources.
  - Each file at most 16 MB, and 256 MB in all.
  - Text fields at most 1,024 characters.
  - Every file names a listed source.
- **Addresses** must be `https://`. Plain `http://127.0.0.1` is accepted only for tests.
- **Files are data**, parsed with the same bounds as files a user puts in place. A file that fails to parse is
  skipped and listed in `skipped`.

## Publishing

```sh
git clone --depth 1 https://github.com/lensfun/lensfun.git
lightcraft-cli profile-db build --out site/profiles \
    --lensfun lensfun/data/db --lensfun-revision "$(git -C lensfun rev-parse --short HEAD)" \
    --cameras my-camera-profiles --credit "Profiles by …"
rsync -a site/profiles/ server:/var/www/profiles/
```

**What `profile-db build` does.**

- It checks every file with the app's parsers, so it never writes a feed the app would refuse.
- It numbers the publication one above the folder's last (`--serial` to choose).
- It keeps the files of earlier publications for devices in the middle of an update (`--prune` deletes them).
- It does not publish neutral-look tables (`*.neutral.json`).

lensfun's `data/db` gives 1,570 lens profiles and 1,057 cameras in 63 files, 5.7 MB (revision `b98997f`, October
2026). Camera profiles come from `lightcraft-cli calibrate` on photos whose owners agree to their use, fitted to
their cameras' own JPEGs. Fitting to another raw converter's output needs the maintainer's go-ahead (`CLAUDE.md`).

**Serving.** Any static file server will do. `files/*` never change, so they can be cached forever; `index.json`
should be revalidated. With Caddy:

```
handle_path /profiles/* {
    root * /var/www/profiles
    header /files/* Cache-Control "public, max-age=31536000, immutable"
    header /index.json Cache-Control "no-cache"
    file_server
}
```

**Licences.**

- **lensfun's database** is CC-BY-SA 3.0. Republishing it is allowed with its credit; the `lensfun` source carries
  that credit, and the app shows it. Changes to its files must stay CC-BY-SA.
- **Camera profiles** are the publisher's to license. The bundled ones are MIT OR Apache-2.0
  (`assets/ATTRIBUTION.md`).

## Limits

- **No signature yet.** The index's integrity rests on HTTPS and the publisher's server; each file's rests on the
  index's SHA-256. A signed index (Ed25519, its public key built into the app) is the next step for feeds served from
  mirrors.
- **No compression.** The client asks for no compression, so a first download of lensfun's database is about 6 MB.
- **Photos already in the library update only when decoded** or reloaded from disk (`photo.reload`). No pass looks
  them up in the background after an update, because reading each raw's header again costs a full read on most
  formats.
- **Neutral-look tables and spectral matrices are not distributed.**
