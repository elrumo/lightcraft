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

- Photos taken close together are one **marker**: the best-rated photo's thumbnail with a count. Click a marker to
  zoom into what it holds (a single photo is selected, double-click opens it). The context menu zooms to a
  marker's photos, selects them, or shows them in the grid.
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

Dragging photos onto the map to give them a position, saved locations, a track log drawn on the map, a filter
bar tied to the visible map area, satellite or terrain layers, and checking the Map in a browser and on an
iPhone (it is tested headlessly on the desktop UI and type-checked for wasm and iOS).
