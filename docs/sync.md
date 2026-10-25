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
  copied or moved, and written to (XMP sidecars; uploads filed into one) only where an admin allows it
  ([below](#library-folders-photos-already-on-the-server)).
- **Clients:** the desktop app (Settings ▸ Sync), the web build the server serves (open the server's address in a
  browser, Settings ▸ Sync), the iOS app (a spike: [ios.md](ios.md)) and `lightcraft-cli`/MCP. The protocol is plain
  HTTP + JSON and the device side is sans-IO Rust (`crates/engine/src/sync.rs`, `crates/catalog/src/sync.rs`).
- **It is one person's devices.** Other people see an album through a [link](#share-an-album) (no account): a
  timeline of its photos, a slideshow, Download All, optionally a password, and optionally a form to **add their own
  photos** to it, as a shared album of a cloud photo service collects everyone's pictures of a trip. There are no
  shared libraries, and no accounts for other people.
  A second library can **join** the server's ([when both have photos](#when-this-library-and-the-servers-both-have-photos)).

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
mounted at `/photos` stay where they are). If everyone uses the apps and nobody the web client, add `--target lite`
to the build (`make push-server-lite` pushes it as `…/lightcraft-lite`): the server and its admin page only, built
without the web client, so it builds faster. It runs as uid 10001, which must be able to read the photo folders, and
reports its health to Docker (`lightcraft-server health`).

### Without Docker

```sh
cargo build --release -p lightcraft-server
printf '%s\n' 'a long password' | target/release/lightcraft-server user add ann --data /srv/lightcraft
target/release/lightcraft-server serve --data /srv/lightcraft --listen 127.0.0.1:8080 [--web target/web-site]
```

| Command | What it does |
|---|---|
| `serve [--listen HOST:PORT] [--web DIR] [--scan-interval MIN] [--max-requests N] [--threads N] [--cors-origin SITE]…` | Serve (default `127.0.0.1:8080`, only this computer; `$LIGHTCRAFT_LISTEN`). `--web` (`$LIGHTCRAFT_WEB`): the `cargo xtask web` bundle, served at `/` with the same cross-origin isolation headers as the dev server. `--scan-interval` (`$LIGHTCRAFT_SCAN_INTERVAL`, default 15): minutes between scans of the library folders, 0 = at start and on demand only. `--max-requests` (`$LIGHTCRAFT_MAX_REQUESTS`, default 64): requests answered at once, more are told to try again (the health check never waits). `--threads` (`$LIGHTCRAFT_THREADS`): the CPU threads decodes, develops and the search models share (default three quarters of the cores). `--cors-origin` (once per site; `$LIGHTCRAFT_CORS`, comma-separated; `*` = any site): web builds hosted on another site may call the API ([below](#a-web-build-on-another-site)). `$LIGHTCRAFT_PREVIEW_THREADS`: previews built at once (default half the CPU threads, 1–4, fewer when memory is short: one per 1.5 GiB). `$LIGHTCRAFT_RENDER_THREADS`: photos rendered at once for devices that [export on the server](#exporting-on-the-server) (default 1, or 2 with 8 GiB of memory or more; a render takes about as much memory as the same export on a desktop). These four limits are worked out from the machine (a container's CPU and memory limits count); an admin changes them on the admin page (Overview → Limits, kept in `<data>/limits.json`) unless the deployment set them here. Previews, renders and requests follow a change at once, the CPU threads at the next start. Work over a limit waits: previews in their queue, renders and requests are told to try again and devices do |
| `user add NAME [--admin]` / `user passwd NAME` | Add a user / change a password (first line of stdin, or `$LIGHTCRAFT_PASSWORD`; 8 characters at least). Works while the server runs |
| `user admin NAME on\|off` | Let a user sign in to [the admin page](#the-admin-page), or stop them (the last admin stays) |
| `user quota NAME SIZE\|off` | The most the server keeps for `NAME` ([storage limits](#storage-limits)): `500M`, `50G`, `1.5T` (powers of 1000, as the admin page shows sizes; `50GiB`: of 1024); `off` = no limit |
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
<data>/users.json                              users: argon2id password hash, library id, admin, storage limit
<data>/server-presets.json                     presets shared with every user, built-in ones hidden for all { version, items }
<data>/video.json                              lighter videos: on or off, format, size, quality, who makes them (admin page)
<data>/users/<name>/library/catalog.log        every change from every device, in order
<data>/users/<name>/blobs/<kind>/<xx>/<hash>   photo files by content: original | smart | mini | video (a clip's lighter copy)
<data>/users/<name>/video/<hash>.json          what became of a clip: its copy's record, "already efficient", or why it failed
<data>/users/<name>/presets.json               user presets { version, presets }
<data>/users/<name>/docs/<name>.json           the other shared settings { version, items }, one file each
<data>/users/<name>/shares.json                album links (with their tokens and passwords' argon2id hashes: server's eyes only)
<data>/users/<name>/shares/cache/              pictures rendered for album links (made again on request)
<data>/users/<name>/devices.json               signed-in devices: name, id space, SHA-256 of the token
<data>/users/<name>/folders.json               library folders as last scanned: each file's size, time, hash, photo
```

`users.json` and `devices.json` are readable by the server's user only.

Back up `<data>` like any folder (stop the server, or copy `catalog.log` first and the blobs after).

### The admin page

`https://<your server>/admin` manages the server from a browser. It has five sections (a side bar on wide
screens, tabs on a phone), follows the system's light or dark mode, and confirms anything destructive in a dialog:

- **Overview:** users, photos, videos, signed-in devices and free disk space at a glance; **Server load** (the server's CPU
  and memory, and the whole machine's, as a number and a chart of the last hours; read from `/proc` and the container's
  cgroup files, so on Linux and in Docker only: elsewhere the page says so and shows the requests alone) and
  **Activity** (what each user's devices asked of the server since it started: requests, data received and sent, the
  time spent answering, when it last called; a signed-out device leaves the list); **Connect a device** (the address
  to type into Settings ▸ Sync on each device, the one you opened the page with, with a Copy button); and the
  server's details: version and the commit it was built from, data folder, the address it listens on, whether it
  serves the web build and that build's version and commit (its `version.json`, written by `cargo xtask web`).
- **Users:** every user with their photo, video, album and device counts and storage used (the files the server keeps for
  them *and* the photos in their library folders, which it only reads: a user page says how much of it is each);
  **Add user**. Open a user for their page: reset the password, make or remove an admin, remove the user (their
  files stay in `users/NAME/` until you delete them), and two tabs:
  - **Storage limit:** the most the server keeps for them ([below](#storage-limits)), and how much of it they use.
  - **Devices:** their signed-in devices (name, id, when signed in, last seen); sign one out (it must sign in again).
  - **Shared links:** the [album links](#share-an-album) they made; revoke one.
  - **Library folders:** their [library folders](#library-folders-photos-already-on-the-server): add one (a path on
    the server, and the name devices show), remove one, whether the server can see it, the names to leave out of the
    scan (**Skipped names**), **Scan for changes** (reads only the files that are new or changed) and, from the arrow
    beside it, **Rescan everything** (reads every file again, [below](#library-folders-photos-already-on-the-server)),
    **Build missing previews** and, from its arrow, **Rebuild all previews**, and what the
    last scan found (new, moved, changed, missing, files that couldn't be read) with progress while it scans and
    while previews are built. Each choice in those menus says what it does, and the two that take hours on a big
    library ask first.
- **Presets:** develop presets every user gets, beside their own. **Import presets** takes LightCraft (`.lcpreset`),
  XMP and Lightroom (`.lrtemplate`) presets and zips of them (the same reader as a device's Import Presets; a preset
  already there is skipped); then rename a preset, move it to another group, delete it, or rename or delete a whole
  group. Below, a switch per built-in group and preset hides it from every user. Devices get the changes the next time
  they sync and show these presets like the built-in ones: users can favourite and hide them, not change them. Two
  admins changing them at once: the second is told and shown the first one's change.
- **Search:** what the server does with each user's photos so their devices don't have to. A switch turns search by
  description on for every device that signs in (the web app, iPhone, other computers find photos by describing them
  without downloading the model); another does the same for finding photos by the words printed in them; **People**
  finds the faces in photos, one person at a time because faces are personal data. Each needs a model: if it isn't on
  the server yet the page shows its size and licence, downloads it onto the server (resumable, you can leave the page)
  and turns it on. Turning a switch off keeps the files. It also shows how far each user's photos have got. The page
  opens with a strip of the six jobs (previews, videos, descriptions, words, sensitive content, people) with their
  percent, state and time left; below, one card per job holds its switch, schedule, per-user progress and its model
  (size, licence, in memory or not, Process again, Remove). It uses the width of a big screen.
- **Processing ▸ Videos:** lighter copies of the library's videos ([docs/video.md](video.md#lighter-copies)). A switch
  turns them on for everyone (off by default) and a form sets the format (H.264, or HEVC for a Mac helper), the
  biggest size (as the video is, 1080p, 720p, 480p), the quality (smaller, balanced, better: a bitrate worked out
  from the picture's size and rate, about 5 Mbit/s at 1080p30 and 2.5 at 720p30 for Balanced), whether to keep only
  copies at least 15% smaller than the video, whether a Mac helper may make them and whether the server may when none
  does. The panel shows how many videos are done of how many, how many are left with the time that takes at the
  pace so far, what is being made now, the originals' size against the copies' and what they saved, the helper
  ("none has asked yet" or when it last did), a list of the videos that failed with the reason each, and **Pause** /
  **Resume**, **Encode existing videos now** (makes the copies again that were made with other settings) and **Try the
  failed ones again**. With nothing to make them (no helper has asked, the server's own encoding off) it says it is
  waiting for a helper.
- **Storage:** the disk holding the data folder (used and free), how many photos and videos the users have (and, once the server
  keeps lighter copies of videos, the originals' size, the copies' and what they saved), storage used by each user (as above; hover a name for
  the split), and photo-file clean-up
  (**Check for unused files** = `gc --dry-run`, then **Remove files** = `gc`).

**First run:** while no user is an admin, the server prints a one-time **setup code** to its log
(`docker compose logs lightcraft`, or the terminal running `serve`). The admin page asks for it before it creates the
first admin, so a server that is already on the internet can't be claimed by whoever finds it first; ten wrong codes
replace it with a new one (logged again). Already have users? `lightcraft-server user admin ann on` (or
`user add NAME --admin`) instead.

Admins sign in to the page with their user name and password. That opens an admin session of its own (not a device:
it can't sync, and a device token can't manage the server). The page keeps it in a cookie (`lc_admin`: `HttpOnly`, so
no script can read it, `SameSite=Strict`, and `Secure` behind an HTTPS proxy), so closing the tab or the browser doesn't
sign you out. **Keep me signed in** (on by default; turn it off on a computer you share) keeps the session for 30 days
after you last used it, up to 180 days after you signed in, and across restarts of the server (`<data>/admin-sessions.json`
holds a hash of each kept session's token, never the token, and is readable by the server's user only). Without it, the
session ends with the browser, after two hours without use, or when the server restarts. Sign Out ends a session at
once; a new password for the admin (from the page or `user passwd`), or no longer being an admin, ends all of theirs
(the browser that changed its own password stays signed in), and an admin keeps at most 20 kept sessions (signing
in on a 21st browser ends the one used longest ago). A request that changes something must carry the header
`X-LightCraft-Admin` when it signs in by cookie, which a form on another site can't send. Scripts and native
(non-Rust) frontends send the token `POST /api/admin/login` answers with as a bearer instead, and need no header.
An admin is also an ordinary user with a library of their own.

The data folder, the listen address and the domain are deployment settings, so the page shows them but doesn't change
them: `--data` / `LIGHTCRAFT_DATA`, `--listen` / `LIGHTCRAFT_LISTEN`, and the domain in the reverse proxy
(`LIGHTCRAFT_DOMAIN` with the compose file). Its JSON API is under `/api/admin/` (`apps/lightcraft-server/src/admin.rs`).

A native (non-Rust) frontend can offer the same console as screens. It calls the same API through the engine host
(`host.admin.*`, [engine-host.md](engine-host.md)) and asks for the admin password itself, since the Sync sign-in is a
device's.

### Storage limits

An admin can limit what the server keeps for a user, as a family plan's share of a cloud service: admin page ›
Users › the user › **Storage limit**, or `lightcraft-server user quota ann 200G` (`off` lifts it). What counts is
what the server stores for them: the originals their devices upload, smart and small previews, and photos visitors
add through their [album links](#share-an-album). **Library folders don't count** (they are the admin's folders,
read in place).

- An upload that would go over the limit is refused (`507`, with how much is used of how much): a device keeps the
  photo and tries again later, as after any failed upload (the sync popover says why); a visitor's form says the
  album's owner has no room left. Nothing already on the server is deleted.
- Devices are told the limit (`GET /api/usage` → `quota`): **Settings ▸ Sync ▸ Storage** shows "x of y used", and
  warns at 90 %.
- The smallest limit is 16 MB. Without one (the default) a user may fill the disk.

## Search by description

The server can also search each user's photos by what is in them, so the web build, iOS and other
computers need no model: `GET /api/search?q=…`, an index of vectors per user that devices may add to, and the admin
page's **Processing** search switch (or `lightcraft-server model download --accept-licences`) to install the model (about 1.5 GB,
loaded on first use, ~1.9 GB of memory while loaded). `lightcraft-server model disable|enable [--text]` are the switches from a shell. It can also read the text in photos (`model download --text`) and, for a user an admin allows
(`lightcraft-server user faces NAME on`, after `model download --faces`), find the people in their photos for devices to
name. Routes, privacy and limits: [`search-people.md`](search-people.md).

## Library folders: photos already on the server

A self-hosted server usually sits next to the photos: a NAS share, a backup disk, folders of past years. An admin
can make such folders a user's **library folders** (admin page › Users › the user › Library folders, or
`lightcraft-server folder add ann /photos/ann --name Photos`). The server reads them in place:

- **Nothing in them is copied, moved or changed.** Mount them read-only (`-v /srv/photos:/photos:ro`). The server
  keeps only the previews it builds and an index (`folders.json`); a photo's original is sent to a device straight
  from its folder, when the device asks for it (Download Originals, or keeping originals on the device). A folder
  can be made **writable** (below); the photos themselves are still never opened for writing.
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
- **Writable folders (opt-in, per folder).** An admin can let the server write in a library folder (admin page ›
  the user › Library folders › **Writing**, or `lightcraft-server folder writable ann Photos on`; add
  `--writable` / `--imports` to `folder add`). The folder must be mounted read-write, writable by the server's user
  (uid 10001 in the image; `:rw` in the compose file); the server tries a file before it says yes, and the admin
  page shows when a folder that should be writable isn't. Three things then happen, and nothing else:
  - **XMP sidecars:** when a device changes a photo in such a folder — rating, flag, colour label, develop settings,
    title, caption, keywords — the server writes its `.xmp` a few seconds later, beside the photo, merged into one
    that's there (the same file, naming and merge as the desktop app's *Automatically Write Changes into XMP*; a
    raw+JPEG pair shares `IMG_1.xmp` for the raw and `IMG_1.JPG.xmp` for the JPEG). Lightroom and other programs
    see the edits. If another program changed that sidecar since the server last read it, the server doesn't
    overwrite it: it reads it first (a scan is started), and what that program wrote wins; the next change goes out.
  - **Companion files:** a device can send a file that goes beside a photo, an XMP sidecar or an edit file such as
    `IMG_1-Edit.tif` (`PUT /api/companions/<content hash>?name=<file name>`; the Lightroom import does). The server
    keeps it under `blobs/companion/<hash>/` and puts it beside the photo, never over a file that is there; a photo
    in no writable folder keeps the file until it is filed into the imports folder.
  - **Uploads are filed here, if it is the user's imports folder** (`--imports`; one per user; implies writable).
    A photo a device uploads would otherwise live in the server's own store (`blobs/original`, not a place you
    browse). In an imports folder it becomes `<folder>/<year>/<yyyy-mm-dd>/<name>` (by capture date, else import
    date; a name already taken gets `-1`, `-2`…, never replaced), is the photo's place on the server like any
    library-folder photo, and the server's own copy is deleted once that's recorded. The file is copied under a
    temporary name and linked into place; if the server stops in between, the next scan recognises the file by its
    content. Previews stay in the server's store.
- **Scans** run when the server starts, every 15 minutes (`--scan-interval`), when a folder is added and on demand
  (the admin page's **Scan for changes**, `lightcraft-server scan`). Files whose size and time didn't change aren't read
  again, and neither are the ones that couldn't be read, until the server restarts. The admin page's **Rescan
  everything** (`POST /api/admin/users/{name}/rescan`) reads every file again, changed or not, and tries again the ones
  that couldn't be read: it finds a file whose content changed but whose size and time didn't, and it takes as long
  as reading the whole library. A file with the content it had leaves its photo, edits and sidecar as they are.
  - a new file becomes a new photo, with smart and mini previews built on the server (`LIGHTCRAFT_PREVIEW_THREADS`),
    in the order the scan found the files — except a photo a device is showing and doesn't have a preview of yet:
    asked for, its previews are built before the rest (see [Photos on a device](#photos-on-a-device)), so what a phone
    shows doesn't wait for the rest of a big library's previews;
  - a file that moved or was renamed keeps its photo, edits and albums (same content, its old place gone);
  - a file changed in place keeps its photo and edits and gets new previews;
  - a copy of a file is a photo of its own;
  - a file a device had uploaded already becomes that photo's place on the server, not a second photo, and a device
    never uploads a file the server has in a folder.
- **Files cut short.** A copy or download that stopped part-way keeps the start of a photo and loses its end. It
  still opens (a raw shows its camera JPEG, a DNG or CR2 develops what is left), so it looks wrong rather than broken.
  The admin page's **Check files** (the user › Library folders, `POST /api/admin/users/{name}/check`, or
  `lightcraft-server check [NAME]`) checks every original of the user, in their library folders and in the server's
  store, from each file's structure, reading only its headers: every box of a CR3 / HEIC / MP4 / MOV and every strip
  and tile of a DNG / CR2 / NEF / ARW / TIFF must end inside the file, a JPEG / PNG / GIF must end with its end
  marker, a WebP or PSD must be as long as it says. It lists each damaged file with what is missing and its photos,
  as a CSV too, and changes nothing; copy those files again from a whole copy, then scan. The last report stays in
  the user's folder (`integrity.json`). A JPEG that ends in zeros passes (some phones write them so, and they decode
  whole), even if a download stopped in a run of zeros. Measured on one library: 1,424 of 148,393 files cut short.
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
  links aren't followed, files larger than 2 GiB aren't read (a TIFF is, in part: its tables, small values and first image, so a Photoshop file whose size is mostly layers imports; the limit is 64 GiB), and every file is decoded under a guard: a damaged
  one is reported, never fatal.

## Connect your devices

Every client needs the same three things: the **server address** (`https://photos.example.com`, or
`http://machine-name:8080` on a tailnet), a **user name** and its **password**, as added on
[the admin page](#the-admin-page) or with `lightcraft-server user add`. The password goes to the server once; the
device gets its own token (kept in the library's `sync.json`), which revoking the device (admin page or
`lightcraft-server device revoke`) or [Sign Out](#signing-out) ends. A bare `photos.example.com` means `https://photos.example.com`;
a bare address at home or on a tailnet (`localhost`, a private or Tailscale IP address, a one-word name such as `nas`,
`*.local`, `*.lan`) means `http://` (type `https://` to say otherwise).
The device is listed on the server under the computer's name (`$LIGHTCRAFT_DEVICE` to choose another).

| Client | How |
|---|---|
| Desktop app | **Settings ▸ Sync** (or click the cloud icon in the top bar): server, user, password, **Sign In** |
| Browser | open the server's address: the server serves the web build, and Settings ▸ Sync has the address filled in; a web build hosted on another site works too ([below](#a-web-build-on-another-site)) |
| Command line / agents | `lightcraft-cli run --library DIR sync.signIn server=… user=… password=… sync.now wait=true` |
| iOS (spike, [ios.md](ios.md)) | **Settings ▸ Sync**, as on the desktop; the library is kept in the app's Documents folder |

Users and devices are managed on the server: on [the admin page](#the-admin-page) or from its command line
(`user add / passwd / admin / remove / list`, `device list / revoke`, `gc`; see the table above).

### A web build on another site

The server serves the web build itself, at its own address, and that needs nothing more. A web build hosted somewhere
else (a static host or CDN) can sign in to the server too, if the server is told that site may call it from a browser:
`lightcraft-server serve --cors-origin https://photos-app.example.com` (once per site, or `$LIGHTCRAFT_CORS`). Then
type the server's address in Settings ▸ Sync on that site. Two things stay the browser's own rules: a page served over
`https://` can only call an `https://` server (no `http://` on a home network), and the library and sign-in live in
that site's browser storage, so each site is a separate device. The admin page is never callable from another site.

## What happens at sign-in

- **The first library** signed in to an empty server **uploads itself**: every photo's original, a smart preview
  (≤ 2560 px, ~1 MB) and a mini preview (≤ 512 px) built on this device, then the whole catalog. A library holding
  only the demo photos uploads nothing: the empty server library replaces them.
- **Other devices sign in from a new, empty library** (Settings ▸ General ▸ Open Library… → a new folder, or a fresh
  install: the iOS and web apps start that way) and get the server's library. A new library holding only the demo
  photos counts as empty (the server's library replaces them). It fills in page by page (2,000 photos at a time,
  compact and gzipped: measured at ~63 bytes a photo for a mostly unedited library, against ~3.4 KB written whole): photos show and their previews download as they
  come, Sync status says how far it got, and quitting carries on where it was the next time (the read starts over
  only if the server has dropped the copy it was serving, e.g. after a restart; changes made meanwhile are kept).
- **A library that already has photos signed in to a server that has some too** is two libraries that never met: their ids mean different photos, so nothing
  syncs until you choose ([below](#when-this-library-and-the-servers-both-have-photos)), or tick *Combine with the photos already on the server* in Settings ▸ Sync (or
  `sync.signIn … merge=true`) to merge at once. A merge never removes anything on either side:
  - photos are matched by what they are (the content hash of the file), not by number — each library numbered its own;
  - a photo on both sides stays the server's and gets what this library added to it (rating, flag, label, edits,
    metadata, versions); where both changed the same value, the server's stays;
  - photos only here are added (with their virtual copies), with their albums, smart albums and stacks; an album with
    the same name in the same place on both sides becomes one album with the photos of both;
  - the changes are made on top of the server's library like any edit, so they reach every device, and the photos'
    files upload as usual. What belongs to this device alone — where its files are, History, Local folders — is kept.
  Merging again changes nothing.
- **Signing out** sends what is left, signs the device out on the server and removes the account's library from this
  device ([below](#signing-out)); signing in again as the same user brings the library back from the server, and another
  user starts from an empty library of their own.
- **File ▸ Pause Syncing** stops talking to the server until resumed; **File ▸ Sync Now** pulls at once (it also pulls
  every 5 seconds and whenever the window comes back to the front).

### When this library and the server's both have photos

Signing in asks (on a phone a page, on a computer a window: **Sync This Library**; *Decide Later* puts it off) what to
do. It shows both libraries' sizes and how many photos both have (the same file). Nothing is pulled, pushed or
uploaded while it waits — the cloud button is amber with a `!`, and its popover, Settings ▸ Sync and
`dialog.syncChoice` offer the choice again. Closing the app doesn't lose it: the next launch asks again. The choices
(`sync.resolveConflict {choice}`; the answer says what happened):

| Choice | What it does |
|---|---|
| **Add this library to the server's** (`upload`) | This library's photos, edits, ratings, keywords, versions, albums, folders, stacks and colour-label names are **added** to the server's library under new ids of this device, and the server's photos come here. Photos both libraries have (same content hash) are kept **once**, as the server has them: where this library knew something the server's copy lacks (a rating, a flag, a colour label, edits, a title, keywords, versions) it is filled in, where both have a value the server's stays. Albums and folders with the same name in the same place are one album (this library's photos are added to it); smart albums keep their rules, pointing at the joined library's albums. Stacks carry over when all their photos did and none is stacked on the server. The originals then upload in the background, as after any import. |
| **Use the server's library instead** (`useServer`) | This library becomes the server's. This library's photos leave the library (their files on this computer are never touched, deleted or moved — copies that import made inside the library's `Originals/` folder stay there too, where the safety copy below points at them) and its edits are gone from the library — see the copy below. |
| **Don't sync** (`cancel`) | Signs out; this library stays as it is. To keep two libraries apart on a computer, open a new, empty library (Settings ▸ General ▸ Open Library…) and sign in from there. |

Before either of the first two changes anything, **a copy of this library's catalog is kept in its folder**
(`catalog-before-sync-<time>.snap`, never overwriting an earlier one). It is a `catalog.snap`: put it in an empty folder
under that name and open the folder as a library to get this library back, photos, edits and albums as they were.
If the copy can't be written (full disk) nothing changes and the choice stays open.

Joining adds photos, it never removes any: nothing of either library is deleted by it, and the server's wins only
where both have a value for the same photo. It is one-way (a library joined to the server's can't be separated from it
again, except by [signing out](#signing-out), which removes it from this device); other devices see the new photos like
any others.

### Signing out

As in Lightroom, the library on a device belongs to the account it is signed in to: signing out waits until everything
has reached the server, then removes the library from the device, and the next account signs in to an empty one. Two
accounts' libraries never meet on a device. **Settings ▸ Sync ▸ Sign Out** (`sync.signOut {confirm: true}`) asks first
("this removes the library from this device; it stays on the server"), then:

1. **It waits for what is left.** The cloud button, its popover and Settings say *Signing out* with the changes and the
   photos still to send (`sync.status` → `state: "signingOut"`, `leaving`, `unsent`, `refused`), and the activity stack
   has a row, *Finishing sync before signing out*, with ✕. Edits made meanwhile are sent too; no photo files are downloaded. It
   ends when the queue of changes is empty, every original the server lacks has been uploaded (one the server says is
   not the file its hash names is reported, not waited for), the shared presets and settings and album links have
   settled, and no import, scan or other work that adds photos is running. A server that is away or full, a storage
   limit, or a photo whose file is missing keeps it waiting: the status says why, LightCraft keeps trying, and Sign Out
   Anyway ends it.
   **Cancel Sign Out** (`sync.cancelSignOut`, the popover's button, or the row's ✕) stays signed in and removes nothing.
2. **The device signs out on the server** (`POST /api/logout`): its token stops working and it leaves the admin page's
   device list. (It is best effort: with the server out of reach at that moment the library is emptied all the same, and
   the device stays listed there until an admin signs it out.)
3. **The library is emptied for the next account:** the catalog and its journal, the queue of unsent changes, the sync
   state, user presets and the other synced documents (export, metadata, filter and curve presets, label and keyword
   sets, LUT profiles that synced (a `.cube` over 4 MiB never does, and stays), the import defaults that don't name a
   place), the files sync made (`sync/originals`, the default
   `Smart Previews` folder, the face-scan files) and everything the screens were showing. Never touched: the copies
   import made inside the library (`Originals/`), a previews folder you chose, the files you imported from, the folders
   you browsed in place (Local folders: they were never on the server), `catalog-before-*` copies, and the account's
   library on the server. Settings ▸ Sync's *On This Device* options go back
   to their defaults.

Signing in as the same user brings the library back from the server like on any new device; another user gets their own.

**Sign Out Anyway** (while it waits, or `sync.signOut {force: true}`) doesn't wait: the library is emptied at once, so
what hasn't been sent is lost, and asks first for that reason. When the library is a folder (not a browser's), a copy is
kept first, whether or not anything was unsent (a server that was set up again, or signed this device out, can't say):
`catalog-before-signout-<time>.snap` (a `catalog.snap`: open its folder as a library, as for the choice above) with
`presets-before-signout-<time>.json` and `prefs-before-signout-<time>.json`; if the copy can't be written, nothing is
removed. A command can't wait for the server's answer, so this way doesn't tell it: the device stays listed there until
an admin signs it out.

A server that signs the device out meanwhile (the device revoked on the admin page, the account removed: any request
answered `401`) never turns the wait into a removal: the library stays as it is, signed out. Settings ▸ Sync then offers
**Remove Library from This Device…** next to the sign-in form: sign in again as the same user to go on, or remove the
library (the same as Sign Out Anyway) to sign in as someone else. A library that never joined the server's (a choice
still waiting, a sign-in that failed) has nothing of it there to lose: it signs out at once and keeps its photos.

If LightCraft quits while it waits, the wait goes on at the next launch; if it quits while the library is being
emptied, the next launch finishes the job (`sync.json` carries a marker) before the library opens, and a sign-in
before then finishes it first, so one account's photos are never sent to another.

- **Desktop, the iOS app (egui) and the browser:** the question is inline in Settings ▸ Sync (on a phone, on its page);
  the browser also deletes the account's preview files from its storage (the originals it holds stay, as imports do).
- **Command line, MCP, control channel:** `sync.signOut confirm=true` returns at once with `state: "signingOut"`;
  in the command line `wait=true` keeps the process running until it is done, or until nothing more can be sent (the
  server away or refusing: the reply says `signingOut` and the status says why; run it again to go on)
  (`lightcraft-cli run --library DIR sync.signOut confirm=true wait=true`). Without `confirm` (or `force`) a library
  that joined an account's refuses, and says why.

## The sync status popover

The **cloud button** — in the top bar on a computer, next to Select and "…" on the phone's grid — shows how syncing
stands (blue and turning while it works, amber with a red `!` on a problem, a choice waiting, signed out or saving
trouble) and opens, as Lightroom's cloud does, a panel (`view.syncStatus`, File ▸ Sync Status):

- **Where it stands**, at the top: up to date, syncing, paused, signed out, signing out (what is left to send), can't
  sync (with the last error, kept in full and wrapped), or the choice above, with the server's address under it. A
  library that never synced says how to start (**Set Up Sync…**); signed out offers **Sign In…**, a choice waiting
  **Choose What to Do…**, signing out **Cancel Sign Out**.
- **This device:** loading the server's library, changes waiting to go, **photos being uploaded** and **files being
  downloaded** with a bar and "12 of 56" (counted since the queue was last empty), and the names of the ones moving
  now; files that couldn't be sent or received.
- **Server:** what the server is doing for this library — scanning its library folders (listing, reading new
  photos: how many of how many, time left), building previews (of library-folder photos and of originals devices
  uploaded), making lighter copies of videos, indexing photos for search, reading the text in them, finding people, or that it has nothing to do
  (and when the folders were last scanned); files or folders in the library folders it couldn't read (listed on the
  admin page). Asked of `GET /api/activity` every 2 seconds, only while the panel is open.
- **Storage:** what the library takes on the server in all (what the server keeps of it, plus the photos read in
  place from library folders), as a share of its disk with what is left free, and how much of the total is those
  photos.
- **Pause / Resume**, **Sync Now** and the gear (Settings ▸ Sync), always at the bottom: when the server has a lot
  to do on a small screen, the sections between the status and the buttons scroll.

Each section is a rounded card with a row per piece of work; a server whose disk is almost full, and changes that
can't be saved, are warnings above them.

The numbers say how many, not which. `sync.queue kind=changes|uploads|downloads [offset] [limit]` lists the photos
behind each of them, a page at a time (100, at most 500): `total` of them in all (the number `sync.status` gives as
`uploads` or `downloads`; for `changes`, the records with changes waiting, where `changes` counts the changes
themselves, `pending` in `sync.status`) and `items` of `{kind, id, name, what, running}`. A change is listed by the record
it is about, with every value of it that is waiting (a photo's `rating` and `develop`, an album's `add`), in the order
they will be sent; a transfer by the photo and the file moving (`original`, `smart preview`, `preview`, `building
previews`), the ones moving now first.

The same numbers are the `sync.activity` command (`lightcraft-cli run --library DIR sync.activity refresh=true`, MCP,
control channel) and part of `sync.status`; `ui.inspect` and `ui.widgets` list the popover and its parts (`icon:cloud`,
`actions:syncStatus`, `section:syncDevice`, `section:syncServer`, `section:syncStorage`, `banner:serverDisk`,
`button:syncPause`, `button:syncNow`, `button:syncSettings`, `button:syncStatusPrimary`).

## Photos on a device

Each device keeps the **whole catalog** (it's small) and **downloads pixels as needed**, by content hash:

| What | When | Used for |
|---|---|---|
| mini preview (≤ 512 px) | a photo whose original isn't here, when the grid or filmstrip shows it (the cells on screen first, then a screen either side; a cell that stays drawn keeps its place in the line however long the ones before it take, one scrolled past before its turn isn't downloaded) — or every such photo with *Download all small previews* | grid and filmstrip thumbnails, rendered here with the current edits (never stale) |
| smart preview (≤ 2560 px) | the photo you open, and everything **made available offline** | the loupe and editing until the original is here (offline: always), exporting at preview size |
| original | the photo you open (once its smart preview is here: the smart preview is an 8-bit proxy that bands when edited), **Photo ▸ Download Originals**, or every photo with *Store the originals of all photos on this device*; not in a browser | editing, full-size export, 1:1 |

- **A preview the server hasn't built yet** (a library folder it is still scanning, an original a device uploaded without
  previews): the server builds it before the rest of its queue and answers `404` with `{"previews": "queued"}`, and the
  device asks again after a few seconds (not a failed transfer: nothing is reported, and it isn't the minute a file the
  server doesn't have waits). `{"previews": "failed"}` is a file the server couldn't read; a plain `404` is a photo it
  has no original of. A `HEAD` only looks.
- **Download all small previews** (Settings ▸ Sync, or `sync.allPreviews {on}`): the whole library browses offline.
  The setting says how many photos lack one and about how much they take (at what the server's small previews weigh
  on average; `sync.usage` → `previewsLeft`). Off, the default: a library of 200,000 photos costs only what you look at.
- **A size limit for downloaded originals** (Settings ▸ Sync ▸ Originals limit, or `sync.originalsBudget {mb}`): beyond
  it the originals used longest ago are deleted, and their photos are the server's again (previews stay; Download
  Originals brings one back). Never an original the server doesn't have, one that is open or selected, or one kept
  available offline, and nothing at all while *Store the originals of all photos on this device* is on. The folder is
  checked about once a minute and when a download lands. No limit is the default. Every photo you open adds its original, so set one on a device with little room.
- **Make Available Offline:** right-click an album (✓ marks it in the sidebar), or **Photo ▸ Make Available Offline**
  for selected photos: their smart previews stay on the device, so they open and edit without a network.
  `photo.makeAvailableOffline {ids, on: false}` on a photo that an offline album holds leaves just that photo out of the
  album's copy (the album switched off and on brings it back). `sync.offline` says how far it has come: per offline
  album the photos it holds and how many are here (a smart preview or the original is on this device), and the ids of
  the photos kept offline that are here (`ready`) and still to come (`waiting`), for a native frontend's album rows,
  photo marks and sync popover. It walks every photo kept offline: ask when the sync state moves, not every frame.
- A downloaded original (`<library>/sync/originals/<hash>/<name>`) becomes the photo's file on this device only. It is
  checked against the photo's content hash before it is used, and a download that breaks off is continued, not started
  again (`<name>.part`); an upload that breaks off is continued from what the server kept.
- A native frontend can say where the open photo's original stands: *Loading original* with how much has arrived, *Original loaded* once it is here, or *Smart preview* while it can't come (paused, offline, a failed download). The state is the host's `sync.original` (`Session::original_status`).
- Photos whose original is on the server are not *Missing Photos*; Info says “Original on the sync server”.
- Photos you import on any desktop device are uploaded with their previews in the background.

### How much space it takes

**Settings ▸ Sync ▸ Storage** shows what this library takes, refreshed while the page is open (**Refresh** asks
at once):

- **On the server:** the originals it keeps, smart previews and small previews (each with its file count), the
  **library folders** (photos read where they are on the server and never copied: LightCraft keeps only their
  previews, but they take that room too), the **total** of all those rows, and how much room the server's disk has
  left (the bar: this library, everything else, free). A library folder is drawn in the bar only when it is on the same
  disk as the server's data (a folder on another disk takes room there, not here); the total counts it either way.
- **On this computer:** the originals downloaded from the server, smart and small previews, the thumbnail cache,
  copies imported into the library, the catalog and edits, their total and the disk's free space. Photos you
  imported from folders on this computer stay where they are and aren't counted. The header says how many photos
  have only previews here.

The same numbers, in bytes, are the `sync.usage` command (`lightcraft-cli run --library DIR sync.usage refresh=true`,
MCP, control channel); `serverTotal` is the server's total. Disk space comes from `df`, so it's missing on Windows and in the browser.

## Share an album

Right-click an album in the sidebar ▸ **Share Link…**: choose when the link ends (never, a day, a week, a month, a
year), whether people can **download the originals**, whether they can **add photos**, and an optional **password**,
then **Create Link**. The server makes the link, `https://photos.example.com/s/<a long random token>`, and the app
puts it on the clipboard (`album.share id=… [expiresDays=…] [originals=true] [password=…] [upload=true]`, or the
dialog: `dialog.shareLink id=…`). Whoever has it opens the album in any browser — no account, nothing to install:

- **A timeline:** the album's photos by the day they were taken (oldest first), with the days it spans. Each photo
  opens on a page of its own, large, with the ones before and after it (**← →** step through them, **Esc** goes
  back: the page's one small script, allowed by its hash and nothing else). **Slideshow** goes through the album by
  itself, five seconds a photo, round and round (no script: the page moves on by itself).
- **The pictures are the photos as you have edited them now** (rendered on the server by the same pipeline an export
  uses, 400 px in the grid and 1600 px when opened), without their metadata (no location, no camera serial number):
  the pages show the day a photo was taken, nothing more. Edit a photo and the link shows the new version. The first
  visit builds the pictures (a few seconds each for large raws; the server renders one at a time by default,
  `LIGHTCRAFT_RENDER_THREADS`); they are kept after that.
- **Download originals** (if the link allows it): one by one, or the whole album with **Download All**, one ZIP of
  the original files as they are (streamed: no waiting for it to be made, its size known from the start).
- **A password** (if the link has one, 4 to 256 characters): the page asks for it before it shows anything, not even
  the album's name. The right one opens this link in that browser for 30 days (a cookie for this link only); wrong
  ones are throttled like sign-ins. The server keeps only the password's argon2id hash.
- **Add photos** (if the link takes them): a form under the photos, with an optional name. What a visitor sends
  becomes **your** photos, in this album, on all your devices — like a photo imported on one of them: its original
  kept by the server (or filed in your imports folder, if you have one), its previews built there, then indexed for
  search like any other. A file your library has already is that photo, put in the album; a file that isn't a photo
  or video the server can read is left out; the page says how many were added, left out, or in the album already. A visitor's name becomes the photo's
  Creator when the file names none. Up to 2 GB a file, 8 GB and 500 files at a time, and within your
  [storage limit](#storage-limits) — set one for a link you post somewhere public.
- **Link previews:** a chat app shows the album's name, its photo count and its first picture for the link (Open
  Graph). A link with a password shows none of that.
- The link can **end** after some days; the default is never, until revoked.
- Settings ▸ Sync ▸ **Shared Links** lists them, with what each allows, **Copy Link** and **Revoke** (`shares.list`,
  `album.unshare id=…`); an admin can revoke any user's on the admin page. A revoked, ended or unknown link is the same
  plain "not found", and guessing is throttled by address.
- Only plain albums can be shared (not folders or smart albums), and only the album's own photos are reachable through
  its link. A photo whose file the server doesn't have yet is left out of the page until it is uploaded.
- The pages ask search engines to stay away and carry their own styles; the link is the secret, so send it to the
  people you mean it for. Visitors can add photos only on a link that lets them; they can never change, remove or
  edit any.

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
- **A busy server is waited for.** The `503` (or a connection the server dropped) makes the export pause for the
  `Retry-After` seconds (at most 30 at a time, 5 without one) and ask again, for up to 15 minutes. The wait shows as a
  "Waiting for the server" row in the activity stack, and the export's ✕ ends it (`cancelled`). After the limit the
  export fails with "the sync server stayed busy for N minutes". `RenderWait`, `crates/engine/src/cloud_sync.rs`.

## What syncs, what stays on the device

| Syncs | Stays on each device |
|---|---|
| photos (added, removed, Recently Deleted, Hidden), ratings, flags, colour labels, label names | where the original is on this disk (renames and relinks of files), Local folders browsed |
| the Hidden password's hash and Blur Sensitive Photos (the `privacy` document, [`hidden.md`](hidden.md)) | whether Hidden is unlocked (it starts locked everywhere) |
| develop settings, versions (incl. automatic ones), capture time edits | edit **History** (each device keeps its own steps) |
| user presets (created or imported on any device, the web and iOS included; rename, move, delete, favourite) | favourites among the built-in presets, recent profiles |
| export, metadata and filter presets, curve presets, colour-label and keyword sets, LUT profiles (with their `.cube` files), the order of the develop preset groups, the import defaults that don't name a place | the watched folder for auto import, the folders and the external editor, the cache size, the search-sharing choices |
| metadata and keywords, analysis results | undo / redo |
| albums, folders, smart albums, stacks | the view (`view.json`, `ui.json`) |
| | Local records (photos seen while browsing a folder, or opened from the Photos library, until added to the library; which Photos library picture is which photo, `photo-library.json`) |

Without a server, a frontend on Apple devices can share the presets, the settings above, the edits and analysis results
of photos opened from the Photos library, and if asked the photos themselves (through iCloud Drive) through iCloud
instead ([below](#icloud-apple-devices-without-a-server)).

A server from before the Hidden album (catalog format 10 or older) would keep a hidden photo shown on the other
devices: a device signed in to one refuses Hide and holds back any hidden change ("Update LightCraft Server first"). The
server says its format with every pull (`format` in the answer to `GET /api/ops`; missing: older).

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
  one edited on both merges field by field. A preset created on a synced device gets an id that includes its id space;
  an imported one gets an id named by its content (its name, group and settings), so a preset pack imported on two
  devices is one set of presets, and two different presets that share a name both stay. Whole collections sync: a
  device holds up to 100 000 user presets (a Lightroom collection imported whole is about 11 000), and what it leaves
  out of the server's document (past that, or not readable as a preset by this version) goes back with what it sends
  as it came, so it is never deleted for the other devices. (Before, a device kept the first 10 000 and sent that back,
  which deleted the rest everywhere, the newest imports first.)

Ids never collide: the server gives each device an id space at sign-in, and ids a device hands out are
`space · 2³² + n` (still exact in JavaScript). The library uploaded first keeps the ids it had.

**Never lose a change:** every change is queued in the library's `sync.outbox` *before* it is written to the catalog
log, and the position in the server's order (`sync.json`) only *after*; a crash in between re-sends or re-pulls, and
both end up the same. A change the server refuses (it no longer applies there) is dropped and the device reloads the
server's state, replaying its other pending changes on top.

## iCloud: Apple devices without a server

Without a server, a native frontend on Apple devices can share the smaller things through the person's own iCloud
(CloudKit's private database: it counts against their iCloud storage, and nobody else, the app's maker included, can
read it). The engine decides what to send and merges what comes back (`crates/engine/src/icloud.rs`, `icloud.*`
commands); the frontend only moves records between it and CloudKit, and turns it on per library (`icloud.enable`).

| Travels through iCloud | Not |
|---|---|
| user develop presets, and the settings a server shares too: export, metadata, filter and curve presets, colour-label and keyword sets, LUT profiles, the import defaults that don't name a place, the order of the preset groups, the hidden presets and groups, the Hidden password's hash and Blur Sensitive Photos | photos imported into the library, unless *Keep Photos in iCloud Drive* is on ([below](#photos-through-icloud-drive)): then they travel through iCloud Drive, not CloudKit |
| the sync server a library is signed in to (its address and user name, which the person's other devices offer when they sign in) | the password and the device's token: every device signs in itself |
| **albums, folders, smart albums and stacks**, and the **versions** of the photos that travel ([below](#albums-stacks-and-versions)) | the Quick Collection, History |
| **edits of photos opened from the Photos library** (develop settings, rating, flag, colour label, title, caption, keywords and the other metadata an XMP sidecar carries), by the content hash of the picture's original; the same for photos kept in iCloud Drive, with Recently Deleted and Hidden | edits of photos whose original stays on the device, History, versions |
| **what the analysis found** in those photos: faces (where, and their face vectors), the text read in them, their search vectors and sensitive scores ([below](#analysis-results)) | names given to people, which faces are whose (each device groups them itself) |

The Photos library already syncs the pictures through iCloud Photos; only the edit travels. Another device applies it
to the same picture as soon as it has it in its library; otherwise it holds the edit until that picture is opened
there (its original is the same bytes, so it hashes the same), and then the photo comes into the library edited (a
History step "Synced from iCloud", no undo step). `icloud.heldAssets` lists those pictures by their cloud identifier
(`photoLibrary.open {cloud}` records it) so the frontend can mark them in the Photos grid.

**A library signed in to a server** syncs its presets, settings and photos through the server, so then only the
server record travels through iCloud. What the device knew of the rest is forgotten when it signs in, and fetched again
when it signs out (sign-out removes the account's presets from the device: they come back from iCloud rather than being
deleted there).

**When two devices change the same thing** (one record per preset, per settings item, per photo): a change made on
one side only wins; changed on both, the newer change wins (each device notes when it first saw its own change); a
removal goes everywhere unless another device changed the item meanwhile. CloudKit refuses a save over a version the
device hasn't seen and hands back its own, which the engine merges like a fetched record, so the devices end up the
same. A record taken from the cloud that this version stores a little differently is not sent back.

**Turning it off** forgets what the cloud has (the cloud keeps it for the other devices, and this device keeps its
presets, settings and edits); turning it on again merges as a new device does: nothing is deleted. Signing out of
iCloud or switching accounts does the same.

### Photos through iCloud Drive

A library without a server can keep its photos in a folder of the person's iCloud Drive, which carries them to their
other devices (*Keep Photos in iCloud Drive*, off until turned on: `icloud.photos {on}`; the frontend names the folder
on each device with `icloud.folder {path}`, its own iCloud Drive container on Apple devices,
`crates/engine/src/icloud/photos.rs`).

- **Imports** that copy their originals put them in that folder instead of the library's `Originals/` (in the folders
  an import makes, `YYYY/YYYY-MM-DD/`), unless the import names another place.
- **Photos already in the library** move there when asked (`icloud.moveIn {limit?}`, up to 64 files a call, each with
  its sidecars; the frontend repeats it while `left` is above zero and a call moved something). Only originals the
  library keeps itself move: photos opened from the Photos library stay (iCloud Photos carries them), and so do files
  referenced where they are. `icloud.status` says how many could (`movable {photos, bytes}`).
- **A photo whose original is in the folder** has an `edit` record like a Photos library picture, which also says where
  its file is (`file`, relative to the folder) and whether it is in Recently Deleted or hidden. Another device that has
  no photo with that picture holds the record and lists the file as wanted (`icloud.wanted {limit?}` → `{files: [{file,
  key?, id?}], total}`). iCloud Drive may not have brought the file to the device yet, so the frontend asks it to and
  hands each file over once it is there (`icloud.adopt {files}`, up to 16 a call → `{added, back, missing, refused,
  left}`). The engine adds it to the library where it is, as an import in place, with the other device's edits. That is
  not an undo step: undoing it would remove the photo from every device.
- **A file iCloud Drive took back** to save space (its photo stays in the library, without its original) is wanted
  again, and `icloud.adopt` shows the photo from it once it has come back.
- **A photo removed for good** on one device is removed from the others' libraries. Its file stays in iCloud Drive:
  LightCraft never deletes originals.
- Paths another device sends are relative and checked: nothing outside the folder is read. Each of the two long jobs
  has an activity row while there is work left ("Bringing photos from iCloud Drive", "Moving photos to iCloud Drive");
  a frontend that stops waiting for files that don't come (deleted in iCloud Drive) says so with `icloud.wanted
  {waiting: false}`, which takes the row away until it asks again.

A library signed in to a server keeps its photos there: *Keep Photos in iCloud Drive* can't be turned on, and an import
puts its originals in `Originals/` again while signed in.

### Through a cloud folder (iCloud Drive, Dropbox, Google Drive)

The same records can travel through any folder a cloud provider carries to the person's other devices, with no
CloudKit host, no account of ours and no server (`crates/engine/src/icloud/folder.rs`).

- **Turning it on:** Settings ▸ Sync, under the server form ("Without a Server"), offers the iCloud Drive, Dropbox and
  Google Drive folders found on the computer (`icloud.cloudFolders`); *Use iCloud Drive* (and so on) is
  `icloud.useFolder {path}`, which turns the iCloud sync on for the library and keeps the folder in the library's
  `icloud.json`. The folder is per device (each computer names its own). Another folder is another cloud: the
  library's records all go there, and nothing is removed from the first. The page shows the folder, how long ago
  the last pass ran (or what went wrong), *Sync Now* and *Stop Syncing* (`icloud.useFolder {path: null}`, which turns
  the sync off again). A library signed in to a server doesn't show it: the server carries everything.
- **The passes run by themselves** (`crates/apphost/src/sync.rs`, shared by the desktop app and the engine host): at
  least every 30 s, sooner (but not closer than 5 s) after something changed here or the app came to the front, and
  at once while a pass left records to write (at most 2000 a pass). The file work runs on a thread of its own; the
  engine's half is applied on the next frame, held back during a slider drag. A pass that takes more than a second has
  a row in the activity stack ("Syncing through the cloud folder").
- **One pass** is `icloud.syncFolder {path?}` (the CLI, tests; the chosen folder by default): it writes what changed
  here, then reads what the other devices wrote and merges it as for CloudKit. `{changed, sent, removed, skipped,
  waiting, more}`.
- **One writer per file.** A provider can't order writes, and two devices writing one file make it keep "conflicted
  copies". So a device writes only `<folder>/LightCraft Sync/<device>/<record>.json`, and the cloud's version of a record
  is the newest file of that name in all the device directories (`modified`, then the device name, so every device picks
  the same one). A removal is a tombstone in place of the record, so an older file of another device can't bring it back.
- **Safe to read.** Files are checked like records from CloudKit: a name that isn't the record's, a file that isn't JSON or is
  too large, a link, or a directory name that isn't a plain device name is left out (`skipped`). A file the provider has
  not brought yet (`.icloud` placeholder) counts as `waiting`; a later pass has it.
- **Use one transport per library**: the folder or a CloudKit host, not both (they share the engine's record of what the
  cloud has).
- **Sync Now** is `icloud.folderNow`: the host's next poll runs a pass at once.
- **Limits.** Every pass reads all the record files (a few thousand small files for a big library; a change list kept
  between passes would avoid it). Tombstones are never deleted. Ratings and the like of photos that are not opened from
  the Photos library or kept in the folder do not travel (only albums, stacks and the other records below do). The
  egui page offers only the providers' usual folders (no folder picker yet) and is English only. Nothing has been run
  against a real iCloud Drive, Dropbox or Google Drive folder, only against a plain one.

### Albums, stacks and versions

Albums, folders, smart albums and stacks travel too, one `album` or `stack` record each
(`crates/engine/src/icloud/collections.rs`), through CloudKit or a cloud folder alike; **versions** travel in the `edit`
record of the photos that have one.

- **By content, not by id.** A record names its album or stack by a uid this device made when it first sent it (in
  `icloud.json`), and its photos by their content key, so another device puts them on the photos it has whatever way
  they got there (opened from the Photos library, kept in iCloud Drive, or imported from the same file on two
  computers). Members this device has no photo for yet are kept and go back with the record, so they are never lost
  here, and join the album when the photo comes.
- **What an album carries:** name, folder or not, the folder it is in, its place among its siblings, a smart album's
  rules, the cover and the members. Not the Quick Collection, and not smart albums whose rules name photos or an album
  (ids mean nothing elsewhere). A stack carries its members top first and whether it is collapsed; it takes the members
  this device has (two at least; one with fewer is held until the photos come).
- **Merging is by record:** changed on both sides, the newer change wins. Photos added to one album on two devices at
  once are therefore not both kept, as a server's member-by-member merge would (limit). A removal removes the album or
  stack on the other devices unless it was changed there; the photos stay. Applying is not an undo step.
- **A library signed in to a server** has them through the server: no `album` or `stack` records travel then.

### Analysis results

A Mac finds faces, reads text, and computes the search vectors and sensitive scores of every photo; a phone may lack
some of those models, or not have run them yet. So what was found in each photo whose edit travels (Photos library
pictures and photos kept in iCloud Drive) travels too, in one `analysis` record per picture
(`crates/engine/src/vision/portable.rs`):

- **Four parts**, each saying which engine made it: `faces` (each face's place, score and face vector), `text` (the
  words read), `vector` (the search vector and its model) and `sensitive` (the score). A device sends a record when it
  has a part the cloud's lacks, and takes only the parts it lacks itself; nothing it found is ever replaced. A picture's
  record is removed only once its edit record is.
- **A device takes a part it can use**: faces from the face model it would run, a search vector only from the same
  search model (another model's vectors don't compare), text and sensitive scores always. A device without the models
  (not downloaded, or too little memory for the search model on a phone) then finds people in its photos, searches the
  text in them (`library.search` words), and blurs sensitive photos with the scores another device worked out, and one
  that has them needn't work out again what another already did.
- Each device groups the faces it has into people itself, and names stay where they were given.

**Files in the library:** `icloud.json` (on, the versions last agreed with the cloud, the change times and the
frontend's own CloudKit state, kept together so that they never disagree after a crash) and `icloud.held` (records
held for pictures or servers this device doesn't have).

**The frontend's side:** it asks `icloud.changes` when the state's `icloud.rev` changes and tells CloudKit about the
records to save and delete; when CloudKit wants a batch it reads them with `icloud.records`; it hands every fetched
record and deletion, and the server's version of every refused save, to `icloud.fetched`, every save CloudKit took to
`icloud.sent`, and its own state to `icloud.transport`. A new `icloud.epoch` means the engine forgot what the cloud has:
the frontend drops its state and fetches everything again (`icloud.reset` asks the engine to forget, after an iCloud
account change).

## Protocol

JSON over HTTP; every route but `login` wants `Authorization: Bearer <token>`. Types: `lightcraft_catalog::sync::proto`.
Big JSON answers (the library, pulled changes, the presets; 16 KiB and up) come gzipped to a device that sends
`Accept-Encoding: gzip`, about a tenth of the bytes.

| Route | Body → answer |
|---|---|
| `POST /api/login` | `{user, password, device}` → `{token, device, space, library}` · `401` |
| `POST /api/logout` | signs this device out |
| `GET /api/me` | `{user, device, library, devices}` |
| `GET /api/snapshot?limit=N[&compact=1][&cursor=C]` | `{defaults, library, seq, catalog, next, total}`: the library after change `seq`, in pages of at most `N` photos (and about 8 MiB), like a cloud photo service's asset listing. With `compact=1` each photo's develop settings (edit, import look, versions) are a JSON merge patch (RFC 7396) against the page's `defaults` (the server's default settings, written first), so an unedited photo takes a few hundred bytes, not ~4 KB, and a default changed in a later version changes no photo. The first page (no `cursor`) also has the albums, stacks and the rest; each page's `next` is the next page's `cursor`, absent on the last; `total` is the library's photo count. Every page comes from the copy taken at the first page, however the library changes meanwhile; the device adopts it after the last page, then pulls the changes since `seq`. `410`: that copy is gone (the server keeps four, and a restart keeps none unless nothing changed): start again. Without `limit` (devices from before paging): the whole library in one answer |
| `GET /api/usage` | what this user takes on the server ([`proto::Usage`](../crates/catalog/src/sync.rs)): `{photos, videos, albums, devices, original, smart, mini, folders: {files, bytes}, foldersOnDisk, disk: {total, free}, quota, video: {originals, encoded, saved}}`; a few seconds old at most. `photos` and `videos` count the library's photos and its videos apart, leaving out Recently Deleted and virtual copies (worked out once per library revision, not per request; a server from before videos counted every record as a photo and says no `videos`, which reads as 0). `video` is what the videos take: `originals` their own files by the sizes the library knows (wherever they are kept), `encoded` the lighter copies the server keeps of them (`blobs/video/<xx>/<content hash>`, none until the server makes them), `saved` the bytes those copies are smaller than the originals they stand for. `original`, `smart` and `mini` are the files the server keeps; `folders` the photos it reads in place from the user's library folders, and `foldersOnDisk` how many of their bytes are on the same disk as its data. What the library takes in all is `original + smart + mini + folders`; `foldersOnDisk` is for the disk's bar (a server that doesn't say it has none on its disk). `quota` is their [storage limit](#storage-limits) in bytes (`null`: none); it counts `original + smart + mini` and the lighter video copies (`video.encoded`) only. A server older than the route answers `404` |
| `GET /api/activity` | what the server is doing for this user now ([`proto::Activity`](../crates/catalog/src/sync.rs)): `{scan: {scanning, phase, done, todo, files, etaSecs, lastScan, problems} \| null, previews: {done, total, etaSecs}, search, text, faces, video: {done, total, etaSecs} \| null}`; `scan` is null for a user without library folders, the others null when not installed / not turned on. `video` is making lighter copies of the user's videos: `done` counts the videos that have a copy or need none, `total` those plus the ones still to do (a failed video is left out), `null` when an admin has it off or there are no videos; a server from before it sends none, which reads as `null`. A server older than the route answers `404` |
| `GET /api/ops?since=N&limit=M` | `{head, ops: [[seq, op]…], presets}` (presets = the presets document's version) · `410`: reload the snapshot |
| `POST /api/ops` | `{base, ops}` → `200 {head}` · `409 {head}` (behind: pull first) · `422 {index, error}` (op `index` doesn't apply; nothing was) |
| `HEAD`/`GET`/`PUT /api/blobs/{original\|smart\|mini\|video}/{hash}` | photo files by 128-bit content hash (`video`: a clip's lighter copy, an MP4 named by the *original's* hash, served as `video/mp4`; see below); `GET` takes `Range`; `GET` of a preview the server hasn't built yet (and has the original of) puts its build first in line and answers `404` `{"error": …, "previews": "queued" \| "failed"}` with `Retry-After: 3`; an original is only kept if its bytes hash to its name, previews must be LightCraft previews and arrive whole; a `PUT` over the user's [storage limit](#storage-limits) is `507`. **Resuming an upload:** an original that breaks off is kept (`tmp/original-<hash>.part`); `HEAD` of a file the server doesn't have yet answers `404` with `Upload-Offset: N` (the bytes kept, `0` if none), and `PUT` with `Content-Range: bytes N-<last>/<total>` carries on from there (`409` with `{"offset": N}` if the server has another number of bytes or another request is writing the file; a plain `PUT` starts over). Downloads of originals resume with `Range`. **`video`:** a `PUT` needs the digest of the copy's own bytes (128-bit, the same hash as a content hash) in `X-LightCraft-Digest` (`400` without), because a copy does not hash to the original's name; the server hashes what it receives and refuses a mismatch (`422`), a file that is not an MP4 or MOV it can read (`422`), one whose length differs from the video's by more than 1% (and half a second; `422`), a hash that names no video in the user's library (`404`), and any copy while lighter videos are off (`409`). It resumes like an original (`tmp/video-<hash>.part`, `HEAD` `Upload-Offset`, `Content-Range`) and may carry `X-LightCraft-Video: {"hardware": true}` (at most 1 KiB) to say a hardware encoder made it. With *Keep only copies at least 15% smaller* on, a copy that is not is dropped, the clip is marked "already efficient" and the answer is `200 {"kept": false, "efficient": true, "srcBytes", "dstBytes"}` (a kept copy answers `200 {}`). A `GET` of a copy that is not there is a plain `404` (no preview is built for it). The copy counts in `Usage.video.encoded` and in the storage limit |
| `GET`/`PUT /api/presets` | `{version, presets}` (`GET` gzipped, see above: a whole collection is tens of MB); `PUT` with a stale `version` → `412` with the current document |
| `GET`/`POST /api/shares`, `DELETE /api/shares/{id}` | the user's album links ([`proto::Share`](../crates/catalog/src/sync.rs): `password` and `upload` say what each asks and allows): `{shares: [...]}`; `POST {album, expiresDays?, originals?, password?, upload?}` → the new link (`400` for a folder or smart album or a password under 4 characters, `404` no such album, `409` over 200 links); `DELETE` takes one back |
| `GET /s/{token}`, `GET /s/{token}/{photo}[?play=1]`, `GET /s/{token}/{photo}/{thumb\|view\|original}`, `GET /s/{token}/download` | **no sign-in**: the album's page, a photo's page (`play`: the slideshow), the pictures, and Download All (a ZIP of the originals) ([above](#share-an-album)); `404` for anything else (unknown, ended or revoked token, a photo not in the album, `original` or `download` on a link that doesn't offer them), `401` until a link's password was typed, `429` when an address keeps guessing, `503` + `Retry-After` while the server is busy rendering |
| `POST /s/{token}/unlock`, `POST /s/{token}/upload` | **no sign-in**: a link's password (`password=…`, a form; the answer sets the cookie that opens the link), and photos a visitor adds (`multipart/form-data`, field `photos`, optional `name`; `303` back to the album with `?added=N&skipped=M`; `403` on a link that doesn't take photos, `507` over the owner's [storage limit](#storage-limits)) |
| `GET`/`PUT /api/docs/{name}` | the other shared settings, `name` one of `export-presets`, `metadata-presets`, `filter-presets`, `curve-presets`, `label-sets`, `keyword-sets`, `lut-profiles`, `prefs`, `preset-groups` (the order of the develop preset groups), `hidden-presets` (the presets and groups the user hid, one item each: `{id: "preset:<id>"}`, `{id: "group:<name>"}`): `{version, items}` (a list; at most 24 MiB), merged by the devices item by item like presets; `PUT` with a stale `version` → `412` with the current document; another name → `404`. Their versions come with every pull (`docs`) |
| `GET /api/docs/server-presets` | the presets an admin shares with every user ([admin page](#the-admin-page) → Presets): `{version, items}`, each preset `{id: "srv.…", name, group, settings}` and one item `{id: "hidden", presets, groups}` (built-in presets and groups hidden for everyone). Read-only (`PUT` → `403`); its version comes with every pull (`docs`) once an admin has written it. Devices take it as it is, never merge or send it |
| `POST /api/admin/login`, `POST /api/admin/logout` | the admin session ([above](#the-admin-page)): `login {user, password, remember?, cookie?}` → `{token, user}`, or with `cookie: true` (the page) `{user}` and a `Set-Cookie: lc_admin=…`; `remember` keeps it for weeks and across restarts; `logout` ends it (and clears the cookie). Anything else under `/api/admin/` takes the token as a bearer or the cookie (with `X-LightCraft-Admin` on what changes something) |
| `GET /api/admin/usage` | (admin session) the Overview's numbers ([`usage.rs`](../apps/lightcraft-server/src/usage.rs)): `{since, supported, cores, memory, history: [{at, cpu, machineCpu, rss, memUsed, requests, out}], users: [{name, devices: [{id, name, requests, in, out, busyMs, lastSeen}]}]}`. `history` is a sample of the machine every 30 s for the last 24 h, averaged to at most 240 points (`cpu`: the server's CPU in cores, `machineCpu`: the whole machine's, 0 to 1, `rss` and `memUsed`: the server's memory and what the machine or container uses, `requests` and `out`: the devices' requests and the bytes sent to them since the point before); `null` where the platform doesn't say (`supported` is false off Linux). The counts are in memory and start again with the server |
| `GET /api/admin/users` | (admin session) every user for the Users and Storage pages (and a native frontend's admin screens, through the same route): `{name, admin, photos, videos, albums, devices, bytes, folderBytes, quota, shares, folders, scan}` plus the videos' numbers `videoBytes` / `videoFiles` (their own files by the sizes the library knows), `videoCopies` / `videoCopyBytes` (the lighter copies the server keeps) and `videoSaved`. `photos` and `videos` leave out Recently Deleted and virtual copies. `bytes` is `null` while the server is still counting the files it keeps |
| `GET`/`POST /api/admin/users/{name}/check` | (admin session) the check of the user's originals for files cut short: `POST` starts one (`409` while one runs), `GET` answers `{running, done, total, report}`, `report` the last finished check (`{finished, files, unchecked, cut: [{path, size, missing?, why, photos}], unreadable}`; `path` is a library-folder place, or `uploads/<hash>` for the server's own copy) |
| `POST /api/admin/users/{name}/scan`, `…/rescan`, `…/previews/{generate\|rebuild\|pause\|resume}` | (admin session) the user's library folders: `scan` reads the files that are new or changed; `rescan` reads every file again; `previews/generate` builds the previews that are missing (and tries again the ones that failed); `previews/rebuild` builds every preview again, replacing the ones there only as each new one is written (so a photo that can't be read keeps its preview); `pause`/`resume` stop and go on, for every user. The answer is the user's folders with their scan status (`full`: this scan reads every file; `previewsRebuilding`) |
| `GET`/`PUT /api/admin/video`, `POST /api/admin/video/{apply\|pause\|resume\|retry-failed}` | (admin session) lighter videos ([`video.rs`](../apps/lightcraft-server/src/video.rs), [docs/video.md](video.md#lighter-copies)). `GET` answers `{settings: {enabled, codec: "h264"\|"hevc", maxHeight: "original"\|1080\|720\|480, quality: "smaller"\|"balanced"\|"better", skipIfSmaller, serverFallback, helper}, state: "off"\|"idle"\|"paused"\|"encoding"\|"waiting", waitingFor: "helper"\|"nothing"\|null, paused, total, have, efficient, failed, noOriginal, left, queuedAgain, etaSecs, perMinute, bytes: {originals, encoded, saved}, serverCanEncode, helper: {seen, there}, encoding: [{user, name, done, total}], users: [{name, total, have, efficient, failed, noOriginal, left}], failures: [{user, hash, name, kind: "unsupported"\|"unreadable"\|"encoder", reason, at}]}`. `PUT` takes the fields to change (a field that is not one, or a value that is not one of its choices, is `400` and names it) and answers the same; the settings are kept in `<data>/video.json`, and a damaged file means the defaults. `apply` makes the copies again that were made with other settings (and the clips judged already efficient under other ones, or with *Keep only copies at least 15% smaller* since turned off) and looks again for originals the server did not have (`409` while lighter videos are off); `pause` and `resume` stop and go on (a pause stops the video being made, which starts again); `retry-failed` puts the failed videos back in the queue. Each answers the same as `GET` |
| `GET`/`PUT /api/admin/presets`, `POST /api/admin/presets/import` | (admin session) the shared presets plus the built-in list (`builtin: [{id, name, group}]`); `PUT {version, items}` replaces them (`412` with the current document when stale; `400` naming a bad item); `import {files: [{name, data: base64}], group?}` → `{version, added, skipped, failed}` |
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
- **A client that stalls is cut off.** The server's own HTTP layer closes a connection whose request head doesn't
  arrive within 10 s, whose body pauses for 60 s, or that is idle for 30 s; it caps a request's head at 32 KiB and
  64 headers, takes at most 512 connections (128 per address) and answers 64 requests at once (`--max-requests`;
  `/api/health` never waits for a place). Request bodies must say their length: `Transfer-Encoding: chunked` gets `411`.
- **Other sites** can call the API from a browser only if listed with `--cors-origin`; nothing is added otherwise. Devices
  authenticate with a bearer token in a header, never a cookie, so a listed site can call the API only as a user who
  signed in to it. `/api/admin/…` never answers cross-origin calls.
- Library folders are set by admins only, must exist, and can't be (or hold) the server's data folder.

## Limits

v1, honestly:

- **The iOS app is a spike** ([ios.md](ios.md)): sync runs on the simulator (sign-in, upload with resume, server-built
  previews, pulling and downloading originals); it has not been run on a device, so TLS from a phone, the local-network
  prompt and syncing while the app is in the background are untested.
- **In the browser** synced previews and downloaded originals are kept in the browser's storage. Auto settings, white
  balance picks and exports up to 2560 px read the photo's preview when its original isn't there; larger exports
  need the original: **Photo ▸ Download Originals** first (the export says so). The browser can't have the server
  render a photo (export runs on the page's main thread), as the desktop and iOS apps can.
- **Settings that describe one computer stay on it**, by design: the folders (library, smart previews, the watched folder for auto import), the cache size, the external editor, what is shared for search, the window layout.
- **Sharing is links to a page**: visitors can see, download (if allowed) and add photos (if allowed), but there are
  no accounts for other people, no comments or likes on the page, and an album shared with several people is still
  one person's album (theirs to edit).
- **The server compacts its log** into a snapshot at 64 MiB and keeps the newest 16 MiB of it (tens of thousands of
  changes): a device behind by more than that reloads the library. (A device that is only a little behind keeps
  pulling; a pull reads just the part of the log it asks for.)
- **Library folders are read-only unless an admin makes one writable** ([above](#library-folders-photos-already-on-the-server)):
  then the server writes XMP sidecars and can file uploads there, never touching the photos themselves. Edits in
  read-only folders stay in the library.
- **Plain HTTP, sized for a household**: TLS is the proxy's or the tailnet's job ([above](#tls-a-reverse-proxy-or-a-private-network));
  the server itself is hardened against stalled clients ([Security](#security)), but it serves a thread per
  connection: right for a home server or a small group, not the open internet at scale.
- **Joining two existing libraries adds only**: [this library's photos are added to the server's](#when-this-library-and-the-servers-both-have-photos) (or replaced by it); photos without a content hash (very old libraries) can't be recognised as the same and are added again.
