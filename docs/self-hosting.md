# Self-hosting LightCraft: deploy the server, connect your devices

This guide takes you from nothing to a LightCraft server of your own, with your desktop, browser and iPhone sharing one
library — like Lightroom's cloud, on a machine you own. The reference for everything here (protocol, merge rules,
limits) is [sync.md](sync.md).

What you'll have at the end:

- **One library per person**, shared by all their devices: photos, edits, albums, ratings, keywords, presets.
- **Your existing photo folders** (a NAS share, a backup disk) in that library, read where they are: never copied
  or moved, and written to (XMP sidecars, and a folder for your devices' uploads) only if you switch that on per
  folder ([library folders](sync.md#library-folders-photos-already-on-the-server)).
- **Links to albums** that anyone you send them to can open in a browser, with no account
  ([sharing](sync.md#share-an-album)): the photos by day, a slideshow, Download All, an optional password, and
  optionally a form for them to add their own photos to the album. You can revoke them any time.
- **The web app** at the server's address, and an **admin page** at `/admin` for users, devices, folders and
  each user's [storage limit](sync.md#storage-limits).

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

- **Docker** (with Compose) for the image, or nothing but the downloaded program for a Mac, Linux or Windows
  computer ([below](#the-program-macos-linux-windows)); a Rust toolchain only to [build it yourself](#from-source).
- **Disk** for what the server keeps: photos uploaded from devices (their originals), plus previews of every photo
  (~1 MB each) and the library itself. Library folders cost only their previews.
- **Memory**: 2 GB at least; 4 GB or more for big raw files (the server decodes each photo once to build its
  previews). The server sizes its work to the machine (CPU threads, previews and renders at once, from the cores and
  memory it sees, container limits included); change the numbers on the admin page under Overview → Limits.

Two ways to get it, both prebuilt for 64-bit Intel/AMD and ARM (Apple silicon, Raspberry Pi 4 and newer, most NASes):

- **The Docker image** on Docker Hub, [`elrumo/lightcraft-server`](https://hub.docker.com/r/elrumo/lightcraft-server)
  (server and web app; `:latest`, or a version such as `:0.14.0`, or `:0.14`). `elrumo/lightcraft-server:lite` leaves
  out the web app (the server and its admin page, for people who only use the apps).
- **The program**, a zip per system from the
  [Releases page](https://github.com/elrumo/lightcraft/releases?q=server): `lightcraft-server-<version>-<system>.zip`
  for `macos-arm64`, `macos-x86_64`, `linux-x86_64`, `linux-aarch64`, `windows-x86_64` and `windows-arm64`. It holds
  `lightcraft-server` (`.exe` on Windows) and a `web` folder next to it, which the server serves by itself.

### A. Docker Compose with HTTPS on your own domain

1. Point a DNS name (`photos.example.com`) at the machine, and make ports **80 and 443** reachable from the internet
   (Caddy needs both to get its certificate).
2. Download the compose file and start the server and Caddy. `LIGHTCRAFT_PHOTOS` is optional: a folder of photos
   already on this machine, mounted read-only at `/photos` inside the container for
   [library folders](#add-library-folders-optional).

   ```sh
   curl -LO https://raw.githubusercontent.com/elrumo/lightcraft/main/apps/lightcraft-server/docker-compose.hub.yml
   LIGHTCRAFT_BIND=127.0.0.1 LIGHTCRAFT_DOMAIN=photos.example.com LIGHTCRAFT_PHOTOS=/srv/photos \
     docker compose -f docker-compose.hub.yml --profile https up -d
   ```

   To keep these settings, put them in a `.env` file next to the compose file (`LIGHTCRAFT_DOMAIN=…` on one line,
   `LIGHTCRAFT_PHOTOS=…` on the next, `LIGHTCRAFT_BIND=127.0.0.1` and `COMPOSE_PROFILES=https`) and run
   `docker compose -f docker-compose.hub.yml up -d` from then on.
3. Check that it runs: `docker compose -f docker-compose.hub.yml ps` shows the server as `healthy`, and
   `https://photos.example.com` opens the web app.
4. Get the one-time setup code: `docker compose -f docker-compose.hub.yml logs lightcraft` →
   `… enter the setup code ABCD-1234`.

Then go on with [first-run setup](#3-first-run-setup). (Every `docker compose …` below needs the same
`-f docker-compose.hub.yml`, or rename the file to `compose.yaml`.)

### B. Docker with Tailscale (or on your home network)

**On a tailnet**, with HTTPS from Tailscale (turn on MagicDNS and HTTPS certificates in the tailnet's admin console):

```sh
docker run -d --name lightcraft --restart unless-stopped -p 127.0.0.1:8080:8080 \
  -v lightcraft-data:/data -v /srv/photos:/photos:ro elrumo/lightcraft-server
tailscale serve --bg 8080          # → https://<this machine>.<your tailnet>.ts.net
```

Devices on your tailnet then use `https://<machine>.<tailnet>.ts.net`. (Without `tailscale serve`, publish the port on
the tailnet address instead — `-p 100.x.y.z:8080:8080` — and use `http://<machine>:8080`: the tunnel encrypts it.)

**On the home network only (C)**, publish the port to the network:

```sh
docker run -d --name lightcraft --restart unless-stopped -p 8080:8080 \
  -v lightcraft-data:/data -v /srv/photos:/photos:ro elrumo/lightcraft-server
```

and use `http://<the machine's address>:8080` (allow port 8080 in its firewall).

In both cases: leave out `-v /srv/photos:/photos:ro` if you have no photo folders to add; `docker ps` shows the
container as `healthy`; `docker logs lightcraft` shows the setup code. On a Synology or QNAP NAS, create a project
(Container Manager) or a container from the same image; the volume that holds `/data` must be writable by uid 10001.

### The program (macOS, Linux, Windows)

#### On a Mac: one command

```sh
curl -fsSL https://raw.githubusercontent.com/elrumo/lightcraft/main/apps/lightcraft-server/install.sh | sh
```

It downloads the newest server release for your Mac (Apple silicon or Intel), checks its checksum and installs it:
the program and the web app go to `~/Library/Application Support/LightCraft Server`, and a launchd agent starts the
server at every login and restarts it if it stops. It prints the admin page's address
(`http://localhost:8080/admin`, reachable from your other devices at `http://<this Mac's name>.local:8080`) and the
one-time setup code. Data is kept in `~/Library/Application Support/LightCraft Server/data`, the log in
`~/Library/Logs/LightCraft Server/server.log`. macOS may ask once to let it accept network connections: allow it.

Already downloaded the zip? Unzip it, clear the download flag once and run the same installer from inside it
(`--data DIR`, `--listen HOST:PORT` and `--web DIR` change where it keeps data, what it listens on and which web app
it serves; the default is `0.0.0.0:8080`, for a home network):

```sh
xattr -dr com.apple.quarantine lightcraft-server-*-macos-*
./lightcraft-server-*-macos-*/lightcraft-server install
```

Remove it with `~/Library/Application\ Support/LightCraft\ Server/program/lightcraft-server uninstall`: it stops the
server and removes the agent and the program, and keeps your data. The Mac must be awake and logged in for the server
to answer (System Settings ▸ Energy: prevent automatic sleeping on a Mac used as a server).

#### By hand

Unzip a download from the Releases page and run it from a terminal (Windows: PowerShell). On a Mac, if Gatekeeper
refuses the program ("cannot be verified"), clear the download flag once: `xattr -dr com.apple.quarantine lightcraft-server-*`.

```sh
cd lightcraft-server-0.14.0-macos-arm64
./lightcraft-server --version
./lightcraft-server serve --data ~/lightcraft-data       # http://127.0.0.1:8080 : this computer only
```

```powershell
cd lightcraft-server-0.14.0-windows-x86_64
.\lightcraft-server.exe serve --data C:\lightcraft-data
```

It serves the `web` folder beside it at `/` (or the one `--web DIR` / `LIGHTCRAFT_WEB` names), keeps everything in the
data folder, and prints the setup code in the terminal. To let your other devices reach it, listen on the network
(`--listen 0.0.0.0:8080`) **only on a home network or tailnet**; for the internet put Caddy or `tailscale serve` in
front of it (see the checklist at the end). A library folder's path is then the folder's real path (`/Volumes/Photos/ann`, `D:\Photos\ann`).

To keep it running, start it as a service:

- **Linux (systemd):** the unit [below](#run-it-as-a-service-linux-systemd).
- **macOS:** `./lightcraft-server install` (below) does it: no service file to write.
- **Windows (Task Scheduler):** runs at start-up, without a window, and restarts after a failure. In an
  administrator PowerShell:

  ```powershell
  $a = New-ScheduledTaskAction -Execute "C:\lightcraft-server\lightcraft-server.exe" -Argument "serve --data C:\lightcraft-data" -WorkingDirectory "C:\lightcraft-server"
  $t = New-ScheduledTaskTrigger -AtStartup
  $s = New-ScheduledTaskSettingsSet -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit 0
  Register-ScheduledTask -TaskName "LightCraft Server" -Action $a -Trigger $t -Settings $s -User "SYSTEM" -RunLevel Highest
  Start-ScheduledTask -TaskName "LightCraft Server"
  ```

  The setup code is then in the terminal only when you run the server by hand once first, or from the command line
  instead of the admin page: `lightcraft-server.exe user add ann --admin --data C:\lightcraft-data`.

Updating the program: stop it, replace `lightcraft-server` and the `web` folder with the new release's, start it.
The data folder stays.

### From source

```sh
git clone https://github.com/elrumo/lightcraft
cd lightcraft
```

Docker: `docker build -f apps/lightcraft-server/Dockerfile -t lightcraft-server .` (`--target lite` leaves out the web
app; the first build takes 10–20 minutes), or `cd apps/lightcraft-server && LIGHTCRAFT_DOMAIN=photos.example.com
docker compose up -d --build` for the Caddy setup from source. Without Docker:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked --version "$(sed -n '/^name = "wasm-bindgen"$/{n;s/version = "\(.*\)"/\1/p;}' Cargo.lock)"
cargo xtask web                                   # the web app → target/web-site
cargo build --release -p lightcraft-server        # → target/release/lightcraft-server
sudo install -m 755 target/release/lightcraft-server /usr/local/bin/
sudo mkdir -p /opt/lightcraft && sudo cp -r target/web-site /opt/lightcraft/web
```

### Run it as a service (Linux, systemd)

On this machine only, behind Caddy or `tailscale serve`. With a downloaded zip, put the program and its `web` folder in
`/opt/lightcraft` and point `ExecStart` there.

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
- **Every other device signs in from a new, empty library**, and fills from the server.
- A library that has photos of its own, signing in to a server that has some, **asks what to do**: add its photos to
  the server's library (photos both have are kept once), or use the server's library instead (a copy of the old one is
  kept in its folder), or don't sync. Nothing syncs until you choose;
  [sync.md](sync.md#when-this-library-and-the-servers-both-have-photos) has the details.
  Or tick *Combine with the photos already on the server* in Settings ▸ Sync before signing in to merge at once
  (photos are matched by what they are, nothing is removed on either side).

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
- **File ▸ Pause Syncing / Sync Now**; Settings ▸ Sync ▸ **Sign Out** sends what is left, signs the device out on the
  server and removes the library from this device: it stays on the server, and the next user to sign in starts from an
  empty library of their own ([sync.md](sync.md#signing-out)).

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

1. The app's library starts with demo photos: it signs in as a new library and fills from the server. (A phone that
   already has photos of its own asks whether to add them to the server's library or to use the server's.) The cloud
   button on the grid shows what syncing is doing, here and on the server: uploads, downloads, scans, previews.
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

- **Updating:** `docker compose -f docker-compose.hub.yml pull && docker compose -f docker-compose.hub.yml up -d`
  (plain Docker: `docker pull elrumo/lightcraft-server`, remove the container and run it again; the program: replace
  it with the new release's; from source: `git pull` and rebuild). The data stays in its volume; the library is upgraded on open. **Update the apps too:** a
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
- **Storage limits** (admin page ▸ Users ▸ the user ▸ Storage limit, or `lightcraft-server user quota ann 200G`): the
  most the server keeps for one person; uploads over it wait, nothing is deleted. Library folders don't count.
- **Logs:** `docker compose logs -f lightcraft`; `LIGHTCRAFT_LOG=debug` for more.
- **Scans of library folders:** every 15 minutes (`LIGHTCRAFT_SCAN_INTERVAL`, minutes; `0` = only at start and from
  the admin page's **Scan for changes**; its arrow also offers **Rescan everything**, which reads every file again). A
  disk that isn't mounted is skipped, never read as everything deleted.

## 6. When something goes wrong

| What you see | What to do |
|---|---|
| `can't reach the server: …` | Check the address and its scheme (`https://` behind Caddy or `tailscale serve`, `http://` on port 8080), that the container is `healthy`, and the firewall. |
| `wrong user name or password` | Check them on the admin page (Reset password). |
| `too many wrong passwords: try again in N s` | Wait: after five wrong passwords each try waits longer (up to 15 minutes), per address and per user name. |
| The app asks **Sync This Library** after you sign in | This library and the server's both have photos. **Add this library to the server's** uploads this one into it (photos both have are kept once), **Use the server's library instead** replaces this one (a copy is kept in its folder), **Sign Out** leaves things as they are. Details: [sync.md](sync.md#when-this-library-and-the-servers-both-have-photos). |
| `this library is a copy of another library on that server` | The server was set up again (or the user's library replaced): remove the library from this device (Settings ▸ Sync ▸ **Remove Library from This Device…**; what hasn't reached the server is lost) and sign in again, or sign in from a new library; in a browser, `?reset` on the address also clears everything. |
| `the server signed this device out` | The device was signed out on the admin page, or its user removed: sign in again as the same user, or (Settings ▸ Sync ▸ **Remove Library from This Device…**) remove the library to sign in as someone else. A sign-out that was waiting for the server never removes anything on its own. |
| The admin page asks for a setup code | No admin exists yet: the code is in the server's log. |
| A library folder `isn't there (not mounted?)` / `is empty but had photos` | The path is the one inside the container (`/photos/…`); check the volume mount, and that uid 10001 can read it. |
| Photos without thumbnails for a while | Their previews are still being built (admin page ▸ Users ▸ the user ▸ Library folders shows how many are built and left, and which photos are being built now). |
| Uploads of big raws fail behind your own proxy | Allow large request bodies (nginx: `client_max_body_size 0;`). |
| `over the storage limit` | The user's [storage limit](sync.md#storage-limits) is reached: raise it on the admin page, or free room (delete photos for good, then clean up storage). |

## Security checklist

- Only Caddy (443), `tailscale serve`, or your home network reaches the server — never port 8080 from the internet.
- Strong passwords (8 characters at least are required). Sign out devices you no longer use (admin page ▸ Users ▸ the user ▸ Devices); an app's own **Sign Out** also removes its token from the server.
- Library folders mounted read-only (`:ro`), unless you want the server to write XMP sidecars or file uploads there
  (then mount that one `:rw`, make it writable for uid 10001, and switch it on in the admin page).
- Album links are secret addresses: send them to people you'd send the photos to, give them an end date (and a
  password, for one that may travel further), and revoke the ones you no longer need (Settings ▸ Sync ▸ Shared Links,
  or the admin page). A link that lets people add photos fills its owner's storage: give that user a storage limit.
- The web app on another site needs that site named with `--cors-origin` (`LIGHTCRAFT_CORS`); leave it off otherwise.
- Keep the server and the apps updated together.

What the server does on its side — throttling password guessing, hashed passwords and tokens, size limits, headers —
is listed in [sync.md → Security](sync.md#security).
