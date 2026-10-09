# What's new in LightCraft

## October 2026

### Self-hosted sync
- **Share an album with a link.** Right-click an album ▸ Share Link: anyone you send it to sees a plain page of the
  album in their browser, with no account. The pictures are your photos as you have edited them, without location or
  camera details; the link can end after some days and can let people download the originals; you can copy or revoke
  your links in Settings ▸ Sync, and the server's admin can too.
- **Merge a library you already have.** Signing in to a server from a library that already has photos used to be
  refused. Tick *Combine with the photos already on the server* and the two are merged: photos are matched by what they
  are, edits merged, albums and stacks kept, nothing removed on either side.
- **Your presets and sets follow you.** Export, metadata, filter and curve presets, colour-label and keyword sets, LUT
  profiles and your import defaults now sync between your devices (the watched folder, cache size and other settings
  that belong to one computer don't).
- **A limit for downloaded originals** (Settings ▸ Sync ▸ Originals limit): the ones you haven't used for longest are
  deleted from the device when it goes over, and come back when you ask; never one the server doesn't have, or one you
  are working on.
- **Uploads and downloads of originals resume** after a broken connection instead of starting again, and a download is
  checked against the photo before it is used.
- **XMP for Lightroom, written by the server (opt-in).** An admin can let the server write in a library folder: edits
  made on your devices then appear as `.xmp` sidecars beside the photos, and the photos you upload can be filed into
  an imports folder as year/date/name. The photos themselves are never changed.
- **The browser** works with a server that hosts the web build elsewhere (`--cors-origin`), asks the server to build the
  previews of what it uploads, and auto tone, white-balance picks and exports up to 2560 px work on a photo whose
  original isn't in the browser.
- **A sturdier server.** The HTTP layer closes stalled connections and bounds connections and requests (the health
  check always answers); the log it keeps for devices is compacted without making every other device reload.

### Sync
- **Sync status on the grid.** The cloud button (beside Select and "…" on the phone, in the top bar on a computer) opens
  a panel like Lightroom's: whether you're up to date, the photos being uploaded and files being downloaded ("12 of 56"
  and which), what your server is doing for you (scanning its library folders, building previews, indexing photos for
  search) and how much room it has left, with Pause and Sync Now. It's amber with a `!` when something needs you.
- **Signing in with photos of your own to a server that already has a library** no longer stops: LightCraft asks
  whether to **add your photos to the server's library** (photos both have are kept once; edits, ratings, keywords and
  albums come along), **use the server's library instead**, or **not sync**. Before anything changes, a copy of your
  library's catalog is kept in its folder. Nothing syncs until you choose. See `docs/sync.md`.
- Settings ▸ Sync on a phone shows the status, server address and errors in full, wrapped, instead of cut off with "…".

### Map and places
- **Map view** (View ▸ Map, or the pin in the bottom bar; on a phone, "…" ▸ Map): your photos where they were
  taken, grouped into thumbnail markers with counts, on desktop, in the browser and on iPhone. Pan, zoom with the
  wheel or a pinch, find a place by name, click a marker to zoom into it. It follows your search and filters.
  The map works offline with a built-in world map; detailed tiles come from OpenStreetMap by default (or a server
  you choose), are saved on disk, and can be switched off in Settings. See docs/map.md.
- **Search by place.** `madrid` or `photos in Lisbon in june 2024` finds the photos taken there, from their GPS
  position, with no internet and in several languages (`españa`, `Londres`, `東京`). Countries, regions, `or`,
  years and months work too; the search chip shows how the text was read.
- The Info panel names the place a GPS position is in (offline); Show on Map opens the Map at the photo.

### Enhance
- Super Resolution (Photo ▸ Enhance ▸ Super Resolution…): enlarges a photo to twice its width and height with an AI
  model (Nomos Uni SPAN 2×) and adds the result next to the original, stacked with it. The model is a 4.5 MB optional
  download (CC-BY-4.0, credited to its author) offered with a consent dialog the first time; nothing downloads
  without a yes. It enlarges the rendered, edited photo as a 16-bit sRGB TIFF (not the raw data), runs on the GPU on
  Mac and the CPU elsewhere, and takes minutes on large photos. It is also on the iPhone and iPad, in a photo's "…"
  menu. See `docs/super-resolution.md`.
- Settings → AI Models: every optional AI model in one place: what it is for, whether it is on this device or still
  to download (and from which host), the space it takes, its licence and credit. Download (after a question that
  names the size and licence), cancel, delete to free the space, or turn a model off. The first use of a feature
  that needs a model offers the download too. See `docs/ai-models.md`.

### iPhone and iPad look like iOS 26
- Floating controls are drawn as glass: the bars' buttons are glass circles and capsules (a photo's share, save and "…"
  share one), the tools float in a glass tab bar whose highlight springs to the tool you tap, the select bar and the +
  button float the same way, and toasts are glass capsules. Pressed, glass swells a little and lights up.
- Menus grow out of the button that opened them with a small bounce, have their icons on the leading side, and round
  their corners as iOS 26 does. Pages close with ✕ and confirm with a blue ✓, and while one is up the screen behind
  steps back with rounded corners, as iPhone sheets do.
- Sliders have iOS 26's capsule thumb, which turns into a glass lens while you drag it; switches have the wider track
  and a knob that springs across; segmented controls slide their highlight to the segment you tap. The library has a
  large title, and Settings' cards are rounder.

### Export on iPhone
- Sharing is a flow of its own, as in Lightroom's mobile app: the share button opens a sheet with a strip of your photos (tick the
  ones to send, or Select All) and round Share and Export As… buttons; Export As… has the file type, size, quality and watermark
  as simple pull-down fields, and More Options everything else the desktop dialog has. While photos are exported a card shows
  which one and how many, with Cancel.
- Save to Photos adds the exported photos to your photo library in one step (JPEG, PNG, TIFF or DNG; the system asks for
  permission to add photos the first time). The photo's top bar has a save button that does it with the last settings.

### Remove
- Content-aware Remove: painting over a distraction now fills it with texture synthesized from around it,
  continuing edges, horizons and patterns through the stroke, instead of healing from one copied spot. The spot's
  source still counts (it seeds the fill and is searched too), so Refresh Source (`/`) or dragging the source
  gives another result; Heal and Clone copy from the source exactly, as before. Rows or tiles seen in strong
  perspective can come out at the wrong angle; use Heal or Clone with a chosen source there.
- Photo Merge ▸ Panorama ▸ Fill Edges is content-aware too: the empty corners of a stitched panorama are filled
  with sky, ground and texture continued from the panorama instead of a smooth smear.

### Presets and profiles
- Import presets from other editors: XMP presets, classic `.lrtemplate` files, "DNG presets" from mobile apps and `.zip`
  bundles of any of these — whole folders at once, grouped by pack. Masks inside presets come along.
- Luminar looks: `.lmp` files and `.mplumpack` collections import as presets (grouped by collection); the sliders
  with a counterpart here come along, the rest is listed.
- 23 new built-in presets: Portrait, Landscape, Urban, Food, Seasons, Vintage and B&W toners.

### Reliability
- LightCraft no longer crashes at launch on Windows PCs whose Vulkan driver is broken (issue #136, e.g. some Intel UHD
  630 drivers): on Windows the window and GPU rendering use DirectX 12 only and never load the Vulkan driver unless
  asked to. `LIGHTCRAFT_GPU_BACKEND=dx12 | vulkan | metal | off` (or wgpu's `WGPU_BACKEND`, which GPU rendering
  ignored before) chooses the graphics backend; `off` renders on the CPU. The GPU now starts after the window
  is up, and only when Settings ▸ Performance ▸ Use the GPU for rendering is on; if LightCraft ever dies while
  starting the GPU, the next launch starts with GPU rendering off and says how to turn it back on.
- Exports and renders never write over a photo's original (issue #93): exporting into the photo's own folder with
  the same name and "Overwrite" (or Export with Previous repeating it), an exact output path from the control
  channel or MCP, a merge preview path or `lightcraft-cli render IMG.jpg -o IMG.jpg` is refused with a clear
  message, and the original is left byte for byte. Ordinary earlier exports are still overwritten when asked.
  Exported files are written to a temp file and then renamed into place, so a full disk or an unplugged drive
  never leaves a truncated file; the XMP sidecar of an "Original" export follows the "If file exists" choice too.
- Convert to DNG, Copy as DNG, Photo Merge and smart previews no longer write straight to the final file (issue
  #106): a DNG is checked against the raw data, written to a temp file, synced and read back before it gets its
  name (never replacing a file), and only then is the photo relinked or the raw copy removed — a failed write
  leaves no DNG and keeps the raw. Smart previews are written the same way; a damaged one (cut short by a crash or
  a full drive) no longer counts as built and Build Smart Previews replaces it.
- Import ▸ Copy verifies every copy, like Move (issue #96): each file is written as a new file, synced to disk and
  checked against the content read from the card. A copy that fails or differs is removed and reported as a failed
  import — so "import complete" means the copies are good before you format the card — and a name that is taken
  gets -1, -2… instead of being replaced.
- Faster exports and card imports, with the same protection for what can't be recreated (issue #134): exports,
  renders and screenshots are still written to a temp file and renamed into place, but no longer forced to disk
  one by one — they can always be exported again, and on a USB drive or a NAS that per-file sync dominated a large
  export. The catalog, XMP sidecars, settings, DNGs, merges, smart previews and the copies an import makes are still
  synced. Import ▸ Copy now checks each copy against the content hash taken while scanning the card instead of
  reading the card a third time (Move still compares byte for byte before it deletes a source); a file that changed
  on the card after Review Import is reported instead of being imported with stale details. Checking an export path
  against the library's originals no longer scans the whole library when nothing is at that path.
- Saving metadata to an XMP sidecar another application wrote no longer replaces it (issue #92): LightCraft merges its
  fields in and keeps the rest — e.g. that application's develop settings and edit history — byte for byte. A
  sidecar that isn't valid XMP is copied to `<name>.xmp.bak-<time>` first. With the default stem naming, a raw and a
  JPEG with the same name (`IMG_0001.CR3` + `IMG_0001.JPG`) no longer share one sidecar: the raw keeps `IMG_0001.xmp`,
  the JPEG uses `IMG_0001.JPG.xmp`.
- A save that fails part-way (a full disk, a network share that drops) no longer looks like a damaged catalog
  afterwards (issue #101): the partial write is cut off before LightCraft retries, so the next launch replays every
  change. Catalogs already holding such a fragment load in full. Quitting while the catalog log can't be written
  still saves your queued changes in the closing snapshot.
- The catalog has a format version (issue #102). Opening a library from an older LightCraft upgrades it; a library
  written by a newer LightCraft is refused with "this library was written by a newer version of LightCraft" and left
  untouched — older versions no longer read part of it as a damaged log. Once this version has opened a library,
  LightCraft 0.2.0 and older refuse it ("unsupported format … v2").
- The control port (`--control`) closes a connection as soon as it receives anything that isn't a JSON request
  (issue #94): an HTTP request from a web page can no longer carry a command in its body. Lines are capped at
  4 MiB and connections at 16.
- A sleeping NAS, a dropped network share or a USB drive spinning up no longer freezes the window (issue #104):
  whether originals are there is checked on a worker thread (grid thumbnails, the Info panel, the photo menu, Missing
  Photos); importing (also by drag and drop), Find Missing Photos, Build / Discard Smart Previews, the Rename preview,
  the Local folder tree and Auto Import read the disk on worker threads too. The import progress window has Cancel.
- Browser version (experimental), keeping a library safe (issue #107): File ▸ Back Up Library… downloads the catalog
  and every imported photo as one zip, and File ▸ Restore Library from Backup… brings it back (the current library
  is kept). A failed save (storage full) now shows the unsaved warning and is retried, a photo that can't be stored
  isn't added, a second tab shows a message instead of overwriting the first, `?reset` asks first, and the page says
  when the browser may evict the library. Hosting: the sample cache headers no longer mark the (unhashed) files
  immutable, and HOSTING.md describes the actual build.
- Canon CR2 photos from the EOS 7D, 50D, 60D, 550D, 600D, 1200D, 1300D, 5D Mark II and 1D Mark IV (and other
  models whose sensor starts on a green-blue row) no longer come out magenta (issue #85): the colour-filter
  layout is read from each file instead of assumed.
- Exports are never black because of the GPU (issue #78): a GPU render that runs out of device
  memory, exceeds the GPU's buffer limits, hits a driver error or reset, or comes back
  incomplete is redone on the CPU — the file is the same image either way. Work is sent to the GPU
  in short pieces so slow integrated GPUs aren't reset by their watchdog. `ui.inspect` → `perf`
  (`gpuReason`, `gpuFallback`), Help ▸ System Info and Settings ▸ Performance say why the GPU isn't
  used (e.g. a skipped software adapter such as llvmpipe) and why the last render fell back.
- The thumbnail cache only ever counts and deletes its own files (issue #98): a library opened on a folder that
  already has a `thumbs/` folder of other pictures no longer loses them when the cache is trimmed or cleared.
- Settings files are never quietly reset (issue #103): a damaged `prefs.json`, `presets.json` or `view.json` is kept
  as `<name>.corrupt-<time>` and you're told; one that can't be read (e.g. locked by another program) is left alone
  for the session instead of being overwritten with defaults. The app settings (`ui.json`, which remembers your
  library) are written atomically and saved as soon as you open another library, not only at quit. Quitting while
  changes couldn't be saved tries once more, then asks: Try Saving Again, Quit Anyway or Cancel.

### Library
- Photos in Recently Deleted can be restored from the app: right-click ▸ Restore (or Delete Permanently), also in the
  Photo menu. The filmstrip has the photo context menu too. Adding a file again that is in Recently Deleted no
  longer just says "duplicate skipped": it opens the side panel on Recently Deleted with the photo selected and says
  how to restore it or delete it permanently and import it afresh. (For a fresh start on a photo, Reset Edits,
  Cmd+Shift+R, keeps the photo and clears its edits.)
- Canon CR3 files show their full-size embedded JPEG (e.g. 6960 × 4640 on an EOS R6 Mark III) instead of the
  1620 × 1080 preview, and import with their metadata: capture time, camera, lens, exposure, GPS and XMP. Their raw
  data is not decoded yet, so they stay preview-only. For CR3s imported earlier, Photo ▸ Reload from Disk (now in
  the Photo menu and the photo context menu) picks up the full-size preview and fills in the camera metadata they
  were missing, without touching anything already set.
- Rename Photos never overwrites another photo when only the letter case changes (issue #95): on case-sensitive
  volumes (Linux, case-sensitive APFS) `img_1.JPG` next to `IMG_1.JPG` is a different photo and the renamed one gets
  `img_1-1.JPG`; on case-insensitive volumes the case change still goes through.
- Rename Photos reports files it could not move back after a failure (issue #105), e.g. when a network share drops
  mid-batch: the error lists them (old → new) and the library points at their new names (an undoable partial rename),
  so none shows as missing. Renaming one of a raw + JPEG pair copies their shared `IMG_0001.xmp` instead of taking
  it away from the other (issue #92). Find Missing Photos also finds renamed files by their content, prefers a content match
  over a same-name same-size look-alike, and skips (and reports) photos it can't tell apart instead of guessing.
- Rename Folder… and Move Folder To… (Local) can be undone and redone (issue #97): the folder moves back on disk with
  its photos and XMP sidecars, and the photos point at it again. If something now occupies the old place, the undo
  is refused and nothing is overwritten.
- Smart albums with a rule editor: match all / any / none, nested groups, 26 fields.
- Quick Collection and target album (B in the grid), keyword sets (⌥1–⌥9), colour-label sets.
- Colour-label filter with several labels at once; expandable folder tree in Local.
- Import: copy to any folder, by day / by month / one folder, rename on import, metadata preset, Copy as DNG.
- Import ▸ Move: photos go into the destination (with the same folders and renaming as Copy, e.g.
  `Photos/2026/20260114/20260114_001.jpg`) together with their XMP sidecars; each original leaves the card only after its
  copy is verified and in the library. Duplicates and files that fail stay where they were.
- Watched-folder auto import; Convert to DNG; Duplicate; Build Standard / 1:1 / Smart Previews.
- Export file names use the Rename Photos tokens ({title}, {seq:2}, {date:%Y-%m-%d}…), plus new {num}, {folder}, {lens}, {iso}, {rating}, {creator}.
- Copyright status, rights usage terms and copyright info URL in Info, metadata presets and exports.
- Auto-Tag from Tracklog: GPS locations for your photos from a GPX track log, matched by capture time.
- A change that can't be saved to disk (full or unplugged drive) is no longer silent: the command reports
  "saved in memory but not written to disk", the top bar shows a warning, and LightCraft keeps retrying until the
  save goes through.
- Smaller, faster catalogs: photos you only looked at in Local (never added, rated or edited) are forgotten once
  their folder has not been browsed for 30 days — your files and sidecars stay, and browsing the folder shows them
  again. Change the period (or turn it off) in Settings → Performance.
- A library is open in one program at a time (issue #99): opening a library that LightCraft or `lightcraft-cli` already
  has open — on this computer or another one sharing the folder — says who has it ("already open in LightCraft (process
  123 on studio-mac)") instead of letting both write and silently drop each other's edits. A crash never leaves the
  library locked: the lock is the operating system's and goes away with the program.
- If your library can't be opened at launch (open in another program, unreadable, on a drive that isn't connected,
  written by a newer LightCraft), LightCraft says so and why, and offers Try Again, Choose Another Library…, Continue
  Without Saving and Quit (issue #100). It no longer quietly starts a demo session that looked like a reset library and
  lost everything at quit; a temporary session shows a banner the whole time and never writes to your library.

### Editing
- AI masks with SAM 3 (Object and Describe in the Masking panel): click an object to select it (⌥-click leaves a
  part out), or type what to select ("sky", "the red car", "car, road"); both combine with other masks, have an
  Edge setting, and get a sharper zoomed-in pass in the background. The model runs inside LightCraft in pure Rust
  and never freezes the window. It is optional and not part of LightCraft (Meta's SAM License, about 3.4 GB): the
  first time you use an AI mask, LightCraft asks before downloading it, shows the progress, can cancel and resume,
  and checks the file before using it. Masks keep their selection, so they render and export without the model.
- Auto Sync: edits apply to every selected photo. Auto B&W mix. Automatic versions.
- Colour-range masks: click the photo to sample. Luminance ranges: range bar, smoothness, luminance map.
- ⌘-drag to straighten, ⇧G Guided Upright, a grid while transforming.
- Nikon NEFs start from a colour and tone look fitted to the camera's own JPEG, as Sony ARWs do, instead of a muted,
  greenish neutral rendering (issue #150); white balance is adjusted relative to the as-shot look. 12-bit NEFs
  (e.g. D750, D780, D850, D7500, Z 50) no longer render nearly black or with crushed shadows: their black level was
  read in the wrong units.

### Viewing and sharing
- Slideshow, second window, All Metadata, System Info.
- Edit in External Editor (⇧⌘E): a 16-bit TIFF copy, stacked, refreshed when you come back.
- Lossy DNG files and Smart Previews open as raw photos.
- Compressed Nikon NEFs (lossless and lossy compressed, 12- and 14-bit — e.g. D3200, D5100, D7000, D750, D850, Z 50)
  now develop from the raw data instead of the camera's embedded JPEG, so a B&W or other picture style set in the
  camera no longer gets baked in. Photos already imported as "preview only" switch over on Reload. (Files that
  Nikon splits into two differently compressed halves still use the preview for now.)
- Sony ARWs from before about 2017 (RX100, RX100 II–V, RX10, NEX, SLT, ILCE-6000, A7 / A7 II / A7R II and their
  siblings) no longer open bright green (issue #148): their as-shot white balance and black level are read from the
  file (Sony stores them only in scrambled maker-note data on these bodies), the few columns of padding at the right
  edge are cropped away, and "12-bit uncompressed" files are no longer clipped. The RX100 series renders much closer
  to the camera's own JPEG.
