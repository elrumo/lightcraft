# App packs

An app pack is a set of looks that ships inside an app, not in a library. A frontend hands the engine a folder at
every start (`pack.set {id, dir?, files?, group?}`: a folder, a list of files, or both; the files of an app bundle need no
copying); the engine reads them and keeps the result in memory:

- every `.cube` file (a 3D LUT, see `crates/pipeline/src/lut.rs`) becomes a profile, named by the file's `TITLE`
  (else its file name) and filed in `group` (default: the folder's name, else the pack's). Its id is `lut:pack-<pack>-<name>` (slugs, see
  `cmd/packs.rs` `profile_id`), so presets in the same folder can name it. Only the file's header is read: the table is
  parsed when a render first needs it (`lut::register_file`), so a pack of big tables adds nothing to a start.
- every `.lcpreset` file (see `presets.rs`) adds its presets to the list as built-in ones: they can be hidden and made
  favourites, not edited, moved, renamed or deleted.

Nothing of a pack is copied into the library, saved or synced. An app update therefore changes its looks for everyone,
a photo edited with one of its profiles keeps the profile's id and renders with the look on every device that has the
pack (without it the profile is skipped, the photo's other settings still apply), and a favourite stays a favourite
across restarts (the library keeps the ids; the pack's presets pick them up when it is set).

A pack's presets are never hidden, whoever asks: the sync server's admin hides built-in presets and groups for all its
users (`server-presets`, [sync](sync.md)) and other devices sync their own hide lists (`hidden-presets`), and neither reaches
them. `presets.list` and `presets.groups {details}` report them as not hidden, and `preset.hide` / `preset.hideGroup`
refuse to hide them (taking a stale hide off is allowed). Connected to a server or not, the app's looks are in the panel.

`pack.set` again replaces the pack's earlier version (its tables and presets go first); without `dir` it takes the pack
out. Files over 64 MB and folders over 500 files are not read. The profiles show in `profiles.menu` and `profiles.list`
(`pack: true`, not `imported`: `profile.deleteImported` refuses them).

Profiles render on the GPU as well as the CPU for tables up to 65 points a side (`lut::GPU_MAX_SIZE`).
