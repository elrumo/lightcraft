# Lightroom catalog import

File → Import Lightroom Catalog… opens a Lightroom Classic `.lrcat` directly, or a Lightroom (cloud)
library's albums ([Lightroom albums](#lightroom-albums-albums-only)). Lightroom, Python, DNG conversion
and a C SQLite runtime are not required. The source database is read-only, including committed
`-wal` pages; a catalog that changes during the read is rejected. LightCraft owns subsequent edits.

File → Open Library… given a Lightroom catalog folder (or an `.lrdata` / `.lrcat-data` folder, or the `.lrcat`)
doesn't make a LightCraft library there: it offers this import instead, with the catalog found
(`crates/engine/src/lightroom_folder.rs`, issue #891). `lightcraft-cli --library` refuses those places too.

Equivalent commands:

```text
lightcraft-cli run --library "LightCraft Library" library.inspectLightroom path="Lightroom Catalog.lrcat"
lightcraft-cli run --library "LightCraft Library" library.importLightroom path="Lightroom Catalog.lrcat"
```

Import adds records to the currently open LightCraft library; it does not create a separate
folder named after the Lightroom catalog. The desktop result stays open until dismissed and
shows the destination library, the number of records added, missing originals and warnings.
**Show in Library** opens All Photos, clears filters and selects an imported record. Errors stay
readable in the same result window. If another dialog is open, the result waits until that dialog closes. The result
is also available through `library.importLightroom {"status":true}` during that session.

Original files are referenced in place. Ratings, picks/rejects, color labels, XMP metadata,
hierarchical keywords, regular collections/collection sets, smart collections, folder stacks,
virtual copies and mapped develop settings are imported. Missing originals remain catalogued for relinking, count as added records and are reported
separately from failures reading files that are present. Known-missing originals are not probed
again. Previously browsed Local records join All Photos even when their originals are offline;
their promotion is counted once and undone together with the catalog import.
Virtual copies count as added records too; reimport reports zero additions when all
records already exist. Existing LightCraft
edits are preserved by default; `updateExisting=true` explicitly replaces them. Persistent import
identity prevents reimport from duplicating virtual copies and collections. One undo step reverses
catalog changes; any saved recovery archives remain available.

Before changing records, the importer attempts to save source image records, decoded XMP,
verbatim develop settings, history, snapshots and collection content to compressed
`Interop/lightroom-import-N.lca` recovery archives in the LightCraft library. Archiving is
best-effort: a size limit or write failure produces a warning and does not prevent photo import.
Unchanged source data reuses its existing archive. Opaque SQL BLOBs are skipped. Each archive is
capped at 32 MiB (uncompressed and on disk), with at most eight archives and 128 MiB total
retained. Oldest managed archives are retired; legacy `.json` archives remain untouched.
`Interop/lightroom-index.json` records imported identities. Source catalogs and originals are
never overwritten.

Native import reads referenced database pages on demand (at most 64 KiB per page), rather than
loading the whole `.lrcat`, so its size is not bounded by memory (a Lightroom cloud catalog is often
over a gigabyte). Database files are capped at 16 GiB and 16,777,216 pages; the committed WAL is read
whole and capped at 4 GiB. Tables still have a 256 MiB encoded-payload budget and one million rows,
individual records are capped at 64 MiB, and XMP packets at 16 MiB. These limits can still reject
large catalogs with dense tables; file size alone no longer determines the memory allocation.
Referenced photo sizes and photo counts are not part of these limits.
The open file and source path are checked again after all tables are read. Close Lightroom before
importing: metadata checks detect ordinary concurrent changes, but are not SQLite snapshot isolation.

The in-tree, read-only Rust SQLite B-tree reader follows SQLite's public file-format
specification; it introduces no dependency or C runtime.
Optional history, keyword and collection tables that exceed limits are skipped with warnings;
required photo tables still report errors. Unrelated unsupported schemas are ignored. A needed
WITHOUT ROWID or virtual table produces an explicit error, handled as a warning when optional.

Develop settings reuse the existing XMP/preset mapper. Supported sliders, curves and supported
mask structures remain editable, but this is approximate rendering: camera profiles, Adobe AI
models, some masking/retouch fields and process-version algorithms are not reproduced. Unmapped
fields are reported. Lightroom's saved Upright correction isn't read from catalogs yet: how a catalog stores it, and
whether the catalog's orientation agrees with the decoded file, haven't been checked on a real catalog, so the mode
is solved again and the import reports `Upright`, whether it is in the catalog's develop settings or its XMP (XMP
sidecars keep it: [xmp-interop.md](xmp-interop.md), *Saved
Upright geometry*). Lightroom's `ProcessVersion` is not mapped: LightCraft has one rendering. When a recovery archive is saved, it retains source settings/history/snapshots
and original smart-collection rules. Smart collections become smart albums when every rule has an exact equivalent: rating, pick
flag, color label, has-adjustments, ISO, focal length and aperture comparisons; text rules on
keywords, filename, camera, lens, title, caption, creator, copyright, location fields and file
format (contains all / any / none, begins/ends with, is, empty); and nested match-all / any /
none groups. Any other rule (collection, dates, metadata status, …) keeps the collection as a
regular album with its current membership, and the report names the rule. On reimport an album keeps
its kind: an existing smart album keeps its current (possibly edited) rules, and a regular album
made by an earlier import stays regular and gains new members, with a warning (delete it and
reimport to get a smart album). The report counts albums by their resulting kind:
`smartAlbums`, `smartAsStatic`, `smartKept` (existing smart albums left as they were) and `stacks`. Folder stacks become stacks in Lightroom's order
and collapsed state; photos already in a stack keep it, so reimport doesn't duplicate them. Archived history and snapshots are source data, not native LightCraft history yet.
A Custom white balance on a raw LightCraft develops relative to its as-shot look (ARW, NEF, RW2,
RAF, CR3, CR2, PEF, SRW; Lightroom names them all `RAW`) can't be taken as Kelvin: the catalog keeps no
as-shot white for Custom photos, so the photo stays As Shot and `Temperature, Tint` are reported
(issue #510; a DNG keeps the Kelvin value, and an XMP packet with `crs:AsShotTemperature` is
shifted by the same mired difference, see [xmp-interop.md](xmp-interop.md)). A raw whose camera has
measured colour matrices (`lightcraft_raw::spectral`: ILCE-7M3, D850, EOS 5D Mark III…), or whose white-balance
presets give a Kelvin scale (`lightcraft_raw::kelvin_presets`: ILCA-99M2, ILCA-77M2…), edits in Kelvin
and keeps the Kelvin value too (issue #730) — when its file was found and probed, since the camera is
read from the file, not from the Lightroom catalog; a missing file is treated as a relative-scale raw
until Reload reads it.
Lightroom's `-999999` deferred-adjustment sentinel is omitted from both catalog and XMP mappings;
it is reported, included in any saved archive, and never clamped into a real slider value. Deferred Adobe Auto Tone
is not evaluated by the importer; LightCraft's Auto control remains available after migration.

New photo records are moved into staged operations; only masters needed for virtual copies
retain a second owned record while the import commits.
Applying catalog operations collects affected source-cache IDs before consuming the operations,
avoiding another full operation copy. Full-strength develop patches reuse their JSON without
serializing an unused copy of the current settings.

Native desktop inspection/import runs on a cancellable background worker with progress. Only
prepared catalog operations commit on the owner thread; archive index writing stays on a worker.
Late results from a closed or switched library are rejected, including reopening the same path.
Ordinary edits during preparation remain intact. CLI commands complete synchronously using the
same preparation/commit pipeline; browser builds reject native catalog import.

Archive version 1 begins with the 12 bytes `LC-LRARCH\0\x01Z`, followed by zlib-compressed JSON.
Recovery readers must cap decompression at 32 MiB. JSON retains source identities, photo paths,
settings, decoded XMP, keywords, history, snapshots, collection definitions and membership.

The importer uses shared Rust code on macOS, Windows and Linux. When a root folder's recorded
path isn't a folder on this computer (a catalog copied from another machine), the importer uses
the path relative to the catalog that Lightroom records for root folders on the catalog's volume
(`relativePathFromCatalog`), when that is a folder, and reports the remapping as a warning. Copying
a project folder with its catalog inside it therefore keeps its originals linked. Other moved
originals stay missing; use LightCraft's missing-photo relinking tools to update their paths.

## Lightroom albums (albums only)

File → Import Lightroom Albums… (and the SwiftUI sheet's **Albums Only** switch) brings a Lightroom
catalog's albums and folders of albums into the open library, holding the photos the library
**already has**. No photo is added, and no rating, keyword or edit changes: the albums are made in one
undo step (`library.importLightroom {"path": …, "albumsOnly": true}`). On a library synced with
`lightcraft-server` the albums are ordinary album changes, so they reach the other devices; run the
import on any device of the library.

Two kinds of catalog give albums:

- **Lightroom (cloud)**: the desktop app keeps a copy of the whole cloud catalog on the computer, even for
  photos whose originals are only in the cloud: `~/Pictures/Lightroom Library.lrlibrary` (Windows:
  `%USERPROFILE%\Pictures\Lightroom Library.lrlibrary`). Pick the `.lrlibrary` (a Mac with Lightroom
  installed shows it as one file), its `<catalog id>` folder or its `Managed Catalog.mcat`. A library
  holds one catalog per account that signed in; the one changed last is read. Such a catalog only ever
  gives albums. Quit Lightroom first: a catalog that changes while it is read is refused. On a phone
  without a computer, copy `Managed Catalog.mcat` and `Managed Catalog.mcat-wal` into one folder and pick
  the folder. Lightroom mobile's own data can't be reached.
- **Lightroom Classic**: the `.lrcat`'s collections and collection sets (a smart collection becomes a smart
  album when its rules map, as in the full import, else an album with the photos it holds now).

The catalog is SQLite: table `docs` lists the documents, table `revs` holds each one's revisions as
MessagePack (decoded in tree, `crates/engine/src/msgpack.rs`). The reader
(`crates/engine/src/lightroom_cloud.rs`) reads every `album` (`collection`: an album,
`collection_set`: a folder), every `album_asset` (a photo in an album) and the `asset` documents of
the photos albums hold (`importSource.fileName`, `fileSize`, `originalWidth`, `originalHeight`,
`xmp.exif.DateTimeOriginal`, `captureDate`), looking each revision up by its rowid. Deleted documents,
albums of shared spaces and photos not in the catalog are left out.

Each Lightroom photo is matched to a library photo (not deleted, not only browsed, not a virtual copy;
`crates/engine/src/lightroom_albums.rs`), never guessed:

1. the same file (Classic: the path LightCraft references);
2. the same byte size and the same name or the same capture second (the same bytes, maybe renamed);
3. the same name (without extension), capture second and dimensions, either way round (a re-encoded copy,
   such as a photo exported from Photos).

Capture times are compared to the second without zone or fraction. When more than one library photo fits
(two copies of one file count as one) the photo is reported as **ambiguous** and left out; one that fits
none is **not in the library**. The result says how many albums and folders were made, how many photos went
into albums, and how many were matched, ambiguous or not in the library.

An album imported before is found again (the import index in `Interop/lightroom-index.json`), else an
album or folder of the same kind with the same name in the same folder (a smart collection's album among
the smart albums, any other among the regular ones), so importing twice, or on a second device, merges
the photos it lacks instead of making a copy. This applies to the full Classic import's collections too.
