# Map and places

LightCraft reads the GPS position of your photos, shows them on a **Map**, and understands **place names** in
the search field: type `madrid` and you get the photos taken in Madrid. Everything about *where* a photo is
runs on your computer; only the map's background pictures come from the internet, and that can be switched off.

## Where positions come from

GPS is read at import, from the file (Exif GPS in JPEG, PNG, WebP, TIFF, DNG and raw files, Canon CR3, HEIC and
AVIF) and from XMP sidecars; stored in the catalog like any other metadata. Photos without GPS get one from a GPX
track log (Photo ▸ Auto-Tag from Tracklog…), from the recorded tracks the library keeps (`photo.autoTagTracks`,
[below](#recorded-and-estimated-paths)) or by typing it in the Info panel. `photo.setMeta gps` writes it.
An existing library needs no re-import: photos that had a position already have it.

## The Map view

View ▸ Map, the pin button in the bottom bar, or Show on Map in the Info panel. On a phone: "…" ▸ Map.

- Photos taken close together are one **marker**: a small picture of what it holds (up to four of its photos, the
  best-rated first, spread through the group) with a count. **Click or tap a marker to see its photos in the grid**,
  and only those; the "Only N photos" chip above the grid takes that off. ⌘ / Shift-click adds them to the selection
  instead. The context menu zooms to a marker's photos, selects them, or shows them in the grid.
- The map shows the photos of the current view and filter, and fits them whenever the search or filter changes:
  type `photos in portugal` in the search field, then open the Map.
- Drag to pan; scroll, pinch, double-click or `+` / `−` to zoom; the corner-brackets button fits all photos.
  **Go to a place** (top right) finds places by name.
- The search chip says how the text was understood: `Search: “photos in madrid in june” → Madrid, Spain · June`.

### The background

From the bottom: ocean and coastlines built into the app (Natural Earth, public domain: the map is never blank
and works offline at world and country scale), then map tiles as they arrive. A coarser tile stands in while a
sharp one loads. Tiles come from the server in **Settings ▸ Interface ▸ Map** (empty: OpenStreetMap's standard
tiles; any `https://…/{z}/{x}/{y}.png` address works), are saved on disk in the library's `map-tiles/` folder
(256 MB, oldest removed first; the browser's own cache on the web) and are credited on the map:
"© OpenStreetMap contributors".

- **Privacy.** Positions and place names never leave your computer. A tile request tells the tile server which
  area you are looking at (and your IP address), as any map does. Settings ▸ Interface ▸ Map ▸ *Load map tiles
  from the internet* off keeps the Map on the built-in map and tiles saved earlier.
- **Tile policy.** OpenStreetMap's tile servers are for light use (<https://operations.osmfoundation.org/policies/tiles/>).
  LightCraft identifies itself, caches, and asks for few tiles at once, which suits a personal library. If you
  deploy LightCraft for many people (the web build on a shared server), point the tile server setting at your own
  or a commercial one.

## Estimated routes

`map.route` works out a smooth path through the photos that have a GPS position *and* a capture time, even when
they were tagged by hand or by a camera and no track log exists. It is an estimate, not a recording, and its answer
says so (`estimated: true`): a screen should show it as one (dashed, say, and labelled).

- **Which photos.** The visible ones (the current view and filter, as `map.points` does), or the `ids` given.
- **Routes.** Photos are put in time order. A pause of more than `gapHours` (36) between two photos ends a route
  and starts the next, so a trip and the one after it are never joined by a line across the weeks between them.
- **Stops.** Photos taken one after another within `stopRadius` (150 m) of each other are one stop, with their ids.
- **Legs.** Consecutive stops are joined by a leg: its distance, the time between the last photo of one stop and the
  first of the next, the speed that implies and a guessed `mode`: `walk` (under 6 km/h), `bike` (under 20), `road`, or
  `flight` (at least 200 km at 150 km/h or more). The speed is a lower bound, since the person may have stopped on the way.
- **Lines.** A leg is a great-circle arc (a flight, a leg of `arcKm` (300) or more, or a stop beyond 80° of latitude)
  or a smooth curve through the stops (a centripetal Catmull-Rom spline in Web Mercator: between only two stops it
  is straight, and a longer trail bends smoothly through them). A line that crosses the antimeridian comes in two
  pieces, so a map never draws it across the whole world. The lines of one answer hold about 60 000 points at most;
  with more legs than that they get coarser. `detail` caps the intervals of one leg.
- **Time zones.** A capture time is the camera's clock. With a recorded offset (Exif `OffsetTimeOriginal`) it is
  exact; without one, the zone of the photo's longitude, to the hour, stands in. That orders and paces photos across
  zones (how long a flight took) well enough for an estimate; `zoneEstimated` counts the photos it was used for.
- **Left out,** and counted in `skipped`: photos without a position, photos without a capture time (a date alone is
  too coarse to put a photo between two others) and ids that aren't photos of the library.

The estimate never changes a photo, and is not a track log: `photo.autoTagTracklog` takes positions from a real
track, and the two shouldn't be mixed up (an estimated route fed back as a track would only repeat the photos).
The code is `crates/geo/src/route.rs` (the maths, on plain positions and times) and `crates/engine/src/map.rs`
(photos in, JSON out).

## Recorded tracks

Where someone actually went: recorded by a phone (a native (non-Rust) frontend records the positions while a
tracking session runs and hands them over when it ends, `track.create`) or imported from a GPX file
(`track.import`). Unlike an estimated route, a track is a recording, and it is stored in the library and synced to the
person's other devices ([sync.md](sync.md#recorded-tracks)).

- **Points.** A point is a moment (milliseconds since 1970, UTC), a position, and optionally an elevation and the
  accuracy of the fix. A point can begin a new stretch of recording (reception was lost, or the recording paused):
  nothing is drawn or counted across that break. A GPX file's segments and tracks each begin a stretch.
- **Sessions and chunks.** A recording is a *session*, stored in *chunks* of at most 20 000 points and 24 hours, one
  item each of the shared document `tracks` (`tracks.json` in the library). The points are packed (columnar
  differences, deflated, base64): some 6 to 10 bytes a point, so a day of walking at a point every 5 seconds takes
  about 100 to 170 KB. A session's distance, climb and descent are the sums of its chunks' (the one step between two
  chunks counts in neither); climbs under 3 m are GPS wobble and don't count.
- **The same recording is the same session.** A session's id (`s` and 16 hex digits) is worked out from its points
  (their count, and the first and last moment and position), so importing a file again, or a phone handing the same
  recording over twice, adds nothing (`existing: true`).
- **Left out, and counted in `dropped`:** points whose time or position isn't real, that go back in time, or that
  repeat a moment. An elevation or accuracy that isn't real leaves only itself out. A point left out that began a
  stretch passes that on to the next one kept.
- **Only the name changes** once a track is stored. Renaming and deleting are not undo steps (a screen asks before a
  delete), and no `track.*` call goes into the command journal: a track says where someone was.
- **Limits.** A new track is refused with an error that starts `limit:` when the document would pass 16 MiB or 1500
  items, the chunks and the settings item (a sync server keeps 24 MiB and 2000 items a document; the margin is for
  what two devices add while apart). `docBytes` and `docItems` in the answers say where it stands.
  A track holds at most 2 000 000 points, and a GPX file read from a path is at most 256 MiB.
- **Privacy trim** (`tracking.settings`, off until turned on): what is exported can leave out the start and the end of
  a track that lie within a radius (300 m by default, 50 to 5000) of where it began and ended, often a home. It is
  applied to what is exported and to the path `map.path` draws ([below](#privacy-trim)) only, never to what is stored,
  and the setting syncs (the item `settings` of the `tracks` document).
- **Export.** `track.export` writes one or several sessions as GPX 1.1, as text or to a file (replaced whole, never
  half written), with the privacy trim when asked or set. `write_gpx(name, points)` in `lightcraft_meta` writes
  time-ordered track points as GPX 1.1 (one `<trkseg>` per run of equal `segment`, positions to 6 decimals, elevation
  to 1 decimal, UTC times with milliseconds only when present, points with an invalid position or a time outside
  years 1 to 9999 skipped), and `parse_gpx` reads the result back: an exported track imports as the same points.
- **Damaged files.** A `tracks.json` that can't be read whole is kept as `tracks.json.corrupt-<time>`, the library
  opens with the tracks that could be read and says so, and a new track is saved beside them.

The code is `crates/geo/src/track.rs` (packing, figures, chunks, simplifying for a map, the privacy trim),
`crates/engine/src/tracks.rs` (the document and its checks) and `crates/engine/src/cmd/track.rs` (the commands).

## Trips

A trip is a saved, named grouping of photos and paths with notes, linked to albums both ways: a native (non-Rust)
frontend shows trips, the core keeps and syncs them ([sync.md](sync.md#trips), where the documents and the album
references are described).

- **Its photos** (`trip.photos`) are the members of the albums it links (manual albums only) and, when `photosInRange`
  is on, the photos with a position captured inside its range (both ends included), each once, by capture time
  (photos without one last). Photos in Recently Deleted or hidden are left out, and so are Local records and virtual
  copies from the range. A capture time is read as `map.route` reads it: exact with its recorded zone, else in the zone
  of the photo's longitude; a photo without a position or a zone is read as UTC (for a trip's span), and one without a
  time (or with a date alone) is never inside a range, though an album still brings it.
- **Its tracks** are the recorded sessions it lists, or with `tracks: "range"` every stored session that overlaps its
  range (or, without one, the span of its photos). Its path is `map.path {trip}`.
- **From an album** (`trip.fromAlbum`): a trip that links the album, its range the span of the album's photos.
  **To an album** (`trip.toAlbum`): a new album with the trip's photos, one undo step; the trip doesn't link it.
- **Notes** (`trip.note.*`) have a moment, an optional place and an optional photo, and are listed by time. Deleting a
  trip deletes its notes. None of `trip.*` but `trip.toAlbum` is an undo step, and none goes into the command journal.
- For the native frontend's map, the core offers `Session::trip_photos`, `trip_tracks`, `trip_range` and
  `trip_overrides` (the modes given to estimated legs).

The code is `crates/engine/src/trips.rs` (the documents, their checks, the album references and a trip's photos)
and `crates/engine/src/cmd/trip.rs` (the commands).

## Recorded and estimated paths

`map.path` gives one path for a selection (the visible photos, `ids`, an `album`, or a `trip`;
`fromMs`/`toMs` narrow it to a time): **recorded** where a stored track covers the time, **estimated** (the legs of
[`map.route`](#estimated-routes)) everywhere else. A screen draws the two differently (solid and dashed); the answer
says which each piece is (`source`).

- **A trip** (`trip`) brings its photos (`trip.photos`), its range as the window, the sessions it lists (or, with
  `tracks: "range"`, those that overlap its span; a listed session this device doesn't have is skipped) and its
  `overrides`: the estimated leg that leaves within a minute of an override's `atMs` takes its mode (a mode this
  version doesn't know is ignored). What the call gives (`tracks`, `fromMs`, `toMs`) wins over the trip.
- **Which tracks.** `tracks: "auto"` (the default) takes the recorded sessions that overlap the window, or, where it
  is open, the time of the photos; a list of sessions takes those whole. Each is read once, one at a time.
- **Recorded pieces.** A track is cut into pieces wherever it pauses longer than `gapHours`, so no line crosses the
  days between. A break in the recording (lost reception, a pause, a chunk of the track that is missing or from a
  newer version) is a new line inside its piece: nothing is drawn across it, and its time counts as neither moving
  nor stopped. When two tracks cover the same time (a phone and a watch), the one that began first keeps it and the
  other keeps only what comes after, so nothing is drawn twice. A piece has its distance, duration, climb and
  descent from every point, its `lines` with a parallel list of `times`, and is simplified by shape to `maxPoints`
  (4000; a long stay gives up its points before a bend does, both ends and both sides of every break are kept);
  all the recorded lines of one answer hold 60 000 points at most.
- **Estimated legs fill the rest.** A recording replaces the estimate over its time: a photo taken while a track was
  recording sits on the track, and the legs run only through the photos taken between recordings (or before the
  first, after the last), from where one recording ends to where the next begins. An estimated leg's `from`/`to` name
  its stops, or are `null` where the leg meets a recording.
- **Stops.** For estimated parts, the stops of the estimate that hold photos. On a recording, a place where the track
  stays within `stopRadius` (150 m) of where the stay began for `stopMinutes` (10) or more: a red light or a photo is
  not a stop, lunch is. Photos taken during a recorded stop are listed with it.
- **Photos on the path.** Each photo with a position and a time is listed with its time and place (`t`, `lat`, `lon`)
  and the piece it is on (`piece`, and `at`: how far through that piece's time, 0 to 1). A photo taken while a track
  was recording is on that recording; a photo at an estimated stop is at the end of the leg that arrives there or the
  start of the one that leaves (or at the end of the recording the stop joins), whichever is nearer in time. A photo
  whose stop is on no piece (a route of a single stop) has `piece: null`.
- **Stats** are the sums of the pieces: distance (recorded and estimated), the time on the path, the moving time
  (the time not in a stop nor in a break of a recording), climb and descent (recorded only), photos and stops.
- **Bounded:** at most 5000 recorded pieces and 4 000 000 recorded points read per answer (more is refused with
  `limit: …`, naming fewer tracks or a shorter time); points and photos that aren't real are left out and counted.

Pieces come in time order and never overlap; lines cross the antimeridian as two, as `map.route`'s do; every time is
seconds since 1970 (UTC). The maths is `crates/geo/src/path.rs` (positions and times in, pieces out); the command is
`crates/engine/src/cmd/path.rs`.

### Privacy trim

A track often begins and ends at home. With the trim on (`tracking.settings` `trim` and `trimRadiusM`, shared by a
person's devices, or `privacyTrim: true | metres` in the call; 0 or `false` is off), `map.path` leaves out what lies
within the radius (on the globe) of where each **track** began and ended (everything before its first point farther
than that from the start, and likewise from the end), and the same for each **estimated route** at its ends that
are photos (an end that meets a recording is not a home). The photos taken in what was cut go too, and the stops,
pieces and stats follow. A track or route that never leaves the radius vanishes. `trim` in the answer says what was
removed: `{applied, radiusM, removed: {start, end (photos), routes (gone whole), points (positions cut)}}`. Only the
answer changes: what is stored, and what syncs, keeps every point.

### Auto-tag from the recorded tracks

`photo.autoTagTracks` gives photos without a position the place a stored track says they were taken at, as
`photo.autoTagTracklog` does from a GPX file and with the same parameters (`ids`, `offset`, `maxGap` 600 s,
`replace`, `dryRun`) and answers, plus the `track` (session) each place came from and `tracks`/`points`/`start`/`end`
for what was searched. Between two points of one stretch at most `maxGap` apart the place is interpolated; else the
nearest point within `maxGap` counts; never across a break, a missing chunk or from one session to the next. Where two
tracks cover a photo, an interpolated place beats a nearest one and a nearer point a farther one. One undo step. With
no stored track the answer is `no such track: none stored`.

## Searching by place

The search field (and `library.filter` `text`, smart albums, the CLI and MCP) understands:

| You type | It finds |
|---|---|
| `madrid`, `photos in Madrid`, `fotos en madrid` | photos taken in Madrid and its metropolitan area (by GPS), and photos whose city / state / country fields say so |
| `españa`, `spain`, `usa`, `reino unido` | a country, by its names in several languages: every photo whose nearest city is in it |
| `new york`, `alcalá de henares`, `rio de janeiro`, `東京`, `Москва` | multi-word names; accents and case don't matter; 34 000 cities with their alternate names |
| `madrid 2024`, `june`, `june 2024`, `paris or rome` | combine with a year, a month, "or"; filler words (`photos`, `taken`, `in`, `at`, `from`, `near`, `the`, `fotos`, `de`, `en`…) are ignored when other words are present |
| `place:madrid`, `city:`, `region:`, `country:` | the same, written as a field (`place:new_york`) |
| `near:40.4,-3.7,5` | within 5 km of a position (default 1 km) |
| `bbox:south,west,north,east` | inside a box (what the map shows) |
| `gps:no` | photos without a position |

A word that is also a place ("nice", "reading") still matches text, so natural language only ever adds results.
Smart albums have a rule **Place (from GPS)**: `place contains madrid`.

How it works: `crates/geo` embeds a gazetteer built from [GeoNames](https://www.geonames.org/) (cities with 15 000
inhabitants or more, with regions, countries and alternate names; CC BY 4.0, see `NOTICE`). A photo's place is its
nearest city within 150 km; a city covers its metropolitan area (9–40 km by population). Photos in the open sea
or polar regions have no place. It's an approximation: a photo near a border can fall in the neighbouring
country, and a town under 15 000 inhabitants is "near" a bigger one. The derived place is shown in the Info panel
but never written to your files; the IPTC fields stay yours.

`cargo xtask geodata <dir>` rebuilds the two embedded files (`crates/geo/data/places.bin`, `land.bin`) from the
GeoNames dumps (`cities15000.txt`, `countryInfo.txt`, `admin1CodesASCII.txt`) and Natural Earth's
`ne_110m_land.shp` found in `<dir>`.

## Commands

| Command | |
|---|---|
| `view.map {lat?, lon?, zoom?, place?, fit?}` | show the Map; look at a position, a place by name, or fit the photos |
| `map.points {zoom?, cell?, bbox?, ids?}` | the visible located photos grouped as markers (what the map draws), for agents and scripts |
| `map.bounds` | the box around the visible located photos |
| `map.route {ids?, stopRadius?, gapHours?, arcKm?, detail?}` | an [estimated route](#estimated-routes) through the located photos by capture time: routes, stops, legs (distance, speed, mode) and their lines |
| `map.path {ids? \| album? \| trip?, fromMs?, toMs?, tracks?, privacyTrim?, stopRadius?, gapHours?, arcKm?, detail?, stopMinutes?, maxPoints?}` | [one path](#recorded-and-estimated-paths): recorded where a stored track covers the time, estimated elsewhere, with stops, the photos placed on it, stats and the privacy trim |
| `photo.autoTagTracks {ids?, tracks?, offset?, maxGap?, replace?, dryRun?}` | positions for photos from the [stored tracks](#auto-tag-from-the-recorded-tracks) by capture time; one undo step |
| `map.place {query}` | what a place name means (city, region, country) with the box that shows it |
| `photo.place {id?}` | a photo's GPS position and the place it is in, in words |
| `track.create {points? \| data?, name?, source?, device?, tz?}` | store a [recording](#recorded-tracks) (`[[tMs, lat, lon, ele?, acc?, seg?]]`, or a packed track) as a session: its chunks, what was left out, the document's size |
| `track.import {gpx? \| path?, name?, tz?}` | a GPX file as one session (every track and segment of it), as `track.create` plus the file's track and untimed point counts |
| `track.list {fromMs?, toMs?, chunks?}` | the stored sessions (start, end, points, distance, climb, box, chunks, `unreadable`), the document's size and limits |
| `track.get {session \| id, points?, maxPoints?, fromMs?, toMs?}` | one session (or one chunk), with its points on request: at most `maxPoints` (4000), both ends and both sides of every break kept |
| `track.rename {session, name}`, `track.delete {session}` | rename (empty clears) or delete a session, on every device through the sync |
| `track.export {session \| sessions, privacyTrim?, name?, path?}` | sessions as GPX 1.1 text, or written to `path`; `privacyTrim` (`true`, `false` or metres) defaults to the setting |
| `tracking.settings {trim?, trimRadiusM?}` | the privacy trim for exports, read (no params) or set; shared by the person's devices |
| `trip.create {name, albums?, range?, photosInRange?, tracks?, cover?}`, `trip.fromAlbum {album, name?}` | a new [trip](#trips) (`limit:` past 500); answers as `trip.get` |
| `trip.update {id, …}`, `trip.link`, `trip.unlink {id, album \| ref}`, `trip.delete {id}` | change a trip (`null` clears a field), link or unlink an album, delete it with its notes |
| `trip.list`, `trip.get {id}`, `trip.photos {id}` | the trips (photos, albums and notes counted, the span), one trip with its linked albums resolved, a trip's photos |
| `trip.toAlbum {id, name?}` | a new album with the trip's photos (one undo step) |
| `trip.note.add {trip, text, tMs?, lat?, lon?, photo?}`, `trip.note.update`, `trip.note.delete`, `trip.notes {trip}` | a trip's notes (`limit:` past 1500), by time |
| `album.ref {id}`, `album.resolve {ref}`, `album.trips {id}` | an album's sync-stable reference, the album a reference names here (or `{id: null}`), the trips that link an album |

`ui.inspect` → `map` reports the camera and what the map drew; widget ids `map`, `marker:<photo id>`,
`map:zoomIn`, `map:zoomOut`, `map:fit`, `map:search`, `map:attribution`.

## Platforms

One implementation (`crates/ui-egui/src/panels/map.rs`) serves all of them; only fetching tiles differs:
desktop and iOS use the engine's HTTP client with the disk cache (`crates/engine/src/tiles.rs`); the web build
uses the browser's `fetch` (`apps/lightcraft-web/src/tiles.rs`). The gazetteer adds about 1.5 MB to the app
(the web bundle too).

## Not yet

Dragging photos onto the map to give them a position, saved locations, a recorded track or an estimated route drawn
on this app's Map (`track.get`, `map.route` and `map.path` give the data, `map.path` made for a native (non-Rust)
frontend's map; the desktop and web Map don't draw them yet), recording a track in the egui apps, showing trips in the egui apps, a filter bar tied to the visible map area, satellite or
terrain layers, and checking the Map in a browser and on an iPhone (it is tested headlessly on the desktop UI and
type-checked for wasm and iOS).
