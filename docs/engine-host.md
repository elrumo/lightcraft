# The engine host (native frontends)

`lightcraft_apphost::host` (`crates/apphost/src/host`) runs the whole core behind a JSON message protocol, for a
frontend that is not Rust. The egui apps call `apphost` directly; a native app reaches the same code through this host.

- **One engine thread.** `Session` is `Send` but not `Sync`, so one thread (`lc-engine`) owns it and runs every command,
  render request and background pump, as the egui app's UI thread does. The frontend never waits on a lock held by
  background work.
- **The host drives the background work.** Between requests the engine thread does what the egui app's frame loop does:
  renders, saving, the activity stack, the self-hosted sync (`apphost::sync`), the face scan's pace (`apphost::faces`),
  exports (`apphost::export::Batch`) and imports (`apphost::import`), all shared with egui.
- **Messages, not calls.** The frontend sends JSON requests and reads JSON events from a queue it polls on a thread of
  its own. Commands go through `Session::execute`, the entry point every frontend uses, so a frontend gets every engine
  command without binding code per command.
- **Pixels are leases.** A render's RGBA8 buffer stays in Rust until the frontend takes it by its token, and is released
  when the last copy of the picture goes. The engine keeps only the render's facts (key, size, histogram); a picture
  nobody took is dropped when a newer one for its slot arrives, and untaken pictures are capped at 256 MB.
- **The frontend holds presentation only.** No document logic, no edit geometry, no image math. Logic it needs that
  still lives in `ui-egui` moves down first (*Extraction rule*).

## Extraction rule

When a native frontend first needs logic that sits in `crates/ui-egui`, the same commit moves it down and switches egui to
it; it is never written twice.

| What | Where it goes |
|---|---|
| Geometry and edit math (crop handles, mask handles, spot pins, curve points, the canvas ↔ image mapping) | `geom`, `develop`, `color` |
| Behaviour of the session (a rule that changes the document) | an engine command |
| Threads, render scheduling, background pumps (import, export, sync, faces…), post-command policy | `apphost` |

A native frontend keeps view state and view-transform math only (fit, pan clamping, zoom anchor). Moved so far: the render
scheduler (`apphost::render::Scheduler`, egui's `Renderer` is it plus egui textures), the thumbnail request rules
(`Scheduler::request_thumb`), headless Export Now (`apphost::export`, shared with the MCP server) and the mobile start-up
(`apphost::startup`, shared with the egui iOS app).

## Protocol

Requests are `{"id", "method", "params"}`; each gets a `reply` event with the same `id` (`result`, or `error: {code,
message}` where `code` is the engine's error kind — `unknownCommand`, `disabled`, `badParams`, `notSaved`… — or the
host's: `badRequest`, `unknownMethod`, `panic`, `closed`, `timeout`).

| Method | Params | Does |
|---|---|---|
| `engine.execute` | `{command, params}` | any engine command, and `app.export` (Export Now: on a worker with an activity row, ✕ stops it; answered when it ends, `cancelled` when stopped; `remember: false` keeps it from being what Export with Previous repeats: a photo dragged into another app); `library.inspectLightroom` / `library.importLightroom {path, updateExisting?}` likewise (`apphost::lightroom`, `path` the `.lrcat` or a folder holding it; `{status: true, cancel?}` answers at once); `enhance.superRes` runs the same way (Enlarging; answered with the new photo, or error `cancelled`); `library.browse {path, subfolders?}` too (`apphost::browse`: the grid shows the folder at once, its new files are probed on a worker and join it as Local records, then changed sidecars are read again; answered with `library.browse`'s result when done, `cancelled` when another folder is chosen or ✕ stops it) |
| `import.scan` | `{paths}` | the files and folders for the import review, scanned on a worker (answered when done: `{candidates}`, each with `inTrash`) |
| `import.start` | `{paths \| indexes, …library.import params, albumName?}` | imports in the background in batches (one undo step; answered when done with what it added) |
| `engine.commands` | | every command with whether it can run now and why not |
| `host.state` | | the state below |
| `host.memory` | | what memory the engine holds: `engine` (its caches, `library.memory`) and `views` (the views' stage caches, CPU and GPU, and the pictures they show); `left`, the bytes iOS still lets the app use before it ends it (`null` elsewhere) |
| `host.uiCommands` | | egui's UI-only commands (`apphost::ui_commands`): `[{id, label, shortcut, menu}]`, for a menu bar and shortcut keys |
| `host.local` | `{roots, hidden, browseRoot?, command?: addRoot \| hide \| restoreHidden, path?}` | a Mac's Local places (`apphost::local`, shared with egui's sidebar): the built-in ones (Pictures, Desktop, Downloads, Home, the disks in `/Volumes`), the kept folders (`roots`, the app keeps them: Favorites) and the folder browsed; after the edit: `{roots, hidden, places: [{label, path}], owner, browseRoot, current}` |
| `host.local.subfolders` | `{path}` | a folder's subfolders (not hidden ones), listed on a thread of its own (a network share can take long): `[{name, path}]` |
| `host.sidebar` | `{layout?, command?, params?}` | the albums sidebar's sections after one of egui's `view.sidebarSection` / `view.sidebarMoveSection` / `view.sidebarReset` (`apphost::sidebar`) on `layout` (the app keeps it): `{layout: [{id, hidden, collapsed}], shown}` |
| `host.lifecycle` | `{event}` | `resignActive`, `background` (save, pause the GPU, free memory), `active`, `memoryWarning`, `terminate` |
| `host.merge.preview` | `{command: merge.hdr \| merge.panorama \| merge.hdrPanorama, ids?, options?}` | Photo Merge's preview on a worker (`apphost::merge`, shared with egui's dialog): the picture in slot `merge`, then the answer `{info, slot}`; a newer preview answers the older `cancelled` |
| `host.merge.start` / `host.merge.last` | `{command, ids?, options?}` | the full merge in the background (activity row, ✕ stops it); answered `{started}` at once, a message when it ends; `last` uses the options last merged with |
| `host.merge.lastOptions` | `{command}` | the options of the last merge of that kind (kept in the library preferences; the defaults the first time): what the merge dialog opens with |
| `host.merge.cancel` | | drops the preview being drawn (the dialog closed) |
| `host.editIn` / `host.editIn.check` / `.originals` / `.claim` | `{ids?, mode?, format?, colorSpace?, bitDepth?}` / / `{ids?}` / `{paths}` | Photo ▸ Edit In (`apphost::external_edit`, the host half egui keeps in `ui.json`): `host.editIn` runs `photo.editExternal` and keeps the files it hands over in `external-edits.json` in the library folder, answering `{mode, copies: [{path, id, original}]}` for the app to open in the editor (image files with no `mode` answer `{ask: true}`: the Edit Photo choice); `.check` brings back what the editor saved (`photo.checkExternalEdits`; the host also runs it when the app becomes active, with a message saying what came back); `.originals` lists the original files themselves for Open Original in Other Editor; `.claim` takes files shared into the app that are named as a watched copy (a phone cannot hear an editor save) back to it |
| `host.exportPrevious` | | Export with Previous: the last export's options for the photos chosen now, on a worker like `app.export` |
| `host.admin.signIn` / `.call` / `.search` / `.signOut` / `.session` | `{server, user, password}` / `{method, path: [segment…], body?, server?}` / / / | the sync server's admin API (`/api/admin/*`, what its web console does) for Settings ▸ Server Administration (`apphost::host::admin`): the admin session (not the Sync device's: the server keeps them apart) lives in the host, in memory; requests run on a worker thread and are answered when the server has; path segments are percent-encoded there; without a session only `state` and `setup` work (against `server`); a `401` on a signed-in call ends the session (error `unauthorized`); a `412` (the shared presets changed by someone else first) is error `conflict`; `.search` is `GET search` with `progress` added, what its numbers mean for the Processing screen (whether the worker is at each index and on whose photos, the time left, each user's time left, waiting and failed counts and last problem), worked out as the web console does (`apphost::host::admin::search`) |
| `render.thumb` | `{photo, edge, priority?}` | a grid thumbnail (embedded raw previews first; cheap to repeat). Ask once, when the cell appears: while `render.retain` lists the photo and it has no picture, the host asks again every tick (a synced photo's small preview downloads only while asked for, and its thumbnail renders once it has landed) |
| `render.view` | `{photo?, width, height, draft?, before?, preset?, replacing?, amount?, applyCrop?, overlay?: {kind: mask, id, view?, color?, opacity?} \| {kind: allMasks, view?, color?, opacity?} \| {kind: spots, threshold?} \| {kind: pointColor, index} \| {kind: hdr} \| {kind: depth}, proof?: {space, destWarning?, displayWarning?}, zoom?: {drawn, visible: [x0, y0, x1, y1]}}` | the loupe's picture (`main`; a stand-in in `preview` until it is there; the reply's `key`, a string, is the render the picture will be, its image event's `key`: Lens Blur's "Rendering" badge waits for it), with a diagnostic overlay (the selected mask, Visualize Spots…); zoomed in (`drawn`: the photo's long edge as drawn, px; `visible`: the part on screen, as fractions) also the window on screen at the zoom scale (`region`, its image event's `window: {full: [w, h], x, y, w, h}` says where it goes; planned by `apphost::region`); with `preset`, the photo with that preset, nothing committed (`hover`, the reply's `key` names its picture; `replacing`: over the photo as the last step's Undo leaves it): the Mac's preview while the pointer rests on a preset |
| `render.hdrDisplay` | `{on}` | the screen can (or can no longer) show the range above SDR white (the frontend tells at start and when it becomes active; `LIGHTCRAFT_HDR_DISPLAY=1\|0` forces it): an HDR edit's `main` and `region` pictures then render in HDR (CPU) and their `image` event carries `hdr: {token, width, height, peak}`, a lease of RGBA16F pixels (Display P3 encoded with the sRGB curve past 1, [hdr-display](hdr-display.md)); the 8-bit picture is then Display P3 too. Without it, or off, the SDR rendition as before |
| `render.prefetch` | `{photos, width, height}` | prepares the neighbours |
| `render.variant` | `{photo?, preset \| profile \| version \| edited: true, amount?, edge?}` | a preview of the photo with a preset, profile or version, or as it is (Compare, Survey, the filmstrip) (slot `variant:<key>`); a preset's is of the original photo (its import settings, framed as it is), so the Presets panel's pictures don't follow the edit |
| `render.face` | `{photo?, rect, edge?}` | a face, as People shows it (`rect` as `people.list` and `faces.person` give it; slot `variant:<key>`) |
| `render.candidate` | `{index, edge?}` | an import candidate's thumbnail (slot `import:<index>`) |
| `render.cancel` | `{slots}` | stops rendering for slots |
| `render.retain` | `{thumbs, max?}` | the thumbnails on screen (queued renders of others are dropped) |
| `ui.reply` | `{id, result \| error}` | the app's answer to a `uiRequest` |

Events: `reply`; `image {slot, photo, key, token, width, height, final, draft, quick?, ms, histogram?}` (slots
`thumb:<id>`, `thumbQuick:<id>`, `main`, `preview`, `before`…); `renderFailed {slot, photo, key, error}`; `state {state:
{catalogRevision, visibleGen, visibleCount, libraryGeneration, presetsGeneration, active, selectionCount, undo, redo,
interaction, activeMask, activeSpot, unsaved, sync?, faces?}}` when any of it changes (`presetsGeneration`: changes
with the preset list, including presets the sync brings from another device: the Presets panel reads `presets.list`
again) (`faces`: the face scan's `{pendingPhotos, indexedFaces}` while recognition is on) (`activeMask` / `activeSpot`: the Masking tool's
selected mask and the Remove tool's selected spot, wherever they were chosen) (`sync`: the server sync for the cloud icon, `{state, server, pending, uploads,
downloads, reading, error}` with `state` one of `signedOut`, `signingOut`, `conflict`, `paused`, `error`, `syncing`, `idle`; absent for a local library, and
after a sign-out has emptied it, together with a new `libraryGeneration`: a frontend then drops whatever names the old
library's photos, as it does for any other library; see [sync.md](sync.md#signing-out)); `activity {tasks}` (the activity stack); `message {text,
level}`; `copy {text}` (put it on the clipboard: a share link just made); `ask {dialog, params}` (show a dialog the
engine needs an answer from: `syncChoice`); `uiRequest {id, method, params}`; `closed`.

A slider drag: `develop.beginInteraction {label}`, then `develop.set {control, value}` with at most one in flight (the
newest value wins) and `render.view {draft: true}` right behind each (requests run in order: the draft renders as soon
as the engine has the value, without waiting for its answer to come back), then the last value, `render.view` for the
sharp picture and `develop.endInteraction` (one undo step: the picture renders while it is saved). Send each request
when it is due, not from a task that may start later: a value that arrives after `develop.endInteraction` is an edit
of its own, saved to disk on the engine thread while the sharp picture waits. The draft's size is learned per slider
(its `label`, `apphost::render::DraftBudget`). `LIGHTCRAFT_HOST_TRACE=1` prints every request the engine thread
handles (with its time) and every picture it sends, to stderr.
