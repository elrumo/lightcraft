# Self-hosted sync

LightCraft is local-first: a library is a folder on your computer and needs no account. **Sync is optional**: run
your own LightCraft server and every device you sign in shares one library — photos, edits, albums, ratings,
keywords, versions — the way Lightroom (cloud) does, but on a machine you own. Nothing is sent anywhere until you sign
in, and only to the server you name.

- **Server:** `apps/lightcraft-server`, one pure-Rust binary (or a Docker image). It keeps each user's library and
  photo files, orders every device's changes, and serves the web build at `/`.
- **Clients:** the desktop app (Settings ▸ Sync), the web build the server serves (open the server's address in a
  browser, Settings ▸ Sync) and `lightcraft-cli`/MCP. An iOS app is next: the protocol is plain HTTP + JSON and the
  device side is sans-IO Rust (`crates/engine/src/sync.rs`, `crates/catalog/src/sync.rs`).
- **v1 is one person's devices.** Sharing albums with other people comes later.

## Run a server

### Docker Compose, with HTTPS on your own domain

The easiest setup on a machine reachable from the internet (a home server, a small VPS): the server plus
[Caddy](https://caddyserver.com), which gets and renews the HTTPS certificate by itself.

```sh
# a DNS name pointing at this machine, ports 80 and 443 open
cd apps/lightcraft-server
LIGHTCRAFT_DOMAIN=photos.example.com docker compose up -d --build
docker compose logs lightcraft     # "… enter the setup code ABCD-EFGH"
```

Open `https://photos.example.com/admin`, enter the setup code, create your admin account and add users there
([the admin page](#the-admin-page)); or from the command line:
`docker compose exec -it lightcraft lightcraft-server user add ann` (type the password; it isn't shown).

`apps/lightcraft-server/docker-compose.yml` and its `Caddyfile` are all there is to it; everything the server keeps is
in the `lightcraft-data` volume. Then [connect your devices](#connect-your-devices) to `https://photos.example.com`.

### Docker

```sh
docker build -f apps/lightcraft-server/Dockerfile -t lightcraft-server .
docker run -d --name lightcraft --restart unless-stopped -p 8080:8080 -v lightcraft-data:/data lightcraft-server
docker exec -i lightcraft lightcraft-server user add ann        # type the password, then Enter
```

The image holds the server and the web build; everything it keeps is in the `/data` volume.

### Without Docker

```sh
cargo build --release -p lightcraft-server
printf '%s\n' 'a long password' | target/release/lightcraft-server user add ann --data /srv/lightcraft
target/release/lightcraft-server serve --data /srv/lightcraft --listen 127.0.0.1:8080 [--web target/web]
```

| Command | What it does |
|---|---|
| `serve [--listen HOST:PORT] [--web DIR]` | Serve (default `127.0.0.1:8080`, only this computer; `$LIGHTCRAFT_LISTEN`). `--web` (`$LIGHTCRAFT_WEB`): the `cargo xtask web` bundle, served at `/` with the same cross-origin isolation headers as the dev server |
| `user add NAME [--admin]` / `user passwd NAME` | Add a user / change a password (first line of stdin, or `$LIGHTCRAFT_PASSWORD`; 8 characters at least). Works while the server runs |
| `user admin NAME on\|off` | Let a user sign in to [the admin page](#the-admin-page), or stop them (the last admin stays) |
| `user remove NAME` / `user list` | Remove a user (signs out every device; their files stay in `users/NAME/`) |
| `device list NAME` / `device revoke NAME ID` | A user's signed-in devices; sign one out (it must sign in again) |
| `gc [--dry-run]` | Delete photo files no library refers to any more (photos deleted for good) and unfinished uploads, if older than a day |

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
```

Back up `<data>` like any folder (stop the server, or copy `catalog.log` first and the blobs after).

### The admin page

`https://<your server>/admin` manages the server from a browser:

- **Users:** every user with their photo count, storage used and devices; add one, reset a password, make or remove
  an admin, remove a user (their files stay in `users/NAME/` until you delete them).
- **Devices:** a user's signed-in devices (name, id, last seen); sign one out (it must sign in again).
- **Server:** version, the data folder and the free space on its disk, the address it listens on, whether it serves
  the web build, and photo-file clean-up (**Check** = `gc --dry-run`, then **Remove** = `gc`).
- **Connect:** the address to type into Settings ▸ Sync on each device (the one you opened the page with).

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

## Connect your devices

Every client needs the same three things: the **server address** (`https://photos.example.com`, or
`http://machine-name:8080` on a tailnet), a **user name** and its **password**, as added on
[the admin page](#the-admin-page) or with `lightcraft-server user add`. The password goes to the server once; the
device gets its own token (kept in the library's `sync.json`), which revoking the device (admin page or
`lightcraft-server device revoke`) or Sign Out ends.

| Client | How |
|---|---|
| Desktop app | **Settings ▸ Sync** (or click the cloud icon in the top bar): server, user, password, **Sign In** |
| Browser | open the server's address: the server serves the web build, and Settings ▸ Sync has the address filled in |
| Command line / agents | `lightcraft-cli run --library DIR sync.signIn server=… user=… password=… sync.now wait=true` |
| iOS (later) | the same three fields, the same API |

Users and devices are managed on the server: on [the admin page](#the-admin-page) or from its command line
(`user add / passwd / admin / remove / list`, `device list / revoke`, `gc`; see the table above).

## What happens at sign-in

- **The first library** signed in to an empty server **uploads itself**: every photo's original, a smart preview
  (≤ 2560 px, ~1 MB) and a mini preview (≤ 512 px) built on this device, then the whole catalog.
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
| `GET /api/ops?since=N&limit=M` | `{head, ops: [[seq, op]…], presets}` (presets = the presets document's version) · `410`: reload the snapshot |
| `POST /api/ops` | `{base, ops}` → `200 {head}` · `409 {head}` (behind: pull first) · `422 {index, error}` (op `index` doesn't apply; nothing was) |
| `HEAD`/`GET`/`PUT /api/blobs/{original\|smart\|mini}/{hash}` | photo files by 128-bit content hash; `GET` takes `Range`; an original is only kept if its bytes hash to its name, previews must be LightCraft previews |
| `GET`/`PUT /api/presets` | `{version, presets}`; `PUT` with a stale `version` → `412` with the current document |
| `GET /…` | the web build (`--web`) |

Ops are the catalog's own (`lightcraft_catalog::Op`); the server applies each push to its copy of the library, all
or nothing, so it never stores a change that doesn't apply. Device-local ops (file paths, Local folders, History) are
refused.

## Limits

v1, honestly:

- **No iOS app yet** (it would reuse this protocol and the sans-IO device code).
- **In the browser** the web build signs in to the server that serves it (same origin; no cross-origin servers). Synced
  previews and downloaded originals are kept in the browser's storage. Commands that read a photo's pixels on the main
  thread (auto settings, export) need its original there: **Photo ▸ Download Originals** first. A photo imported in
  the browser is uploaded with previews built in the page (slow for large raws).
- Preferences, LUT profiles and export / metadata / filter presets stay per device.
- **No merging of two existing libraries**, no sharing with other people, no shared albums or links.
- **Originals downloaded to a device are kept** until you delete them (no automatic eviction under a size budget yet).
- **The server never compacts its log** (it only grows; a pull reads it whole when behind). Fine for one person's
  libraries; compaction (and `410` re-bootstraps) is the upgrade path.
- **Uploads aren't resumable**: an interrupted upload starts again.
- **One request per thread, plain HTTP**: meant for a home server behind a proxy or on a tailnet, not the open
  internet at scale.
