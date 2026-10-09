# Self-hosted sync

**Setting it up step by step:** [self-hosting.md](self-hosting.md) (deploy the server, connect the desktop, web and
iOS apps). This page is the reference.

LightCraft is local-first: a library is a folder on your computer and needs no account. **Sync is optional**: run
your own LightCraft server and every device you sign in shares one library — photos, edits, albums, ratings,
keywords, versions — the way Lightroom (cloud) does, but on a machine you own. Nothing is sent anywhere until you sign
in, and only to the server you name.

- **Server:** `apps/lightcraft-server`, one pure-Rust binary (or a Docker image). It keeps each user's library and
  photo files, orders every device's changes, and serves the web build at `/`. Photo folders that are already on the
  server (a NAS share, years of `2019/Holidays/…`) can be a user's **library folders**: read where they are, never
  copied, moved or changed ([below](#library-folders-photos-already-on-the-server)).
- **Clients:** the desktop app (Settings ▸ Sync), the web build the server serves (open the server's address in a
  browser, Settings ▸ Sync), the iOS app (a spike: [ios.md](ios.md)) and `lightcraft-cli`/MCP. The protocol is plain
  HTTP + JSON and the device side is sans-IO Rust (`crates/engine/src/sync.rs`, `crates/catalog/src/sync.rs`).
- **v1 is one person's devices.** Sharing albums with other people comes later.

## Run a server

### Docker Compose, with HTTPS on your own domain

The easiest setup on a machine reachable from the internet (a home server, a small VPS): the server plus
[Caddy](https://caddyserver.com), which gets and renews the HTTPS certificate by itself.

```sh
# a DNS name pointing at this machine, ports 80 and 443 open
cd apps/lightcraft-server
LIGHTCRAFT_DOMAIN=photos.example.com LIGHTCRAFT_PHOTOS=/srv/photos docker compose up -d --build
docker compose logs lightcraft     # "… enter the setup code ABCD-EFGH"
```

`LIGHTCRAFT_PHOTOS` (optional) is a folder of photos already on this machine; it is mounted read-only at `/photos`
in the container, for [library folders](#library-folders-photos-already-on-the-server).

Open `https://photos.example.com/admin`, enter the setup code, create your admin account and add users there
([the admin page](#the-admin-page)); or from the command line:
`docker compose exec -it lightcraft lightcraft-server user add ann` (type the password; it isn't shown).

`apps/lightcraft-server/docker-compose.yml` and its `Caddyfile` are all there is to it; everything the server keeps is
in the `lightcraft-data` volume. Then [connect your devices](#connect-your-devices) to `https://photos.example.com`.

### Docker

```sh
docker build -f apps/lightcraft-server/Dockerfile -t lightcraft-server .
docker run -d --name lightcraft --restart unless-stopped -p 8080:8080 \
  -v lightcraft-data:/data -v /srv/photos:/photos:ro lightcraft-server
docker exec -i lightcraft lightcraft-server user add ann        # type the password, then Enter
docker exec lightcraft lightcraft-server folder add ann /photos/ann --name Photos   # optional
```

The image holds the server and the web build; everything it keeps is in the `/data` volume (the photo folders
mounted at `/photos` stay where they are). It runs as uid 10001, which must be able to read the photo folders, and
reports its health to Docker (`lightcraft-server health`).

### Without Docker

```sh
cargo build --release -p lightcraft-server
printf '%s\n' 'a long password' | target/release/lightcraft-server user add ann --data /srv/lightcraft
target/release/lightcraft-server serve --data /srv/lightcraft --listen 127.0.0.1:8080 [--web target/web]
```

| Command | What it does |
|---|---|
| `serve [--listen HOST:PORT] [--web DIR] [--scan-interval MIN]` | Serve (default `127.0.0.1:8080`, only this computer; `$LIGHTCRAFT_LISTEN`). `--web` (`$LIGHTCRAFT_WEB`): the `cargo xtask web` bundle, served at `/` with the same cross-origin isolation headers as the dev server. `--scan-interval` (`$LIGHTCRAFT_SCAN_INTERVAL`, default 15): minutes between scans of the library folders, 0 = at start and on demand only. `$LIGHTCRAFT_PREVIEW_THREADS`: threads building their previews (default half the cores, 1–4). `$LIGHTCRAFT_RENDER_THREADS`: photos rendered at once for devices that [export on the server](#exporting-on-the-server) (default 1; a render takes about as much memory as the same export on a desktop) |
| `user add NAME [--admin]` / `user passwd NAME` | Add a user / change a password (first line of stdin, or `$LIGHTCRAFT_PASSWORD`; 8 characters at least). Works while the server runs |
| `user admin NAME on\|off` | Let a user sign in to [the admin page](#the-admin-page), or stop them (the last admin stays) |
| `user remove NAME` / `user list` | Remove a user (signs out every device; their files stay in `users/NAME/`) |
| `device list NAME` / `device revoke NAME ID` | A user's signed-in devices; sign one out (it must sign in again) |
| `folder add NAME PATH [--name FOLDER]` | Make `PATH` (a folder on the server) one of `NAME`'s [library folders](#library-folders-photos-already-on-the-server), shown on devices as `FOLDER` (default: its own name) |
| `folder remove NAME FOLDER` / `folder list [NAME]` | Stop reading a library folder (its photos stay in the library) / list them |
| `folder ignore NAME [PATTERN…]` | Names the scan of `NAME`'s library folders leaves out, like `'*.fcpbundle'` (see [below](#library-folders-photos-already-on-the-server)); the list given replaces the old one, none shows it, `''` clears it |
| `scan [NAME]` | Scan the library folders now (a running server is asked to; otherwise here, with their previews) |
| `gc [--dry-run]` | Delete photo files no library refers to any more (photos deleted for good) and unfinished uploads, if older than a day |
| `health` | Exit status 0 when the server on `--listen` answers (the Docker health check) |

Every command takes `--data DIR` (default `$LIGHTCRAFT_DATA`, else `./lightcraft-data`). `$LIGHTCRAFT_LOG` = `error`,
`warn`, `info` (default) or `debug`.

### TLS: a reverse proxy, or a private network

The server speaks plain HTTP. Passwords and tokens must not cross a network in the clear, so either:

- **Caddy** (automatic HTTPS certificates):

  ```
  photos.example.com {
      reverse_proxy 127.0.0.1:8080
  }
  ```

  Allow large uploads if your proxy limits bodies (raw files and videos; the server takes up to 16 GiB per file).

- **Tailscale** (or another WireGuard network): listen on the tailnet address (`--listen 100.x.y.z:8080`) and use
  `http://machine-name:8080` — the tunnel is already encrypted. `tailscale serve` can add HTTPS on top.

Clients trust the Mozilla root certificates (pure-Rust TLS: rustls with the RustCrypto provider, no C), so a
certificate from Let's Encrypt or Tailscale works as is; a self-signed one doesn't.

### What the server keeps

```text
<data>/users.json                              users: argon2id password hash, library id, admin
<data>/users/<name>/library/catalog.log        every change from every device, in order
<data>/users/<name>/blobs/<kind>/<xx>/<hash>   photo files by content: original | smart | mini
<data>/users/<name>/presets.json               user presets { version, presets }
<data>/users/<name>/devices.json               signed-in devices: name, id space, SHA-256 of the token
<data>/users/<name>/folders.json               library folders as last scanned: each file's size, time, hash, photo
```

`users.json` and `devices.json` are readable by the server's user only.

Back up `<data>` like any folder (stop the server, or copy `catalog.log` first and the blobs after).

### The admin page

`https://<your server>/admin` manages the server from a browser. It has three sections (a side bar on wide
screens, tabs on a phone), follows the system's light or dark mode, and confirms anything destructive in a dialog:

- **Overview:** users, photos, signed-in devices and free disk space at a glance; **Connect a device** (the address
  to type into Settings ▸ Sync on each device, the one you opened the page with, with a Copy button); and the
  server's details: version, data folder, the address it listens on, whether it serves the web build.
- **Users:** every user with their photo, album and device counts and storage used; **Add user**. Open a user for
  their page: reset the password, make or remove an admin, remove the user (their files stay in `users/NAME/` until
  you delete them), and two tabs:
  - **Devices:** their signed-in devices (name, id, when signed in, last seen); sign one out (it must sign in again).
  - **Library folders:** their [library folders](#library-folders-photos-already-on-the-server): add one (a path on
    the server, and the name devices show), remove one, whether the server can see it, the names to leave out of the
    scan (**Skipped names**), **Scan now**, and what the
    last scan found (new, moved, changed, missing, files that couldn't be read) with progress while it scans and
    while previews are built.
- **Storage:** the disk holding the data folder (used and free), storage used by each user, and photo-file clean-up
  (**Check for unused files** = `gc --dry-run`, then **Remove files** = `gc`).

**First run:** while no user is an admin, the server prints a one-time **setup code** to its log
(`docker compose logs lightcraft`, or the terminal running `serve`). The admin page asks for it before it creates the
first admin, so a server that is already on the internet can't be claimed by whoever finds it first; ten wrong codes
replace it with a new one (logged again). Already have users? `lightcraft-server user admin ann on` (or
`user add NAME --admin`) instead.

Admins sign in to the page with their user name and password. That opens an admin session of its own (not a device:
it can't sync, and a device token can't manage the server); it ends after two hours without use, or with Sign Out.
An admin is also an ordinary user with a library of their own.

The data folder, the listen address and the domain are deployment settings, so the page shows them but doesn't change
them: `--data` / `LIGHTCRAFT_DATA`, `--listen` / `LIGHTCRAFT_LISTEN`, and the domain in the reverse proxy
(`LIGHTCRAFT_DOMAIN` with the compose file). Its JSON API is under `/api/admin/` (`apps/lightcraft-server/src/admin.rs`).

## Search by description

The server can also search each user's photos by what is in them, so the web build, iOS and other
computers need no model: `GET /api/search?q=…`, an index of vectors per user that devices may add to, and
`lightcraft-server model download --accept-licences` to install the model (about 1.5 GB, loaded on first use, ~1.9 GB of
memory while loaded). It can also read the text in photos (`model download --text`) and, for a user an admin allows
(`lightcraft-server user faces NAME on`, after `model download --faces`), find the people in their photos for devices to
name. Routes, privacy and limits: [`search-people.md`](search-people.md).

## Library folders: photos already on the server

A self-hosted server usually sits next to the photos: a NAS share, a backup disk, folders of past years. An admin
can make such folders a user's **library folders** (admin page › Users › the user › Library folders, or
`lightcraft-server folder add ann /photos/ann --name Photos`). The server reads them in place:

- **Nothing in them is copied, moved or changed.** Mount them read-only (`-v /srv/photos:/photos:ro`). The server
  keeps only the previews it builds and an index (`folders.json`); a photo's original is sent to a device straight
  from its folder, when the device asks for it (Download Originals, or keeping originals on the device).
- **Every photo there joins the user's library** like any other: on every device, in All Photos, in searches and
  smart albums, in any album (albums stay virtual: a photo can be in many, its file stays where it is), editable
  (edits are kept in the library, never written to the folder).
- **Devices show the folders** in the sidebar under **Server Folders**, with how many photos each holds: a click
  shows a folder's photos (and those of the folders in it); right-click it to create an album from it, make it
  available offline or download its originals. Info shows each photo's place on the server.
- **XMP sidecars are read** when a file is found: ratings, flags, colour labels, title, caption, keywords and
  Lightroom (Camera Raw) edits from `IMG_0001.xmp` (or a raw's own XMP), as an import reads them. A sidecar that
  changes later (the photo was edited again in Lightroom) is read again by the next scan, without reading the photo
  file again: what it states replaces the photo's values on every device (fields it doesn't state are kept), as
  Read Metadata from File does. Edits made on a device stay until the sidecar changes. The index keeps each
  sidecar's time; an index written by an older server only notes them on its first scan.
- **Scans** run when the server starts, every 15 minutes (`--scan-interval`), when a folder is added and on demand
  (**Scan now**, `lightcraft-server scan`). Files whose size and time didn't change aren't read again.
  - a new file becomes a new photo, with smart and mini previews built on the server (`LIGHTCRAFT_PREVIEW_THREADS`);
  - a file that moved or was renamed keeps its photo, edits and albums (same content, its old place gone);
  - a file changed in place keeps its photo and edits and gets new previews;
  - a copy of a file is a photo of its own;
  - a file a device had uploaded already becomes that photo's place on the server, not a second photo, and a device
    never uploads a file the server has in a folder.
- **A scan never takes anything away.** A file that disappears keeps its photo (its original can't be downloaded
  until it's back); a library folder that is missing, or empty when it had photos (an unmounted disk), is skipped
  and shown as such on the admin page. A photo removed from the library on a device isn't added back by the next
  scan (unless its file changes). Removing a library folder keeps its photos in the library.
- **Names you choose are skipped too** (admin page › the user › Library folders › **Skipped names**, or
  `lightcraft-server folder ignore ann '*.fcpbundle' 'Proxy Media'`). A pattern is one file or folder name, not a
  path: `*` stands for any run of characters, `?` for one, capitals don't matter, and anything inside a skipped
  folder goes with it, so `*.fcpbundle` leaves out every Final Cut Pro bundle (and its `Original Media`) wherever it
  is. The list belongs to the user and covers all their library folders; the next scan (started when it is saved)
  uses it. Photos already read from a name that is skipped now stay in the library with their edits, their files
  showing as missing, as with any file that disappears; take the name off the list and they are found again.
- Hidden folders and NAS bookkeeping (`@eaDir`, `#recycle`, `#snapshot`, `$RECYCLE.BIN`…) are skipped, symbolic
  links aren't followed, files larger than 2 GiB aren't read, and every file is decoded under a guard: a damaged
  one is reported, never fatal.

## Connect your devices

Every client needs the same three things: the **server address** (`https://photos.example.com`, or
`http://machine-name:8080` on a tailnet), a **user name** and its **password**, as added on
[the admin page](#the-admin-page) or with `lightcraft-server user add`. The password goes to the server once; the
device gets its own token (kept in the library's `sync.json`), which revoking the device (admin page or
`lightcraft-server device revoke`) or Sign Out ends. A bare `photos.example.com` means `https://photos.example.com`;
a bare address at home or on a tailnet (`localhost`, a private or Tailscale IP address, a one-word name such as `nas`,
`*.local`, `*.lan`) means `http://` (type `https://` to say otherwise).
The device is listed on the server under the computer's name (`$LIGHTCRAFT_DEVICE` to choose another).

| Client | How |
|---|---|
| Desktop app | **Settings ▸ Sync** (or click the cloud icon in the top bar): server, user, password, **Sign In** |
| Browser | open the server's address: the server serves the web build, and Settings ▸ Sync has the address filled in |
| Command line / agents | `lightcraft-cli run --library DIR sync.signIn server=… user=… password=… sync.now wait=true` |
| iOS (spike, [ios.md](ios.md)) | **Settings ▸ Sync**, as on the desktop; the library is kept in the app's Documents folder |

Users and devices are managed on the server: on [the admin page](#the-admin-page) or from its command line
(`user add / passwd / admin / remove / list`, `device list / revoke`, `gc`; see the table above).

## What happens at sign-in

- **The first library** signed in to an empty server **uploads itself**: every photo's original, a smart preview
  (≤ 2560 px, ~1 MB) and a mini preview (≤ 512 px) built on this device, then the whole catalog. A library holding
  only the demo photos uploads nothing: the empty server library replaces them.
- **Other devices sign in from a new, empty library** (Settings ▸ General ▸ Open Library… → a new folder). A library
  that already has photos can't join a server library that has some: v1 doesn't merge two libraries. A new library
  holding only the demo photos counts as empty (the server's library replaces them).
- **Signing out** keeps the library and its unsent changes; signing in again (same server and user) resumes.
- **File ▸ Pause Syncing** stops talking to the server until resumed; **File ▸ Sync Now** pulls at once (it also pulls
  every 5 seconds and whenever the window comes back to the front).

The cloud icon shows the state: its tooltip says synced / syncing (with what's queued) / paused / signed out / the last
error, and it turns amber while the server can't be reached (changes wait on the device, nothing is lost).

## Photos on a device

Each device keeps the **whole catalog** (it's small) and **downloads pixels as needed**, by content hash:

| What | When | Used for |
|---|---|---|
| mini preview (≤ 512 px) | every photo whose original isn't here | grid and filmstrip thumbnails, rendered here with the current edits (never stale) |
| smart preview (≤ 2560 px) | the photo you open, and everything **made available offline** | the loupe, editing, exporting at preview size |
| original | **Photo ▸ Download Originals**, or every photo with *Store the originals of all photos on this device* | full-size export, 1:1 |

- **Make Available Offline:** right-click an album (✓ marks it in the sidebar), or **Photo ▸ Make Available Offline**
  for selected photos: their smart previews stay on the device, so they open and edit without a network.
- A downloaded original (`<library>/sync/originals/<hash>/<name>`) becomes the photo's file on this device only.
- Photos whose original is on the server are not *Missing Photos*; Info says “Original on the sync server”.
- Photos you import on any desktop device are uploaded with their previews in the background.

### How much space it takes

**Settings ▸ Sync ▸ Storage** shows what this library takes, refreshed while the page is open (**Refresh** asks
at once):

- **On the server:** the originals it keeps, smart previews and small previews (each with its file count), their
  total, the **library folders** (photos read where they are on the server, never copied, so not in the total) and
  how much room the server's disk has left (the bar: this library, everything else, free).
- **On this computer:** the originals downloaded from the server, smart and small previews, the thumbnail cache,
  copies imported into the library, the catalog and edits, their total and the disk's free space. Photos you
  imported from folders on this computer stay where they are and aren't counted. The header says how many photos
  have only previews here.

The same numbers, in bytes, are the `sync.usage` command (`lightcraft-cli run --library DIR sync.usage refresh=true`,
MCP, control channel). Disk space comes from `df`, so it's missing on Windows and in the browser.

## Exporting on the server

A phone can't always hold a full-size render in memory: a 48 MP photo takes about 2.9 GB to develop and encode, and
iOS ends an app that goes over its limit. A device that is signed in to a server with the photo's original can have
the **server render it**: it sends the photo's content hash, its develop settings and the export options
(`POST /api/render`, [`proto::Render`](../crates/catalog/src/sync.rs)); the server decodes the original, runs the same
pipeline and encoder as the apps, and sends the image back, which the device saves like any export.

- **When:** an export with `onServer: true` (the `app.export` parameter), or **automatically** when the export would need
  more memory than the device may use (`lightcraft_engine::memory::export_limit`: on iOS two memory budgets;
  elsewhere none unless `LIGHTCRAFT_EXPORT_MEMORY_MB` is set). Without a server that has the photo, such an export
  fails with a message saying how big it is and what to do (export smaller, or sign in), instead of the app being
  ended halfway.
- **Smaller exports never need it:** a DNG exported at half its size or less is read at about the size the export
  needs (an Apple ProRAW is binned while it is decoded), so a 4096 px export of a 48 MP photo takes about as much as one
  of a 12 MP photo.
- **What the server renders:** the edits and options the device sends. The metadata comes from the original's own
  EXIF and XMP; titles, keywords and location edits kept only in the device's library are not on the exported file.
  AI masks (subject, sky, people) travel with the settings, which store their segmentation, so they render the same.
- The server renders one photo at a time by default and tells a device that waits more than 45 seconds to try again.

## What syncs, what stays on the device

| Syncs | Stays on each device |
|---|---|
| photos (added, removed, Recently Deleted), ratings, flags, colour labels, label names | where the original is on this disk (renames and relinks of files), Local folders browsed |
| develop settings, versions (incl. automatic ones), capture time edits | edit **History** (each device keeps its own steps) |
| user presets (create, rename, move, delete, favourite) | favourites among the built-in presets, recent profiles |
| metadata and keywords, analysis results | undo / redo |
| albums, folders, smart albums, stacks | Settings (`prefs.json`), the view (`view.json`, `ui.json`) |
| | Local records (photos seen while browsing a folder, until added to the library) |

## When two devices change the same thing

The server keeps **one order** of every change. A device sends its changes on top of the newest one it has seen; if
another device got there first it pulls, merges, and sends again. What the merge does:

- **Different settings of the same photo both survive**: Exposure changed on the Mac and Contrast on the laptop end up
  together (a field-by-field three-way merge of the develop settings, metadata, album and stack records).
- **Albums, stacks and keywords merge member by member**: photos added to one album on two devices are all in it.
- **The same value changed on both**: the change that reaches the server last wins (curves and masks count as one
  value each).
- **A removal beats an edit**: a photo or album deleted on one device is gone, and edits waiting for it elsewhere are
  dropped.
- **Undo after someone else's change:** a change from another device clears Redo (and Undo too when something was
  removed), so undo never silently reverts another device's edit.
- **During a slider drag** nothing from the server is applied; it waits until you let go.
- **Presets** are one document on the server, merged by preset id against the copy both sides last agreed on: a preset
  deleted on one device stays deleted unless the other changed it meanwhile, presets added on both are all kept, and
  one edited on both merges field by field. A preset created on a synced device gets an id that includes its id space.

Ids never collide: the server gives each device an id space at sign-in, and ids a device hands out are
`space · 2³² + n` (still exact in JavaScript). The library uploaded first keeps the ids it had.

**Never lose a change:** every change is queued in the library's `sync.outbox` *before* it is written to the catalog
log, and the position in the server's order (`sync.json`) only *after*; a crash in between re-sends or re-pulls, and
both end up the same. A change the server refuses (it no longer applies there) is dropped and the device reloads the
server's state, replaying its other pending changes on top.

## Protocol

JSON over HTTP; every route but `login` wants `Authorization: Bearer <token>`. Types: `lightcraft_catalog::sync::proto`.

| Route | Body → answer |
|---|---|
| `POST /api/login` | `{user, password, device}` → `{token, device, space, library}` · `401` |
| `POST /api/logout` | signs this device out |
| `GET /api/me` | `{user, device, library, devices}` |
| `GET /api/snapshot` | `{library, seq, catalog}`: the library after change `seq` |
| `GET /api/usage` | what this user takes on the server ([`proto::Usage`](../crates/catalog/src/sync.rs)): `{photos, albums, devices, original, smart, mini, folders: {files, bytes}, disk: {total, free}}`; a few seconds old at most. A server older than the route answers `404` |
| `GET /api/ops?since=N&limit=M` | `{head, ops: [[seq, op]…], presets}` (presets = the presets document's version) · `410`: reload the snapshot |
| `POST /api/ops` | `{base, ops}` → `200 {head}` · `409 {head}` (behind: pull first) · `422 {index, error}` (op `index` doesn't apply; nothing was) |
| `HEAD`/`GET`/`PUT /api/blobs/{original\|smart\|mini}/{hash}` | photo files by 128-bit content hash; `GET` takes `Range`; an original is only kept if its bytes hash to its name, previews must be LightCraft previews |
| `GET`/`PUT /api/presets` | `{version, presets}`; `PUT` with a stale `version` → `412` with the current document |
| `POST /api/render` | `{hash, name, settings, export}` → the encoded image (`X-LightCraft-Size: WxH`): the photo with this original, rendered with these edits and [export options](#exporting-on-the-server) · `404` no such original (or a server older than the route) · `422` can't be rendered · `503` busy, `Retry-After` |
| `GET /api/health` | `{ok, version}` (no sign-in) |
| `GET /…` | the web build (`--web`) |

Ops are the catalog's own (`lightcraft_catalog::Op`); the server applies each push to its copy of the library, all
or nothing, so it never stores a change that doesn't apply. Device-local ops (file paths, Local folders, History) are
refused. The server adds ops of its own for library folders (`AddPhoto`, `SetServerPath`, `SetContent`), in an id
space no device is given. An original kept in a library folder is served from there (`HEAD`/`GET`).

## Security

- Passwords are argon2id hashes; device and admin tokens are 256-bit random, stored as SHA-256 hashes. The account
  files are readable by the server's user only.
- **Password guessing is throttled** by client address and by user name: five wrong passwords in a row are free
  (typos), then each further try must wait, doubling from a second up to 15 minutes (`429` with `Retry-After`).
  Behind a reverse proxy on the same machine or a private network, the client's address is taken from
  `X-Forwarded-For`.
- Requests that need no sign-in (signing in, first-run setup) are capped at 64 KiB; JSON at 64 MiB, photo files at
  16 GiB. Every answer says `X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer`, the API's
  `Cache-Control: no-store`; the web build can't be framed by another site, the admin page has a strict Content
  Security Policy. The Caddy configuration adds HSTS.
- Library folders are set by admins only, must exist, and can't be (or hold) the server's data folder.

## Limits

v1, honestly:

- **The iOS app is a spike** ([ios.md](ios.md)): it runs on the simulator (compact touch layout); sync there is wired
  up but not yet run on a simulator or device.
- **In the browser** the web build signs in to the server that serves it (same origin; no cross-origin servers). Synced
  previews and downloaded originals are kept in the browser's storage. Commands that read a photo's pixels on the main
  thread (auto settings, export) need its original there: **Photo ▸ Download Originals** first. A photo imported in
  the browser is uploaded with previews built in the page (slow for large raws).
- Preferences, LUT profiles and export / metadata / filter presets stay per device.
- **No merging of two existing libraries**, no sharing with other people, no shared albums or links.
- **Originals downloaded to a device are kept** until you delete them (no automatic eviction under a size budget yet).
- **The server compacts its log** into a snapshot at 64 MiB and keeps the newest 16 MiB of it (tens of thousands of
  changes): a device behind by more than that reloads the library. (A device that is only a little behind keeps
  pulling; a pull reads just the part of the log it asks for.)
- **Library folders are read, never written**: edits stay in the library (no XMP written back to the folders), and
  photos imported on a device are kept by the server as uploads, not filed into the folders.
- **Uploads aren't resumable**: an interrupted upload starts again.
- **One request per thread, plain HTTP**: meant for a home server behind a proxy or on a tailnet, not the open
  internet at scale.
