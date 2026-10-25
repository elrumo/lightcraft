# File format parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-25 · **Change:** minor (gain map JPEG read downgraded to 🟡 (import never applies the gain map); PQ / HLG PNG and lossless PQ JPEG XL decode; lossy DNG export; Render to DNG as an export format; PSD write ✅: export and Edit In write flat 8/16-bit PSDs, and what an external editor saves, a Save As as a PSD included, comes back; Render to DNG as an export format) · **Target:** Adobe Lightroom Classic 15.5 / Lightroom 9.6

Every format Lightroom reads or writes, with our support, fidelity and tests. Camera raw formats are
deep enough to have their own document: [raw-parity.md](raw-parity.md).

**How measured:** the target's document types come from the installed Lightroom 9.6
`Info.plist` (`CFBundleDocumentTypes`: catalog `mcat`, `xmp`, toolkit / module / library bundles, and
the raw-and-image list: jpg, tif, dng, heic and 27 raw extensions) plus Lightroom Classic's documented
import and export formats. Ours come from `crates/codecs/src/sniff.rs`, `crates/codecs/src/encode.rs`,
`crates/engine/src/export.rs`, `crates/engine/src/import.rs` and the tests named below.

## Images (non-raw)

| Format | Target reads | Target writes | LightCraft reads | LightCraft writes | Notes / tests |
|---|---|---|---|---|---|
| JPEG | ✅ | ✅ | ✅ (parallel decoder, ICC, EXIF orientation) | ✅ quality, size limit, ICC, metadata; Huffman tables optimized per image (issue #1373) | `crates/codecs/src/jpeg.rs`, `crates/codecs/src/jpeg_par.rs`, export tests |
| TIFF | ✅ 8/16/32-bit | ✅ 8/16-bit, LZW/ZIP | ✅ 1–64-bit integer, 16–64-bit float, bilevel, CCITT fax, WhiteIsZero | ✅ 8/16-bit, 32-bit float linear, none / LZW / ZIP | `crates/tiff`, `crates/codecs/src/tiff_codec.rs` |
| PNG | ✅ | ✅ | ✅ | ✅ | |
| PSD / PSB | ✅ (maximized-compatibility composite) | ✅ (Classic: PSD export, edit-in round trip) | ✅ merged composite | ✅ PSD, flat 8/16-bit with ICC (export, Edit In); ⬜ PSB. Edit In round trip: an editor's saves over the file come back, and so does its Save As beside it (e.g. a PSD of the TIFF copy, added to the stack) | `crates/codecs/src/psd.rs`, `crates/engine/src/tests_external_edit.rs` |
| HEIC / HEIF | ✅ | ⬜ (Classic) / HDR only | ✅ in every release build (`crates/heif`, heic-rs) | ⬜ | `heif` feature on by packaging scripts; EXIF and XMP read from the container in every build (`crates/meta/src/heif.rs`, #693) |
| AVIF | ✅ | ✅ (HDR) | ⬜ recognised, not decoded (no permissive pure-Rust AV1 decoder) | ✅ 8/10-bit, PQ HDR | EXIF and XMP read from the container (`crates/meta/src/heif.rs`); exported EXIF reads back |
| JPEG XL | ✅ | ✅ | ✅ (`jxl-oxide`) incl. DNG 1.7 JXL tiles | ✅ lossy / lossless, 8/16-bit, PQ HDR (`jixel`, native only; ⬜ web) | `crates/engine/src/tests_export.rs` |
| WebP | not in its document types | ⬜ | ✅ | ✅ | beyond target |
| GIF, BMP | ⬜ | ⬜ | ✅ | ⬜ | beyond target |
| DNG (from raw) | ✅ | ✅ incl. lossy, embedded original | ✅ | ✅ uncompressed, LJ92, Deflate or lossy (JPEG tiles, optional long edge); ⬜ embedded original | `crates/raw/src/dngwrite.rs` |
| Render to DNG (15.5) | — | ✅ | — | ✅ Photo ▸ Render to DNG (stacked with the original) and the "Rendered DNG" export (linear float DNG, edit baked in) | `crates/raw/src/dngwrite.rs` (`write_rendered_dng`) |
| Original + sidecar | — | ✅ | — | ✅ | |
| HDR gain map JPEG (ISO 21496-1) | ✅ | ✅ | 🟡 the gain map can be parsed and applied (`gainmap::read_jpeg`, `apply`, tested) but import never calls it: a gain map JPEG or an iPhone HEIC opens as its SDR base | ✅ | `crates/codecs/src/gainmap.rs` |
| PQ / HLG HDR | ✅ | ✅ AVIF, JXL, TIFF | 🟡 PQ / HLG PNG and HEIF `nclx` decode to linear light with SDR white at 203 cd/m² (they were read as sRGB); a lossy PQ JPEG XL is still tone mapped to 255 cd/m² by the decoder; PQ AVIF can't be opened | 🟡 PQ AVIF, HLG AVIF, PQ PNG, PQ JPEG XL, float TIFF | |
| Contact sheet / print PDF | — | ✅ (Print module: PDF / JPEG / printer) | — | 🟡 paginated contact-sheet PDF (A4 / Letter, 150 dpi, sRGB) | `crates/engine/src/contact_sheet.rs` |
| Video (MP4, MOV, AVCHD…) | ✅ playback, trim, edit | ✅ export | 🟡 catalogued (kind, duration) | ⬜ | see video gap |

## Colour spaces and profiles

| Feature | Target | LightCraft |
|---|---|---|
| Export colour spaces | sRGB, Adobe RGB, Display P3, ProPhoto, Rec.2020, custom ICC | ✅ sRGB, Display P3, Adobe RGB-compatible, ProPhoto, Rec.2020 with gamut mapping (CPU and GPU), embedded ICC; ⬜ custom ICC output profiles; AVIF stays sRGB |
| Input ICC (JPEG / TIFF / PNG) | ✅ | ✅ matrix / TRC and LUT profiles (moxcms) |
| Soft proofing with printer profiles | ✅ | 🟡 |

## Metadata, sidecars, presets and catalogs

| Format | Target | LightCraft | Notes |
|---|---|---|---|
| XMP sidecars (`crs:` develop settings, ratings, keywords, regions) | read / write | ✅ read / write, manual (⌘S) and automatic | [xmp-interop.md](xmp-interop.md) |
| EXIF / IPTC / XMP embedded on export | ✅ | ✅ with metadata filters | `crates/meta` |
| Develop presets `.xmp` | ✅ | ✅ import (mapping of supported settings) | |
| Legacy presets `.lrtemplate` | ✅ (converted) | ✅ import | |
| Camera / creative profiles (`.dcp`, `.xmp` profiles) | ✅ | ⬜ deliberately (Adobe profile formats are never used); `.cube` LUT profiles ✅ | |
| Lens profiles `.lcp` | ✅ | ⬜ deliberately | |
| Other apps' presets | — | ✅ Luminar `.lmp` / `.mplumpack`, `.zip` bundles | beyond target |
| Lightroom Classic catalog `.lrcat` | native | 🟡 import (ratings, flags, labels, keywords, collections, virtual copies, supported develop settings; read-only pure-Rust SQLite incl. WAL) | [lightroom-catalog-import.md](lightroom-catalog-import.md) |
| Lightroom (cloud) library `.lrlibrary` | native | ⬜ | cloud library format, local cache only |
| Write back to `.lrcat` | native | ⬜ (one-way migration by design) | |
| Keyword lists (text) | import / export | ✅ | |
| GPX track logs | ✅ | ✅ | `crates/meta/src/gpx.rs` |
| Export presets, metadata presets, filename templates | ✅ (`.lrtemplate`) | ✅ own JSON | not interchangeable |
| Lightroom plug-ins (`.lrplugin`, `.lrdevplugin`) | ✅ | ⬜ | see ecosystem in [target-app-parity.md](target-app-parity.md) |

## Scores

| Part | Weight | Ready for real work | Kind |
|---|---:|---:|---|
| Standard image import / export | 35% | ~85% | estimated (table above; missing: AVIF read, Render to DNG in the catalog) |
| Sidecars, metadata, presets | 25% | ~85% | estimated |
| Lightroom catalog migration | 25% | ~50% | estimated (one-way, approximate rendering of unsupported settings) |
| HDR, print, video output | 15% | ~35% | estimated |
| **Non-raw file formats** | 100% | **~70%** | estimated; raw is in [raw-parity.md](raw-parity.md) (~50%) |

Remaining: **22–39 h** (AVIF decode 8–15 h if a permissive decoder is written or found; lossy DNG and Render to DNG 4–8 h; HLG 3–5 h; catalog import depth 8–12 h).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-25 | minor | HDR gain map JPEG read 🟡 (the reader and `apply` exist and are tested, import never calls them: a gain map JPEG or iPhone HEIC opens as its SDR base); PQ / HLG HDR read: PNG and HEIF `nclx` (cICP 16 / 18) and lossless PQ JPEG XL decode to linear light, SDR white at 203 cd/m² (they were read as sRGB, or tone mapped down to 255 cd/m²); PQ / HLG write row corrected (HLG AVIF and PQ PNG exist) |
| 2026-10-13 | minor | PSD write ✅ (it landed with Edit in PhotoCraft, #953; the row still said the round trip wrote TIFF only); the Edit In round trip brings back an editor's Save As beside the file. Remaining hours without PSD write |
| 2026-10-13 | patch | JPEG write (#1373): Huffman tables made for each image, 12–14 % smaller files at the same pixels; statuses unchanged |
| 2026-10-12 | minor | Render to DNG: "Rendered DNG" export format (linear 16-bit float DNG of the rendered photo) |
| 2026-10-12 | minor | Lossy DNG export (JPEG tiles, `dngCompression: "lossy"`); PSD write had already landed (#953), so it leaves the remaining hours |
| 2026-10-10 | major | Created from the code and the installed Lightroom's document types |
| 2026-10-10 | minor | JPEG XL export (#247): JXL write ✅ (native only), PQ JPEG XL HDR |
| 2026-10-12 | minor | Render to DNG: "Rendered DNG" export format (linear 16-bit float DNG of the rendered photo) |
| 2026-10-12 | minor | Photo ▸ Render to DNG: rendered DNGs written next to the originals and stacked with them |
| 2026-10-12 | minor | HEIC / HEIF and AVIF: EXIF and XMP read from the container, without the decoder (#693); statuses unchanged |
