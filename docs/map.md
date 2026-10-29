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

`ui.inspect` → `map` reports the camera and what the map drew; widget ids `map`, `marker:<photo id>`,
`map:zoomIn`, `map:zoomOut`, `map:fit`, `map:search`, `map:attribution`.

## Platforms

One implementation (`crates/ui-egui/src/panels/map.rs`) serves all of them; only fetching tiles differs:
desktop and iOS use the engine's HTTP client with the disk cache (`crates/engine/src/tiles.rs`); the web build
uses the browser's `fetch` (`apps/lightcraft-web/src/tiles.rs`). The gazetteer adds about 1.5 MB to the app
(the web bundle too).

## Not yet

Dragging photos onto the map to give them a position, saved locations, a track log or an estimated route drawn on
the map (`map.route` gives the data; no screen draws it yet), a filter bar tied to the visible map area, satellite or
terrain layers, and checking the Map in a browser and on an iPhone (it is tested headlessly on the desktop UI and
type-checked for wasm and iOS).
