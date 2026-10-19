# Hidden photos and sensitive content

## The Hidden album

Like Apple Photos' Hidden album: a hidden photo leaves All Photos, every album and smart album, search, People,
the map, the date and keyword lists and shared album links. It is still in the library (synced, backed up,
exported when asked) and nothing about the file changes.

- **Hide / Unhide:** Photo ▸ Hide Photo (⌘L), the photo's right-click or "…" menu, or the select bar's More menu.
  `photo.hide {ids?}` / `photo.unhide {ids?}`, one undo step.
- **See them:** the **Hidden** row in My Photos (a padlock while locked) shows only the hidden photos;
  **Show Hidden** (the filter bar, the phone's grid "…" and filter page) mixes them
  back into whatever is shown, with an eye-slash badge.
- **The lock:** both need Hidden unlocked. The egui apps (desktop, web, egui iPhone) ask for the library's Hidden
  password, set the first time (Settings ▸ General ▸ Hidden to change it). A native (non-Rust) frontend can unlock with
  its platform's own check (Face ID, Touch ID or the device passcode) instead.
- **It locks again** when nothing shows hidden photos any more (Hidden left with Show Hidden off, or Show Hidden turned
  off), when the app goes to the background on a phone or tablet, when another library opens, and at every launch
  (a relaunch never opens on Hidden).

The lock is the engine's (`crates/engine/src/hidden.rs`), so the control channel and MCP get it too:

| Command | What it does |
|---|---|
| `hidden.status` | `{locked, hasPassword, showHidden, count}` (`count` only while unlocked; also in `library.state`) |
| `hidden.unlock {password}` | unlock; a wrong password is error kind `locked`, five in a row wait 30 s |
| `hidden.setPassword {password, current?}` | at least 4 characters; changing it needs the current one, or Hidden unlocked |
| `hidden.lock` | lock now |
| `library.source {kind: "hidden"}`, `library.showHidden {on}` | refused with kind `locked` while locked |

A native frontend unlocks after the platform's own check (Face ID) with `host.hidden.unlock`, which only the engine host
answers ([engine-host.md](engine-host.md)). Password commands are queries: no password is kept in the command journal.

**What it is not.** It keeps photos out of sight, like Apple's. It is not encryption: the catalog and the files are as
readable as before by anyone with the library folder, and the password's argon2id hash is in its `prefs.json`.

**Forgot the password?** A native frontend that unlocks with Face ID can reset it (`host.hidden.resetPassword`), or
reset it on the computer that has the library:

```sh
lightcraft-cli hidden reset-password --library ~/Pictures/LightCraft
```

The next unlock then sets a new password; a synced library forgets the old one on every device.

**Syncing.** Hiding is a catalog change (`Op::SetHidden`, catalog format 11) that syncs like a flag. The password's
hash syncs as its own document (`privacy`), so one password opens Hidden on every egui device of the library. A server
older than format 11 would drop the hidden state and show the photos on the other devices, so while a device is signed
in to one, Hide is refused and a waiting hidden change isn't sent ("Update LightCraft Server first"). The server reports
its format with every pull (`proto::Ops.format`).

## Possibly sensitive photos

An optional, downloaded model flags photos that might be sensitive (nudity and the like) so they can be reviewed and
hidden. It only suggests: nothing is hidden or changed until the person chooses.

- **Turn it on:** Settings ▸ AI Models ▸ NSFW Image Detection (about 87 MB, Falcons.ai's
  `nsfw_image_detection`, Apache License 2.0, shown before anything is fetched), or `vision.sensitive.download
  {acknowledged: true}` then `vision.setSensitive {on: true}`. Photos are checked in the background pass that also
  describes them (`vision.index`), from a neutral 512 px rendering.
- **Review:** the **Possibly Sensitive** row in My Photos (shown once something is flagged) lists them, most likely
  first, with **Hide All** (`library.hideSensitive`); hide or keep them one by one with the photo menus.
- **Blur:** **Blur Sensitive Photos** (on by default; the bar above the list, Settings ▸ General) obscures the flagged photos in the grid, the filmstrip and the map. Opening a photo shows it.
  `library.setBlurSensitive {on}`; the choice syncs with the library.
- **On the server:** an admin turns it on for everyone (admin page ▸ Processing ▸ Sensitive content, or
  `lightcraft-server model download --accept-licences --sensitive`). The server checks each user's smart previews and
  devices without the model (iOS, the web) list its flags (`GET /api/sensitive`).

**Large model.** Settings ▸ AI Models ▸ NSFW Image Detection (Large) (about 89 MB, AdamCodd's `vit-base-nsfw-detector`,
Apache License 2.0; `models.download {id: "nsfw-large"}`; on a server `lightcraft-server model download
--accept-licences --sensitive-large`) is a larger-input model (384 px instead of 224) with fewer false alarms. Once it is
installed it checks photos instead of the compact one (a **Run On** choice of CPU or GPU keeps the compact one), on the
CPU, about four times slower per photo, unless its GPU weights are installed too (Settings ▸ AI Models ▸ NSFW Image
Detection (Large, GPU), 344 MB, Mac and iPhone: candle on Metal in half precision, about five times faster than its int8
file; `nsfw-large-gpu`), which `Auto` runs instead; its scores are kept apart (`sensitive-adamcodd-nsfw-384.bin`), so every photo is
checked again. Removing it goes back to the compact model. Not yet: an admin-page button (the server uses the command
line).

**Choosing the model and the processor.** Two choices, saved with the library: the model (`vision.setSensitiveModel
{model: auto|compact|large}`; Automatic is the large one once it is downloaded) and, for that model, where it runs
(`vision.setSensitiveBackend {backend: auto|cpu|gpu}`); Settings ▸ AI Models ▸ Sensitive Content has a **Model** and a
**Run On** picker. A device doing the server's work (`vision.setHelper`) uses the same two choices, and the server only
accepts results of the model it keeps (`GET /api/sensitive` says which): when the two differ the server refuses the
device's claim ("this server keeps results of …"), so pick the server's model on the device to help it.

The scores are derived data, like the search vectors: `<library>/search/sensitive-falconsai-nsfw.bin` (server:
`<data>/users/<name>/search/`), keyed by content, never in the catalog, never sent anywhere by a device. A photo scored
0.7 or more is flagged.

**Limits (honest).** The cut-off is not calibrated on a real library, and the model makes mistakes both ways (art,
swimwear and skin-coloured scenes can be flagged; some explicit photos won't be). There is no "not sensitive" mark yet:
a wrongly flagged photo stays in the list and blurred until Blur is turned off. True-positive accuracy has not been
measured here. On a device that reads the server's flags, a flag that arrives shows in the grid at the next change to
the library.
