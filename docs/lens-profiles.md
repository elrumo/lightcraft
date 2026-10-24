# Lens profiles from a lens database

Raws that carry no lens correction of their own (DNG `OpcodeList3`, or a camera correction a raw reader converts,
see [sony-lens-corrections.md](sony-lens-corrections.md) and [nikon-lens-corrections.md](nikon-lens-corrections.md)) can be corrected from a **lens database in the lensfun
XML format** that you supply. The app does not ship any lens database: lensfun's data is licensed CC-BY-SA 3.0 and
stays a separate download. Adobe LCP profiles are never used.

## Setup

Either:

- **Download it from a server.** A profile database published as described in [profile-db.md](profile-db.md) is
  downloaded and kept up to date with `profileDb.update {url}`. lensfun's database can be republished there with its
  credit, which the app shows. It is read again after each update, without a restart.
- **Point the app at a folder.**
  1. Get the lensfun database: the `data/db/*.xml` files of <https://github.com/lensfun/lensfun>, or the copy a Linux
     distribution installs under `/usr/share/lensfun/version_1/`.
  2. Point `LIGHTCRAFT_LENSFUN_DIR` at that directory and start the app (or the CLI).

  The folder is read once per process, and its profiles come before the downloaded ones.

Invalid files are skipped with a warning in the log. Without either, nothing changes.

A photo records where its correction comes from (`photo.lensProfile`): its file, a measurement from the camera JPEG,
or a database profile. A database profile replaces a CR3's measured vignetting and an older database's profile when
the photo is next decoded. It never replaces a file's own correction ([profile-db.md](profile-db.md)).

## What is applied

The lens is identified from EXIF: camera make / model (for the mount and crop factor; the 35 mm equivalent focal
length is used when the camera isn't listed) and the lens model string. Lens names match on their words; every
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
the image centre (lensfun's `<center>` offsets are not read).

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
- No manual make / model picker yet; matching is automatic. On lensfun's database (revision `b98997f`), EXIF names
  from Sony, Canon RF / EF-S, Nikon Z / F, Fujifilm XF, OM System, Panasonic, Sigma and Tamron lenses each found their
  profile (14 names tried).
- Database profiles are not yet validated against real photos and another renderer; the tests use synthetic profiles
  and grids.
- The CR3 estimate copies the camera's partial correction (about 0.5 EV short of Lightroom's in the outer corners on
  the RF 24-105 mm), is centred (no optical-centre offset) and covers vignetting only (no distortion or lateral CA).
