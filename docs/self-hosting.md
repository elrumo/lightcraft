# Self-hosting LightCraft: deploy the server, connect your devices

This guide takes you from nothing to a LightCraft server of your own, with your desktop, browser and iPhone sharing one
library — like Lightroom's cloud, on a machine you own. The reference for everything here (protocol, merge rules,
limits) is [sync.md](sync.md).

What you'll have at the end:

- **One library per person**, shared by all their devices: photos, edits, albums, ratings, keywords, presets.
- **Your existing photo folders** (a NAS share, a backup disk) in that library, read where they are: never copied,
  moved or changed ([library folders](sync.md#library-folders-photos-already-on-the-server)).
- **The web app** at the server's address, and an **admin page** at `/admin` for users, devices and folders.

Sync is optional: LightCraft works without an account or a server, and nothing leaves a device until it signs in.

## 1. Pick how your devices will reach the server

| Setup | Address on your devices | HTTPS | Best for |
|---|---|---|---|
| **A. Your own domain** (Docker Compose + Caddy) | `https://photos.example.com` | automatic (Let's Encrypt) | reaching it from anywhere; every client, browser included |
| **B. Tailscale** | `https://nas.your-tailnet.ts.net` (with `tailscale serve`), or `http://nas:8080` | from Tailscale, or the tunnel itself | private access from anywhere, no open ports |
| **C. Home network only** | `http://192.168.1.20:8080` | none (traffic stays at home) | trying it out; devices that never leave home |

The server itself speaks plain HTTP. **Never open its port (8080) to the internet**: passwords and photos would
travel in the clear. Put it behind Caddy (A), keep it on a tailnet (B), or keep it on your home network (C).

## 2. Deploy the server

You need a machine that stays on — a home server, a NAS that runs Docker, a small VPS — with:

- **Docker** (with Compose) for options A and B; or a Rust toolchain to [build it yourself](#without-docker).
- **Disk** for what the server keeps: photos uploaded from devices (their originals), plus previews of every photo
  (~1 MB each) and the library itself. Library folders cost only their previews.
- **Memory**: 2 GB at least; 4 GB or more for big raw files (the server decodes each photo once to build its
  previews; `LIGHTCRAFT_PREVIEW_THREADS=1` lowers the peak).

There is no prebuilt server image yet: the steps below build it from the source (the first build takes 10–20 minutes).

```sh
git clone https://github.com/elrumo/lightcraft
cd lightcraft
```

### A. Docker Compose with HTTPS on your own domain

1. Point a DNS name (`photos.example.com`) at the machine, and make ports **80 and 443** reachable from the internet
   (Caddy needs both to get its certificate).
2. Start the server and Caddy. `LIGHTCRAFT_PHOTOS` is optional: a folder of photos already on this machine, mounted
   read-only at `/photos` inside the container for [library folders](#add-library-folders-optional).

   ```sh
   cd apps/lightcraft-server
   LIGHTCRAFT_DOMAIN=photos.example.com LIGHTCRAFT_PHOTOS=/srv/photos docker compose up -d --build
   ```

   To keep these settings, put them in `apps/lightcraft-server/.env` (`LIGHTCRAFT_DOMAIN=…` on one line,
   `LIGHTCRAFT_PHOTOS=…` on the next) and run `docker compose up -d --build` from then on.
3. Check that it runs: `docker compose ps` shows the server as `healthy`, and
   `https://photos.example.com` opens the web app.
4. Get the one-time setup code: `docker compose logs lightcraft` → `… enter the setup code ABCD-1234`.

Then go on with [first-run setup](#3-first-run-setup).

### B. Docker with Tailscale (or on your home network)

Build the image once, from the repository's root:

```sh
docker build -f apps/lightcraft-server/Dockerfile -t lightcraft-server .
```

**On a tailnet**, with HTTPS from Tailscale (turn on MagicDNS and HTTPS certificates in the tailnet's admin console):

```sh
docker run -d --name lightcraft --restart unless-stopped -p 127.0.0.1:8080:8080 \
  -v lightcraft-data:/data -v /srv/photos:/photos:ro lightcraft-server
tailscale serve --bg 8080          # → https://<this machine>.<your tailnet>.ts.net
```

Devices on your tailnet then use `https://<machine>.<tailnet>.ts.net`. (Without `tailscale serve`, publish the port on
the tailnet address instead — `-p 100.x.y.z:8080:8080` — and use `http://<machine>:8080`: the tunnel encrypts it.)

**On the home network only (C)**, publish the port to the network:

```sh
docker run -d --name lightcraft --restart unless-stopped -p 8080:8080 \
  -v lightcraft-data:/data -v /srv/photos:/photos:ro lightcraft-server
```

and use `http://<the machine's address>:8080` (allow port 8080 in its firewall).

In both cases: leave out `-v /srv/photos:/photos:ro` if you have no photo folders to add; `docker ps` shows the
container as `healthy`; `docker logs lightcraft` shows the setup code.

### Without Docker

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked --version "$(sed -n '/^name = "wasm-bindgen"$/{n;s/version = "\(.*\)"/\1/p;}' Cargo.lock)"
cargo xtask web                                   # the web app → target/web
cargo build --release -p lightcraft-server        # → target/release/lightcraft-server
sudo install -m 755 target/release/lightcraft-server /usr/local/bin/
sudo mkdir -p /opt/lightcraft && sudo cp -r target/web /opt/lightcraft/web
```

Run it as a service (Linux, systemd), on this machine only, behind Caddy or `tailscale serve`:

```ini
# /etc/systemd/system/lightcraft.service
[Unit]
Description=LightCraft sync server
After=network-online.target

[Service]
User=lightcraft
Environment=LIGHTCRAFT_DATA=/srv/lightcraft LIGHTCRAFT_LISTEN=127.0.0.1:8080 LIGHTCRAFT_WEB=/opt/lightcraft/web
ExecStart=/usr/local/bin/lightcraft-server serve
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```sh
sudo useradd --system --home /srv/lightcraft --create-home lightcraft
sudo systemctl enable --now lightcraft
journalctl -u lightcraft          # the setup code
```

Caddy in front of it is two lines (`photos.example.com { reverse_proxy 127.0.0.1:8080 }`); another proxy must allow
large request bodies (raw files: nginx's `client_max_body_size` defaults to 1 MB).

## 3. First-run setup

Open the admin page: `https://photos.example.com/admin` (or `http://<address>:8080/admin`).

1. **Create the admin.** Enter the setup code from the log, a user name and a password. (The code makes sure nobody
   else can claim a server that is already reachable; ten wrong tries replace it with a new one, logged again.)
2. **Add users** (Users ▸ Add user): one per person; each gets their own library. An admin is also an ordinary user.
3. **Overview ▸ Connect a device** shows the address to type on each device, with a Copy button.

The same from the command line, if you prefer (`docker compose exec` with Compose, `docker exec -it lightcraft`
with plain Docker, or `lightcraft-server` directly):

```sh
docker compose exec -it lightcraft lightcraft-server user add ann --admin     # asks for the password
docker compose exec -it lightcraft lightcraft-server user add ben
```

### Add library folders (optional)

Folders of photos already on the server become part of a user's library, in place. On the admin page: **Users ▸
the user ▸ Library folders ▸ Add folder**, then the folder's path **as the server sees it** — inside Docker, under `/photos` — and
the name devices show (say `/photos/ann` named `Photos`). Or:

```sh
docker compose exec lightcraft lightcraft-server folder add ann /photos/ann --name Photos
```

The server reads the folder right away (the tab shows the progress: files read, new photos, previews still to
build) and again every 15 minutes. Ratings, labels, keywords and Lightroom edits in XMP sidecars come along. Nothing
in the folder is ever written; the container only needs to read it (it runs as uid 10001: the files must be readable
by others, which is the usual `644`/`755`). Two people can be given the same folder; each gets their own photos of it.

## 4. Connect your devices

Each device needs the **server address**, a **user name** and its **password**. The password is sent once; the
device keeps a token of its own, which the admin page can sign out (Users ▸ the user ▸ Devices).

**Which library signs in matters:**

- The **first** device with photos to sign in to an empty server **uploads its library** (originals and previews).
  If a library folder already filled the server, it isn't empty.
- **Every other device signs in from a new, empty library**, and fills from the server. A library that has photos of
  its own can **merge** into the server's: tick *Combine with the photos already on the server* in Settings ▸ Sync
  before signing in (photos are matched by what they are, nothing is removed on either side; without the tick it
  tells you what a merge would do and signs nothing in).

Address typing: `photos.example.com` means `https://`; a bare address on your home network or tailnet
(`192.168.1.20:8080`, `nas:8080`, `100.101.102.103:8080`, `*.local`) means `http://`. Type the scheme to choose.

### Desktop (macOS, Windows, Linux)

Sync isn't in a released build yet: build the app from the same source as the server
(`cargo build --release -p lightcraft`, then run `target/release/lightcraft`; on a Mac,
`packaging/macos/package.sh --arch aarch64` makes a `LightCraft.app` on a DMG). Keep the apps and the server on the
same version.

1. **First device** with your photos: open your library as usual. **Other devices**: **File ▸ Open Library…** (also in
   Settings ▸ General) and pick a new, empty folder.
2. **Settings ▸ Sync** (or click the cloud icon in the top bar): server address, user name, password, **Sign In**.
3. The cloud icon shows what's happening (synced, syncing with what's queued, paused, error). The library fills from
   the server: thumbnails first, the smart preview of each photo you open, originals when you ask for them.

Then:

- **Server Folders** in the sidebar lists the library folders on the server; a click shows a folder's photos,
  right-click to create an album from it, make it available offline or download its originals.
- **Make Available Offline** (right-click an album, or Photo ▸ Make Available Offline) keeps photos editable without
  a network; **Photo ▸ Download Originals** fetches full-size files (Settings ▸ Sync can keep every original).
- **File ▸ Pause Syncing / Sync Now**; Settings ▸ Sync ▸ **Sign Out** keeps the library and its unsent changes.

### Web (any modern browser)

1. Open the server's address (`https://photos.example.com`). The server serves the web app.
2. **Settings ▸ Sync**: the address is filled in; type the user name and password, **Sign In**. A new browser
   library starts with demo photos; they don't count, and the server's library replaces them.

Good to know:

- The library lives in that browser's storage (back it up with File ▸ Back Up Library…; your photos are safe on the
  server either way). Use **one tab** at a time.
- **HTTPS is best.** Over plain HTTP from another machine (`http://192.168.1.20:8080`), the app still works but uses
  IndexedDB, and can't protect against two tabs writing at once.
- Editing works on previews; exporting at full size and a few other commands need the original in the browser:
  **Photo ▸ Download Originals** first.
- To start over in a browser (another user, or a server set up again): open the address with `?reset` (it deletes
  what that browser stores, after asking).

### iPhone and iPad

The iOS app is in an early stage: it builds from the source and installs with [xtool or Xcode](ios.md#build-and-run-with-xtool)
(there is no App Store version yet), and **its sync has not been run on a device yet** — expect rough edges. It
syncs exactly like the desktop app:

1. The app's library starts with demo photos: it signs in as a new library and fills from the server.
2. **Settings ▸ Sync** (in the phone layout, from the menu): address, user name, password, **Sign In**. The device
   appears on the admin page under its name (iPhone, iPad).
3. For a server on your home network, allow LightCraft when iOS asks for **Local Network** access (Settings ▸ Privacy
   & Security ▸ Local Network if you declined).

The app checks certificates against the usual public roots: Let's Encrypt (Caddy) and Tailscale certificates work,
a self-signed one doesn't. Plain `http://` works on the home network and over Tailscale.

### Command line and agents

```sh
lightcraft-cli run --library ~/LightCraft-synced sync.signIn server=photos.example.com user=ann password=… sync.now wait=true
```

## 5. Running it

- **Updating:** pull the source and rebuild — `git pull && docker compose up -d --build` (or `docker build …` and
  re-run the container). The data stays in its volume; the library is upgraded on open. **Update the apps too:** a
  newer server's library can be one an older app can't read.
- **Backups:** everything the server keeps is in its data folder (the `lightcraft-data` volume, `/srv/lightcraft`
  without Docker); your library folders stay where they are, so back them up as you already do. A consistent copy:

  ```sh
  docker compose stop lightcraft
  docker run --rm --volumes-from "$(docker compose ps -aq lightcraft)" -v "$PWD:/backup" \
    debian:bookworm-slim tar czf /backup/lightcraft-data.tgz -C /data .
  docker compose start lightcraft
  ```

- **Storage clean-up** (admin page, or `lightcraft-server gc`): removes files of photos deleted for good.
- **Logs:** `docker compose logs -f lightcraft`; `LIGHTCRAFT_LOG=debug` for more.
- **Scans of library folders:** every 15 minutes (`LIGHTCRAFT_SCAN_INTERVAL`, minutes; `0` = only at start and from
  the admin page's **Scan now**). A disk that isn't mounted is skipped, never read as everything deleted.

## 6. When something goes wrong

| What you see | What to do |
|---|---|
| `can't reach the server: …` | Check the address and its scheme (`https://` behind Caddy or `tailscale serve`, `http://` on port 8080), that the container is `healthy`, and the firewall. |
| `wrong user name or password` | Check them on the admin page (Reset password). |
| `too many wrong passwords: try again in N s` | Wait: after five wrong passwords each try waits longer (up to 15 minutes), per address and per user name. |
| `this library has photos and the server already has a library` | Sign in from a new, empty library (desktop: File ▸ Open Library…). |
| `this library is a copy of another library on that server` | The server was set up again (or the user's library replaced): sign in from a new library; in a browser, open the address with `?reset`. |
| `the server signed this device out` | The device was signed out on the admin page, or its user removed: sign in again. |
| The admin page asks for a setup code | No admin exists yet: the code is in the server's log. |
| A library folder `isn't there (not mounted?)` / `is empty but had photos` | The path is the one inside the container (`/photos/…`); check the volume mount, and that uid 10001 can read it. |
| Photos without thumbnails for a while | Their previews are still being built (admin page ▸ Users ▸ the user ▸ Library folders shows how many are left). |
| Uploads of big raws fail behind your own proxy | Allow large request bodies (nginx: `client_max_body_size 0;`). |

## Security checklist

- Only Caddy (443), `tailscale serve`, or your home network reaches the server — never port 8080 from the internet.
- Strong passwords (8 characters at least are required). Sign out devices you no longer use (admin page ▸ Users ▸ the user ▸ Devices).
- Library folders mounted read-only (`:ro`).
- Keep the server and the apps updated together.

What the server does on its side — throttling password guessing, hashed passwords and tokens, size limits, headers —
is listed in [sync.md → Security](sync.md#security).
