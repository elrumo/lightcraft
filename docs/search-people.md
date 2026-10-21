# Search by description, text in photos, and People

Describe a photo in your own words — "a dog on a beach at sunset", "雪の山", "红色的汽车" — and LightCraft shows the
photos that look most like it, best first. It works on the desktop app, on the self-hosted sync server (so the web build,
iOS and your other computers can search without running a model), and from the command line and MCP.

**Nothing requires the model.** It is not part of LightCraft: it is downloaded only when you turn the feature on and agree
to its licence, and everything else works without it.

## Using it

- **App:** click **Describe** at the right end of the search field, type a description, press Return. The grid shows the
  best matches first (no date headers: the order is relevance) with a *Looks like: …* chip; empty the field, click the chip's
  ×, or turn the switch off to leave. The first time, the panel under the field shows what the model is, its size
  (about 1.5 GB), where it is kept and its licence, and downloads it only when you click **Download and Turn On**. Your photos
  are then indexed in the background (you can search the ones that are ready meanwhile).
- **A device with no model** (the web build, the egui iOS app, a phone that may use too little memory for it, or a desktop
  that didn't download it) searches through the sync server when
  the server can: the switch appears once the server says so, and the panel says how many photos the server has ready.
  An administrator turns that on once for everyone from the server's admin page (**Processing**), so no device downloads the
  model. If it is off or still downloading, the panel says so instead of offering a download it can't use (on a device
  that can't run the model).
- **This device only:** a desktop next to a server that can search answers from the server by default and offers no
  download. Tick **Search on this device only, not through my server** (or `vision.setLocalOnly`; saved with the library)
  to keep searching on this computer: the panel then offers the model's download, and no search text is sent to the
  server (sending the search data is its own, separate choice below). Unticking goes back to the server.
- **CLI / MCP / control channel:**

  | Command | What it does |
  |---|---|
  | `vision.model.status` | whether the model is installed and loaded, how many photos are indexed, the server's state, sharing |
  | `vision.model.download {acknowledged: true}` / `vision.model.cancel` | download (resumable, SHA-256-checked) or stop |
  | `vision.index {wait?, ids?}` / `vision.indexProgress` / `vision.indexCancel` | embed the photos that have no vector yet |
  | `library.search {q, limit?, wait?, source?}` | `source`: `local`, `server`, or `auto` (this device's index when it covers the library, else the server's; always this device's when `vision.setLocalOnly` is on). A one-word query that is part of a file name (`IMG_4008`, `img_4008.dng`) lists those photos first (`name: true`), on every device and source; the plain search field (`library.filter {text}`) matches file names too |
  | `vision.setLocalOnly {on}` | search on this device only, never through the sync server (saved with the library; no effect on a device that can't run the model) |
  | `vision.share {wait?}` / `vision.setShare {on}` | send this device's vectors (and text) to the server, once or whenever new photos are indexed |
  | `vision.setHelper {parts}` / `vision.helper` | the work this device does for its server instead of the server (`search`, `text`, `faces`, `sensitive`), and how it is going; see *A device as the server's processing unit* |
  | `vision.setText {on}` | also read and search the text printed in photos (off until turned on; saved with the library) |
  | `vision.setFaces {on}` / `vision.faces.download {acknowledged: true}` / `vision.faces.cancel` | find the people in photos (off until turned on and the user agreed to the models), and download or stop the face models |
  | `people.list {all?, limit?, wait?}` / `people.name {cluster, name}` / `people.show {cluster}` / `people.deleteData` | the people found (unnamed ones, and named ones with faces to confirm), name one, show their photos, forget every face |
  | `vision.text.download {acknowledged: true}` / `vision.text.cancel` | download (resumable, SHA-256-checked) or stop the text-reading models |

## How it works

A photo and a sentence are each turned into a vector by **SigLIP 2 B/16** (Google, Apache-2.0; multilingual); a search
compares the sentence's vector with every photo's and returns the closest. Each photo is embedded from a **neutral,
unedited rendering**, so editing a photo never changes its vector and the same file gets the same vector on any device.

Vectors are **derived data**: kept in `<library>/search/<model>.bin` (desktop) or `<data>/users/<name>/search/<model>.bin`
(server), keyed by the photo's content hash, stored as f16, never part of the catalog (which syncs whole to every device).
Delete the folder and they are computed again. Measured on an M-series laptop: 100k photos × 768 dimensions are searched
in about 90 ms and loaded in about 0.4 s; the model embeds a photo in about 40 ms on Metal and a sentence in about 20 ms;
loading the model takes a few seconds (the first text pass compiles GPU kernels, so loading does that up front).

The model and the index leave memory after ten idle minutes (about 1.9 GB while the server has it loaded).

## Text in photos

Signs, menus, receipts, documents, screenshots, slides: with **Also find words in photos** ticked in the Describe panel
(or `vision.setText`), the words printed in a photo are found by searching for them, in English, Chinese (Simplified and
Traditional), Japanese and 46 other languages. It is a separate, opt-in step with its own small download (about 31 MB:
**PP-OCRv6 small** from PaddleOCR, Baidu, Apache-2.0, a detector plus a recogniser, run by a pure-Rust ONNX runtime
(`rten`)), shown with its licence before anything is fetched. Nothing is read until you tick the box.

- **Reading is slow** (a second or so per photo on a laptop, more on a server): it runs in the background after the photos
  are described, from a 1280 px unedited rendering, and can be stopped (**Stop**) and resumed; photos that were read are
  never read again (kept by content hash in `<library>/search/text-<engine>.bin`, derived data like the vectors).
- **Searching:** the photos that have *every* word of your query in their text come first (`text: true` in the result,
  score 2 and up), then the photos that look like it. Matching is forgiving, because OCR is: case, accents and full-width
  forms don't matter, hiragana and katakana are the same, one wrong letter in a word of five or more still matches, and
  Chinese/Japanese match as phrases or when most of their character pairs are there. Text search works without the
  description model.
- **On the server:** with the models installed (the admin page's **Processing ▸ Words in photos**, or `lightcraft-server model download --accept-licences --text`; `model disable --text` or the switch turns it off, keeping what was read) the server reads
  each photo's smart preview, 25 photos at a time so new photos are never kept waiting, and `GET /api/search` merges the
  words and the looks; a desktop can send what it read (`vision.share`, same opt-in as the vectors) — the server keeps text
  for photos of that user's library only and refuses another reader's or a damaged upload whole. A server that only has text
  (sent by a desktop) still answers.

```text
GET  /api/index/text/keys          {engine, keys: [content hash]}          whose text the server has
POST /api/index/text               text a device read (checked whole)
```

Known limits: text upside down isn't read (there is no orientation classifier yet) and neither is curved text; very small
print needs the original's resolution; handwriting is hit and miss. There is no layout analysis (a line is a line), and
no "copy the text" action yet.

## People

Turn on **Find People…** at the top of the People view and LightCraft finds the faces in your photos, groups the ones that
look alike into people, and shows each as a card: type a name and press Return and that person is named in every photo
they are in, in one undoable step (a person on a bad day may be two cards: name both the same). New photos of a named
person show up as a **Confirm** button on their card (nothing is written until you click it); a click on a face shows
that person's photos.

**Where to find it:** the People row of the albums sidebar (or View ▸ People). On a phone it is a screen of round
faces: tap a face to name it (the people it looks like are offered), a person to see their page (rename them everywhere,
✓ / ✕ the faces that look like them, Confirm All), and in the photo editor tap a face on the photo to name it, or mark one
the finder missed with **Tag a Face** in the photo's "…" menu (`photo.addRegion`).

**It is opt-in, local and deletable.** Off by default, per library. Nothing is downloaded or looked at until you agree:
the bar shows the two models, their size (about 39 MB), where they are kept and their licences first. The models are
**YuNet** (finds faces; MIT, Shiqi Yu) and **SFace** (tells them apart; Apache-2.0, BUPT) from the OpenCV Zoo, run by a pure-Rust
ONNX runtime. Their makers trained them on collections of public photos of people (WIDER FACE for YuNet, a large web-scraped
face set for SFace) and publish them for general use; LightCraft ships and trains none of it and never trains on your photos.
What they find (a rectangle and 128 numbers per face) stays in the library's `search/` folder: never in the catalog, an
XMP sidecar or a backup of the catalog, and not sent anywhere unless you turn on **Send the search data to my server** and the
server's admin allowed it for you. **Forget All Faces** (or `people.deleteData`) deletes it, here and on the server. Names
are different: naming a person writes an ordinary face region (the MWG-RS kind the People view already shows) on each of
their photos, in the catalog (synced like any edit, undoable, never written to the XMP sidecar); remove one with the ×
on its face box, or undo.

How it works: each face is cut out and turned upright using its five landmarks (eyes, nose, mouth corners), SFace reads
that crop into 128 numbers, and faces whose numbers point the same way (cosine similarity of at least 0.40 with the average
of the person's faces; SFace's own pair threshold is 0.363) are one person. On public-domain portraits the same person
scores 0.86 and different people 0.11–0.16; a photo takes about 150 ms to look at on a laptop. Faces are looked at in the
same background pass as the description and the text, from a 1280 px unedited rendering.

**On the server** (faces are personal data, so it is separate and off): install the models once and allow each user,
on the admin page (**Processing ▸ People**: download the models after reading their licences, then a switch per user that asks
before it turns on) or from a shell:

```sh
lightcraft-server model download --accept-licences --faces
lightcraft-server user faces ann on        # and `off`; `user list` shows who has it
```

The server then looks at that user's smart previews (25 photos at a time) and lists the people for devices that find
none themselves (the web build, iOS): `people.list` shows them, naming one is a catalog edit on the device. The same people
group the **Unnamed faces** (face boxes from Detect Faces, another app or a hand-drawn one) into one picture per person
(`faces.unnamed` `group`): a box is matched to a found face on the same photo by their overlap, on this computer or with
the server's people on a device that finds none. A desktop that
finds faces can send them instead (same opt-in as the vectors, `vision.share`). The user can always delete what the server
found (`people.deleteData` asks it to, or `DELETE /api/index/faces`).

```text
GET    /api/people/status            {available, enabled (an admin allowed it), installed, engine, faces, photos, total}
GET    /api/people/clusters          {engine, faces, photos, clusters: [{id, members: [[photo, face, x0, y0, x1, y1, score]]}]}   403 until allowed
GET    /api/index/faces/keys         {engine, keys: [content hash]}                                                              403 until allowed
POST   /api/index/faces              faces a device found (checked whole; photos not in the library are skipped)                 403 until allowed
DELETE /api/index/faces              forget every face (always allowed)
```

Known limits: faces under about 32 px in the 1280 px view and faces turned away from the camera are missed or unreliable;
grouping is one quick pass (no hierarchy, no learning from your corrections); there is no "not this person", no merge by
drag and no suggestions from your other names yet; it has been checked on a handful of portraits and procedural images, not
on a large real library. Face recognition is only as fair as its training data: expect more mistakes on some groups of people
than others, which is one more reason names stay yours to give and confirm.

## Possibly sensitive photos

A fourth opt-in model, its own download: Falconsai's NSFW classifier flags photos that might be sensitive, in the same
background pass, so they can be reviewed and hidden. It hides nothing by itself. See [hidden.md](hidden.md) →
*Possibly sensitive photos* (commands `vision.setSensitive`, `vision.sensitive.download`, `vision.sensitive.list`,
`library.hideSensitive`, `library.setBlurSensitive`; server `GET /api/sensitive`, admin page ▸ Processing ▸ Sensitive
content, `model download --sensitive`).

## On the server

The server indexes each user's photos from the previews it already keeps (a worker wakes when a preview arrives, a folder
scan builds one, and every 30 s) and answers searches:

```text
GET  /api/search/status            {available, installed, enabled, installing, model, dim, indexed, total, text: {installed, engine, indexed}}
GET  /api/search?q=…&limit=…       {query, ids: [photo id], scores: [f32], indexed, total}
GET  /api/index/embeddings/keys    {model, dim, keys: [content hash]}      what the server already has
POST /api/index/embeddings         vectors a device computed (checked whole; see below)
```

An admin turns search on for everyone from the admin page (**Processing**): the switch shows the model's size and licence,
downloads it onto the server after the admin agrees (resumable, SHA-256-checked, it keeps going if the page is closed)
and turns search on. The page's **AI Models** list shows every model (description, words, people, sensitive content) with Download, Cancel and, once installed, Remove: it asks, then deletes only that model's files (`DELETE /api/admin/search/models/<search|text|faces|sensitive>`); what the model found stays, and downloading again resumes. **Reprocess** forgets what a model found for every user (`DELETE /api/admin/search/results/<part>`) and has every photo done again, by the server or by the devices that do it for their users; the model stays. A signed-in user can do the same for their own results (`DELETE /api/index/<embeddings|text|faces|sensitive>`, `vision.reprocess` with `scope` `server`, `device` or `both`; Settings ▸ AI Models ▸ Server Data). Standalone clients remove theirs in Settings ▸ AI Models (`models.delete`). The same from a shell (never without the flag; Docker: `docker compose exec lightcraft-server …`):

```sh
lightcraft-server model status
lightcraft-server model download --accept-licences      # about 1.5 GB, Google's Apache License 2.0
lightcraft-server model disable                         # off for every device; the files and vectors stay
lightcraft-server model enable
lightcraft-server model disable --text                  # the same for reading the text in photos
```

The switches are `<data>/search.json` (`{"enabled": bool, "text": bool}`; no file means on, so a server that already had the model keeps
searching). A running server notices a change within its next 30 s tick or at the next request, and drops the model from
memory when it is off. While it is off, `/api/search/status` says `installed: false, enabled: false` (and `installing`
while the admin page's download runs), so devices stop asking and say why. Admin routes (admin session token):

```text
GET    /api/admin/search            {enabled, installed, active, loaded, model, dim, dir, sizeBytes, license, licenseUrl, download, schedule,
                                     text: {enabled, installed, active, sizeBytes, license, licenseUrl, download, schedule},
                                     faces: {installed, sizeBytes, licenses, download, schedule},
                                     users: [{name, faces, total, indexed, read, found, work}]}   (null: not done for this server)
                                     schedule: {paused, from, to, runsNow, left, ready, perMinute, lastSecs, now: {user, secs}}
                                     work: {search|text|faces: {perMinute, waiting, failed, problem}}
PUT    /api/admin/search            {enabled}: the switch; the model stays on disk when off
PUT    /api/admin/search/text       {enabled}: the same for reading the text in photos (what was read is kept)
POST   /api/admin/search[/text|/faces]/download   {acknowledged: true}: download those models if they aren't there (400 without the
                                     licence agreed); for search and text that turns it on too
DELETE /api/admin/search[/text|/faces]/download   stop the download (a new one resumes it)
PUT    /api/admin/users/NAME/faces  {enabled}: find the people in that user's photos (needs the face models)
```

The admin page shows how the worker gets on with each kind of index. It takes turns: up to 200 photos described, 25 read
and 25 looked at for faces per user, then the next user, so a big library being described doesn't hold up the rest. A
line with a dot says what it is on now and when it last finished a photo (green while it works, orange when a turn has
gone ten minutes without finishing one, or nothing was done for half an hour although photos are ready). Each user's bar
says the time left at the pace of the last ten minutes (`perMinute`, counting the time spent on the other indexes and
users), how many photos wait for their previews (`waiting`), how many couldn't be done (`failed`: tried again after a
restart) and the last problem. A photo the model can't take is set aside on its own: it no longer fails its whole batch
on every pass, which used to stop describing for good. `ready` is the photos left that have previews and weren't set
aside; the time left is worked out from it. A native (non-Rust) frontend shows the same, from `host.admin.search` (the
host works out what the numbers mean, `crates/apphost/src/host/admin/search.rs`, [engine-host.md](engine-host.md)),
and sets each index's Pause and daily window as the web page does (`PUT search/schedule/<part>`, the times shown and
picked in the admin's own time, sent in UTC).

Until it is installed every route says so (`503`) instead of failing, and uploads are still accepted. Old servers answer
`404`, which clients read as "this server can't search". The routes are additive: old clients and servers keep working.

### Sending vectors to the server (opt-in)

A desktop (Metal) is far quicker than a server's CPU, so a device that has the model can send the server the vectors it
computed, and nothing is done twice. This is **off until you turn it on** (the checkbox in the panel, or `vision.setShare`;
saved with the library). The server keeps only vectors for photos that are in that user's library, only for the same model,
and refuses a whole upload that is for another model (`409`), damaged, truncated, or not finite (`422`). Vectors are rescaled
to unit length, so an upload can't skew rankings. Vectors describe what is in your photos: keep the server private (TLS
reverse proxy or Tailscale, see `sync.md`).

## A device as the server's processing unit

A server on a NAS has a slow processor and little memory; the Mac next to it has a fast one, a GPU and more RAM. Per
task, you can move the server's model work onto a device, without moving the photos: tick **Search / Text / People /
Sensitive content: … on this device instead of my server** in the Describe panel (`vision.setHelper {parts}`; a task
shows only when its models are installed on that device). Then:

1. the device **claims** a few photos from the server (`POST /api/work/claim`, 16 at a time, with the engine ids of its
   models, so only results the server would keep are asked for). A claimed photo is the device's for ten minutes and
   no other device, nor the server's own worker, is given it meanwhile;
2. it **fetches only their previews** (`GET /api/blobs/smart|mini/<hash>`) into a private scratch folder, renders each
   unedited, processes it in memory, and **deletes the preview** at once (the folder goes when the helper stops);
3. it **sends the results** through the routes devices already use (`POST /api/index/embeddings|text|faces|sensitive`:
   checked whole, only photos of that user's library are kept), **releases** the photos (`POST /api/work/release`; one
   it can't process is marked failed so it isn't offered again) and takes the next batch. Nothing of the library stays
   on the device and it needs no copy of it. Editing never moves: it is always on the device.

While a part is moved, the server's worker **skips it for that user** (`GET/PUT /api/work/settings`, kept in
`<data>/users/<name>/offload.json`; a device adds and removes only its own parts), so a part with no device online
waits instead of loading the NAS. Admin switches still rule: a part the admin turned off, or faces for a user who
isn't allowed them, is refused (`refused` in the claim's answer), and so is a part the admin **paused** (or limited to hours): Pause on the admin page holds back the devices too. `vision.helperOnce {part}` takes a part up only until a claim comes back empty, then gives it back to the server by itself (Settings ▸ Sync ▸ Server Work ▸ Build Missing Previews Once; `once` in `vision.helper`). A device can pause its own part (`vision.setHelperPaused`, kept with the library: the part stays its, nothing is claimed) from Settings ▸ AI Models ▸ Server Work. What devices send counts in the admin page's pace and last-done time, and a user's row says a device does that part (`work.<part>.device.seenSecs`, `deviceSecs`). The helper is one thread, a row in the activity
stack, started again after a sign-in or a failure, and ends on sign-out. `lightcraft-server` needs no model for a part
that is moved.

```text
POST /api/work/claim      {parts: {search|text|faces|sensitive: engine}, max?} → {items: [{part, key, kind, photo}], refused, ttl}
POST /api/work/release    {items: [{part, key, failed?}]}
GET  /api/work/settings   {moved: [part], helperSeen: seconds | null}
PUT  /api/work/settings   {moved: [part]} | {add?: [part], remove?: [part]}
POST /api/index/sensitive scores a device computed (checked whole, like the others)
```

The egui desktop app does it (the Describe panel). Other work
that could move the same way (building previews from the originals, Lens Blur depth) does not: the originals would have
to travel.

## Limits (honest)

- Faces: see **People** above.
- Relevance has **no score cut-off**: you get the best N (200 by default, up to 2000), not "only the matches". SigLIP's
  scores are small numbers; a calibrated cut-off needs a real-library benchmark we haven't run.
- Quality has been checked on procedural images and a handful of portraits, not on a large real library. Gender and similar
  attributes in non-English queries are weak spots of this model class.
- Reading text is slow (see above) and only as good as the print: a photo of a crowded street won't give up its shop signs.
- Brute-force search (exact, one thread): fine to several hundred thousand photos; an approximate index would be needed
  beyond about a million.
- The egui iOS app's build has no models; it searches through the server. A native (non-Rust) frontend's phone build can
  have them (the engine's `vision` feature): the text, face and sensitive-content models are a few tens of MB and run on
  any phone, the sensitive-content classifier's GPU weights on the phone's GPU; the search model only where the app may
  use about 3.05 GB (`vision::SEARCH_PHONE_MEMORY`, from `os_proc_available_memory` at launch; an iPhone 17 Pro's 3.3 GB
  is enough), because it takes 1.5 GB while loaded (on the CPU there), next to the caches' 1 GB. Elsewhere it is listed as not available, with why
  (`models.list` → `unavailable`, `vision.model.status` → `unfit`), and the phone searches through the server. Not yet
  measured on a device: the peak memory of an indexing run, and how long a library takes on a phone's CPU.
- The model's training data is the model vendor's: the weights are Apache-2.0 and downloaded by the user, never committed.
