# App packs

An app pack is a set of looks that ships inside an app, not in a library. A frontend hands the engine a folder at
every start (`pack.set {id, dir?, files?, group?, aliases?, absent?}`: a folder, a list of files, or both; the files of an app bundle need no
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

## Looks fetched when they are wanted

An app need not carry every table. `pack.set {…, absent: [look name]}` declares the looks of the pack whose tables are
not on the device: each is a profile like the others (`profiles.list` and `profiles.menu` list it in its group with
`absent: true`, a photo that uses it names it, `presets.list` marks a preset that sets it with `look` and `absent`), but
it has no file (`profile.lutFile` answers `null`) and a render with it draws without the table. A name that has a table
in the same call is not absent, and a group's profiles are in one order, by name, wherever their tables come from. A
retired look whose table is absent is declared as `aliases: {name: null}`.

- `pack.needed {ids?}` names the absent looks the library's photos (or `ids`) are developed with, in their settings and
  their versions: what an app fetches without being asked, at its start and after a sync.
- When a table has been fetched, the app sets the pack again with the file. The answer's `arrived` lists the profiles
  that have a table now and had none, and `changed` the ones whose table is another file than at the last call (or the
  same file written again: a table the server replaced); the photos that use them are drawn again. A render's cache
  key carries the table's stamp for a LUT profile (`lut::stamp`: the file's length and the time it was written, read
  when the pack is set; a value of its own while there is no table, `MediaCache::camera_profile_salt`), so a picture
  drawn without the table, or with the one it replaced, is never reused for the look, and a host's thumbnails follow
  (`MediaCache::take_rebuilt`).
- A final output is never written without its look: an export, a print, a contact sheet's file or a slideshow's
  video of a photo whose look is absent is refused (`Session::look_here`, error kind `LookMissing`, the host's code
  `lookMissing`), naming the look. The frontend fetches the table and tries again. A copy of the original needs no
  look and is not refused, and a pack look nothing was declared about behaves as before (drawn without the table).

`pack.set` again replaces the pack's earlier version (its tables and presets go first); without `dir` it takes the pack
out (a pack of `absent` looks alone is a pack). Files over 64 MB and folders over 500 files are not read. The profiles show in `profiles.menu` and `profiles.list`
(`pack: true`, not `imported`: `profile.deleteImported` refuses them).

Profiles render on the GPU as well as the CPU for tables up to 65 points a side (`lut::GPU_MAX_SIZE`).
