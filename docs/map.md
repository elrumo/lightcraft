# Map and places

LightCraft reads the GPS position of your photos, shows them on a **Map**, and understands **place names** in
the search field: type `madrid` and you get the photos taken in Madrid. Everything about *where* a photo is
runs on your computer; only the map's background pictures come from the internet, and that can be switched off.

## Where positions come from

GPS is read at import, from the file (Exif GPS in JPEG, PNG, WebP, TIFF, DNG and raw files, Canon CR3, HEIC and
AVIF) and from XMP sidecars; stored in the catalog like any other metadata. Photos without GPS get one from a GPX
track log (Photo ▸ Auto-Tag from Tracklog…) or by typing it in the Info panel. `photo.setMeta gps` writes it.
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
  applied to what is exported only, never to what is stored, and the setting syncs (the item `settings` of the
  `tracks` document).
- **Export.** `track.export` writes one or several sessions as GPX 1.1, as text or to a file (replaced whole, never
  half written), with the privacy trim when asked or set. `write_gpx(name, points)` in `lightcraft_meta` writes
  time-ordered track points as GPX 1.1 (one `<trkseg>` per run of equal `segment`, positions to 6 decimals, elevation
  to 1 decimal, UTC times with milliseconds only when present, points with an invalid position or a time outside
  years 1 to 9999 skipped), and `parse_gpx` reads the result back: an exported track imports as the same points.
- **Damaged files.** A `tracks.json` that can't be read whole is kept as `tracks.json.corrupt-<time>`, the library
  opens with the tracks that could be read and says so, and a new track is saved beside them.

The code is `crates/geo/src/track.rs` (packing, figures, chunks, simplifying for a map, the privacy trim),
`crates/engine/src/tracks.rs` (the document and its checks) and `crates/engine/src/cmd/track.rs` (the commands).

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
| `map.place {query}` | what a place name means (city, region, country) with the box that shows it |
| `photo.place {id?}` | a photo's GPS position and the place it is in, in words |
| `track.create {points? \| data?, name?, source?, device?, tz?}` | store a [recording](#recorded-tracks) (`[[tMs, lat, lon, ele?, acc?, seg?]]`, or a packed track) as a session: its chunks, what was left out, the document's size |
| `track.import {gpx? \| path?, name?, tz?}` | a GPX file as one session (every track and segment of it), as `track.create` plus the file's track and untimed point counts |
| `track.list {fromMs?, toMs?, chunks?}` | the stored sessions (start, end, points, distance, climb, box, chunks, `unreadable`), the document's size and limits |
| `track.get {session \| id, points?, maxPoints?, fromMs?, toMs?}` | one session (or one chunk), with its points on request: at most `maxPoints` (4000), both ends and both sides of every break kept |
| `track.rename {session, name}`, `track.delete {session}` | rename (empty clears) or delete a session, on every device through the sync |
| `track.export {session \| sessions, privacyTrim?, name?, path?}` | sessions as GPX 1.1 text, or written to `path`; `privacyTrim` (`true`, `false` or metres) defaults to the setting |
| `tracking.settings {trim?, trimRadiusM?}` | the privacy trim for exports, read (no params) or set; shared by the person's devices |

`ui.inspect` → `map` reports the camera and what the map drew; widget ids `map`, `marker:<photo id>`,
`map:zoomIn`, `map:zoomOut`, `map:fit`, `map:search`, `map:attribution`.

## Platforms

One implementation (`crates/ui-egui/src/panels/map.rs`) serves all of them; only fetching tiles differs:
desktop and iOS use the engine's HTTP client with the disk cache (`crates/engine/src/tiles.rs`); the web build
uses the browser's `fetch` (`apps/lightcraft-web/src/tiles.rs`). The gazetteer adds about 1.5 MB to the app
(the web bundle too).

## Not yet

Dragging photos onto the map to give them a position, saved locations, a recorded track or an estimated route drawn
on the map (`track.get` and `map.route` give the data; no screen draws them yet), tagging photos from the stored
tracks (only from a GPX file so far), recording a track in the egui apps, a filter bar tied to the visible map area, satellite or
terrain layers, and checking the Map in a browser and on an iPhone (it is tested headlessly on the desktop UI and
type-checked for wasm and iOS).
