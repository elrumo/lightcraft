# Lens profiles from a lens database

Raws that carry no lens correction of their own (DNG `OpcodeList3`, or a camera correction a raw reader converts,
see [sony-lens-corrections.md](sony-lens-corrections.md) and [nikon-lens-corrections.md](nikon-lens-corrections.md)) are corrected from a **lens database in the lensfun
XML format**. The app does not ship any lens database: lensfun's data is licensed CC BY-SA 3.0 and is downloaded at
run time, never bundled and never committed. Adobe LCP profiles are never used.

## Setup: one click

Any of these, or none:

1. Nothing is downloaded without the user's click. On the first launch the desktop app asks once: "Download lens
   profiles? LightCraft can correct distortion and vignetting for many lenses using the open lens database
   (lensfun, about N MB)", with **Download** and **Not Now** (N is the pinned files' sizes added up). Not Now (or
   Esc) downloads nothing and the app doesn't ask again; Settings ▸ General ▸ Lens profiles has **Download Lens
   Database** and a switch that turns the offer back on. The answer is kept with the app's settings (`ui.json`,
   `lensDatabase`: `ask` / `accepted` / `declined`; a value it can't read asks again), and
   `lensfun_download::fetch_on_start` is the one place that decides. After a yes, a start that finds the database
   missing or from an older pin downloads it again (an interrupted download resumes).
   Download fetches lensfun's `data/db` (56 XML files, about 5 MB) into the app's data folder
   (`<config dir>/lensfun`, beside the face and denoise models; see `crates/engine/src/config.rs`), with a row in
   the activity stack (its ✕ stops it). The files are pinned to one lensfun commit by name,
   size and SHA-256 (`crates/engine/src/lensfun_download.rs`); a file that differs is thrown away. A later build with
   a newer pin replaces them once. The files are staged in `<config dir>/lensfun.download`, `SOURCE.txt` (the commit
   and the licence) is written there last, and the staged folder then replaces the old one as a whole: until it is
   in place the previous complete database is used, and if the swap fails it is put back. The app's folder is read
   only when it has `SOURCE.txt`, and a folder whose pinned files are missing or cut short is downloaded again.
   The CLI (`render`, `merge`, `snapshot`) and the headless MCP server read this folder too, but never ask and never
   fetch on their own; `lens.downloadDatabase` fetches it on request from any of them.
2. By default the files come from lensfun's GitHub repository at the pinned commit. `LIGHTCRAFT_LENSFUN_URL` lists
   other places serving the same files (base URLs, tried first), e.g. a copy on LightCraft's own server.
3. **A profile database on a server.** A profile database published as described in [profile-db.md](profile-db.md) is
   downloaded and kept up to date with `profileDb.update {url}`. lensfun's database can be republished there with its
   credit, which the app shows. It is read again after each update, without a restart.
4. `LIGHTCRAFT_LENSFUN_DIR` points at a database of your own instead (lensfun's `data/db`, or a Linux
   distribution's `/usr/share/lensfun/version_1/`); then the app doesn't offer the download, and that folder is read
   even after `lens.downloadDatabase` fetches the app's own copy.
   Its profiles come before the downloaded ones.

When the database arrives (or changes), every library raw without a profile whose lens it knows is read again in the
background and gets its profile; a photo still as imported gets the correction switched on, as import would, and an
edited one keeps its settings. When a download replaced the database's files, the raws whose profile came from the
old database are read again too (they get the new profile, or lose one it no longer gives), and so are raws whose
file named no lens at import. Ticking **Enable Lens Corrections** on a photo without a profile looks it up again
too. `library.refreshLensProfiles` (`all: true` for the wider pass), `lens.downloadDatabase` (which also selects the
downloaded folder) and `lens.databaseStatus` do the same for agents.

A database profile is pinned to the photo: every render (fresh decode, cached preview, thumbnail, export) uses the
profile the catalog holds, never one a decode looked up in a newer database, and the profile is part of the render
and thumbnail cache keys, so a changed profile never shows an old cached render. A file's own correction comes from
the file as before.

The Optics panel says which profile is loaded: "Profile: from the file", "Profile: <lens> (lens database)", or
"No lens profile found for <lens>". A profile is never copied between photos: presets, Copy / Paste, Sync and Auto
Sync carry Enable Lens Corrections and its amounts, and each photo keeps the profile of its own lens
(`crates/engine/src/tests/lens_refresh.rs`).

A photo records where its correction comes from (`photo.lensProfile`): its file, a measurement from the camera JPEG,
or a database profile. A database profile replaces a CR3's measured vignetting and an older database's profile when
the photo is next decoded. It never replaces a file's own correction ([profile-db.md](profile-db.md)).

Files are read once per folder and again after a download. Invalid files are skipped with a warning in the log.
Each read logs the folder, whether it is the download or `LIGHTCRAFT_LENSFUN_DIR`, and how many lenses and cameras
it holds; a folder that is missing or holds no lenses is logged as a warning (#2511). The app's own folder before a
download has finished is not read, and says so at info level, since that is the state before the first download.

## What is applied

The lens is identified from EXIF: camera make / model (for the mount and crop factor; the 35 mm equivalent focal
length is used when the camera isn't listed or its crop factor is unknown) and the lens model string. Lens names match on their words; every
number (focal lengths, apertures) in the database name must appear in the EXIF name, and the lens maker must agree
when the file names one.

Some cameras record no lens model at all, only the lens specification: Nikon DSLRs write the lens's focal range and
maximum apertures in their maker note (tag `0x0084`) and no Exif `LensModel`, whatever lens is mounted. For such a
file the lens is looked up by a name built from the specification, `24-70mm f/2.8` (`18-55mm f/3.5-5.6`,
`85mm f/1.8`), and a profile is used only when one of its names is exactly that (ignoring case and punctuation).
The specification fits every maker's lens with that range (a Nikkor, a Sigma, a Tamron), so neither a partial match
nor a name with a maker in it ("Samyang 85mm f/1.4" for "85mm f/1.4") is taken; a database that should correct such
files names its profile that way, e.g. a lensfun entry with an extra `<model>24-70mm f/2.8</model>` for the one
24-70 f/2.8 the photographer owns. lensfun's own database has no such names, so it doesn't correct these files.

| Correction | Models | Interpolated over |
|---|---|---|
| Distortion | `poly3`, `poly5`, `ptlens` | focal length |
| Lateral CA (TCA) | `linear`, `poly3` | focal length |
| Vignetting | `pa` | focal length, aperture (in stops), focus distance (nearest calibrated, by 1/d; infinity when unknown) |

Fisheye and other non-rectilinear lenses get no distortion correction. The profile is converted by least squares
into the DNG `WarpRectilinear` / `FixVignetteRadial` polynomials, so it goes through the same path as embedded
corrections: **Enable Profile Corrections** and its Distortion / Vignetting amounts, the CPU and GPU geometry
stage (one resample), orientation handling and DNG export. A conversion whose radial error exceeds 0.001 of the
half diagonal, or that would fold the image, is not applied.

Normalisation follows lensfun's documentation: distortion and TCA radii are 1 at half the shorter side of the
calibration frame (its `cropfactor` and `aspect-ratio`), vignetting radii 1 at its corner; the optical centre is
the image centre (lensfun's `<center>` offsets are not read). Missing or invalid crop factors remain unknown;
only finite values from 0.05 through 20 are accepted. A calibration inherits the lens's crop factor only when its
`cropfactor` attribute is omitted; an explicit invalid override remains unknown. Profiles with an unknown
calibration crop factor are not applied. An explicit crop factor of 1 still means full frame.
On exact lookup-score ties, both named and built-in fixed-lens matching prefer a known calibration crop; a higher
match score still wins even when its crop is unknown.

## Canon CR3: vignetting measured from the camera JPEG

A CR3 has no lens correction LightCraft can read (Canon's maker note records that Peripheral Illumination Correction
was on, tag `0x4016` `PeripheralLightingSetting`, not the correction itself), and no lens database is bundled, so RF
lenses rendered with dark corners where Lightroom applies its profile and the camera JPEG has Canon's correction.
The correction the camera applied can be read back from that JPEG: it is the sensor times a radial gain, put through
the camera's tone curve. `camera_preview::jpeg_vignetting` pairs the 96 px sensor proxy with the camera JPEG proxy
(the same pair the camera-look fit uses, orientation and framing aligned) and fits the gain
`1 + k0 ρ² + k1 ρ⁴ + k2 ρ⁸` (ρ: distance from the centre over the half diagonal; every term ≥ 0, so the gain never
dips below 1 and rises to the corners) together with a monotone tone curve, by Gauss-Newton on pixels away from
clipping and edges. On the 19 photos below, `1 + k0 ρ² + k1 ρ⁴` alone fell 0.1 EV short of the camera's gain in the
outer corners (ρ² > 0.8; 0.02 EV with the ρ⁸ term), and unconstrained it dipped to 0.92 just off the centre without
explaining held-out pixels any better.

The estimate is kept only when a third of the pixels, held out of the fit, are explained clearly better than without
it (squared error below 0.9 of the uncorrected one), when it brightens the corners by 1.05–4×, and with at least 400
usable training pixels, 64 of them beyond ρ ≈ 0.7. It is the same `EmbeddedVignette` a DNG's `FixVignetteRadial` gives: **Enable
Profile Corrections** applies it with its Vignetting amount, on the CPU and GPU path, and Export as DNG / Convert to
DNG write it as a `FixVignetteRadial`. CR3s start with Enable Profile Corrections on (as raws with lens data do); the
estimate is made when the photo is first decoded (an import only reads headers), recorded on the photo so smart
previews and synced devices render with it, and turns the correction on for a photo still as import left it. Reload
keeps it. While a photo has no lens data the switch corrects nothing and is not counted as an edit
(`Photo::develop_is`), so CR3s imported earlier stay unedited and keep their embedded-preview thumbnails. A CR3 whose camera did not correct (setting off, a third-party lens the camera has no data for, a small
aperture with little vignetting) gets nothing. Lens data a file carries, or a database profile, wins; other formats
are not estimated yet (their cameras correct too: Sony shading, Nikon vignette control).

The camera-look fit of CR3s without measured matrices (`fit_preview`) now pairs the JPEG with the sensor corrected
the same way, or its tone curve would learn part of the correction and brighten the corners a second time.
`LOOK_VERSION` 15 rebuilds the smart previews of raws and `RENDER_CACHE_VERSION` 70 redraws renders; a sync server
needs the new build to record the estimate for photos it decodes.

Measured on the 19 EOS R6 CR3s (RF 24-105 mm F4 L at 24, 85 and 105 mm, f/4) of
[camera-preview-colour.md](camera-preview-colour.md#canon-cr3-a-neutral-look-instead-of-the-camera-jpeg), all
accepted: corner gain 1.47–2.54 (mean 1.98), held-out error 0.19 of the uncorrected one on average (at most 0.40).
Brightness at ρ relative to each reference, rendered at 2048 px with default settings, after a monotone tone map from
the render to the reference fitted on ρ < 0.4 (EV in the reference's display-linear values, mean over the photos;
positive: the reference is brighter there):

| reference | ρ 0.55–0.7 | 0.7–0.8 | 0.8–0.9 | 0.9–1 |
|---|---|---|---|---|
| camera JPEG, before | +0.23 | +0.41 | +0.67 | +1.08 |
| camera JPEG, after | +0.05 | +0.05 | +0.06 | +0.13 |
| Lightroom (default export, lens profile on), before | +0.38 | +0.66 | +1.01 | +1.58 |
| Lightroom, after | +0.21 | +0.32 | +0.44 | +0.67 |
| Lightroom against the camera JPEG | +0.15 | +0.26 | +0.37 | +0.55 |

Most of what is left against the camera JPEG is one photo (+0.87 EV in its outer ring; +0.09 over the other 18). Its
fitted gain follows the camera's in linear values, and its corners are dark: likely Auto Lighting Optimizer (Strong
on that photo) lifting dark regions, which a lens correction should not copy. Lightroom's corners stay brighter than the camera's: Canon's in-camera correction is partial (its
manuals say it is below the maximum Digital Photo Professional applies), and LightCraft reproduces the camera's, not
Adobe's profile. The CC0 corpus CR3s: the EOS M50 with the EF-M 15-45 mm at 15 mm f/4 gets corner gain 1.37 (held-out
error 0.43 of uncorrected); the 90D at f/6.3, the R at f/11, the R5 with a Sigma lens and the R6 at f/25 get none
(gain under 1.05, or no clear held-out gain).

## Limits

- Only raws: rendered images (JPEG, HEIF, TIFF) are often already corrected in camera.
- The focus distance is not read from maker notes yet: vignetting uses the farthest calibrated distance.
- No manual make / model picker and no choice between sources (in the file, lens database, an own calibration)
  yet; matching is automatic, the file's own correction wins. On lensfun's database (revision `b98997f`), EXIF names
  from Sony, Canon RF / EF-S, Nikon Z / F, Fujifilm XF, OM System, Panasonic, Sigma and Tamron lenses each found their
  profile (14 names tried).
- Database profiles are not yet validated against real photos and another renderer; the tests use synthetic profiles
  and grids.
- The CR3 estimate copies the camera's partial correction (about 0.5 EV short of Lightroom's in the outer corners on
  the RF 24-105 mm), is centred (no optical-centre offset) and covers vignetting only (no distortion or lateral CA).
