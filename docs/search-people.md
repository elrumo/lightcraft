# Search by description (and, next, People and text in photos)

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
- **A device with no model** (the web build, iOS, or a desktop that didn't download it) searches through the sync server when
  the server can: the switch appears once the server says so, and the panel says how many photos the server has ready.
- **CLI / MCP / control channel:**

  | Command | What it does |
  |---|---|
  | `vision.model.status` | whether the model is installed and loaded, how many photos are indexed, the server's state, sharing |
  | `vision.model.download {acknowledged: true}` / `vision.model.cancel` | download (resumable, SHA-256-checked) or stop |
  | `vision.index {wait?, ids?}` / `vision.indexProgress` / `vision.indexCancel` | embed the photos that have no vector yet |
  | `library.search {q, limit?, wait?, source?}` | `source`: `local`, `server`, or `auto` (this device's index when it covers the library, else the server's) |
  | `vision.share {wait?}` / `vision.setShare {on}` | send this device's vectors to the server, once or whenever new photos are indexed |

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

## On the server

The server indexes each user's photos from the previews it already keeps (a worker wakes when a preview arrives, a folder
scan builds one, and every 30 s) and answers searches:

```text
GET  /api/search/status            {available, installed, model, dim, indexed, total}
GET  /api/search?q=…&limit=…       {query, ids: [photo id], scores: [f32], indexed, total}
GET  /api/index/embeddings/keys    {model, dim, keys: [content hash]}      what the server already has
POST /api/index/embeddings         vectors a device computed (checked whole; see below)
```

An admin installs the model once (never without the flag; Docker: `docker compose exec lightcraft-server …`):

```sh
lightcraft-server model status
lightcraft-server model download --accept-licences      # about 1.5 GB, Google's Apache License 2.0
```

Until it is installed every route says so (`503`) instead of failing, and uploads are still accepted. Old servers answer
`404`, which clients read as "this server can't search". The routes are additive: old clients and servers keep working.

### Sending vectors to the server (opt-in)

A desktop (Metal) is far quicker than a server's CPU, so a device that has the model can send the server the vectors it
computed, and nothing is done twice. This is **off until you turn it on** (the checkbox in the panel, or `vision.setShare`;
saved with the library). The server keeps only vectors for photos that are in that user's library, only for the same model,
and refuses a whole upload that is for another model (`409`), damaged, truncated, or not finite (`422`). Vectors are rescaled
to unit length, so an upload can't skew rankings. Vectors describe what is in your photos: keep the server private (TLS
reverse proxy or Tailscale, see `sync.md`).

## Limits (honest)

- Relevance has **no score cut-off**: you get the best N (200 by default, up to 2000), not "only the matches". SigLIP's
  scores are small numbers; a calibrated cut-off needs a real-library benchmark we haven't run.
- Quality has been checked on procedural images and a handful of portraits, not on a large real library. Gender and similar
  attributes in non-English queries are weak spots of this model class.
- It does not read text in photos (signs, documents, screenshots): OCR is planned as its own step.
- Brute-force search (exact, one thread): fine to several hundred thousand photos; an approximate index would be needed
  beyond about a million.
- iOS runs no model on the device (memory budget); it searches through the server.
- The model's training data is the model vendor's: the weights are Apache-2.0 and downloaded by the user, never committed.
