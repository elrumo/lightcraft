//! The record sync of [`super`] through a folder instead of CloudKit: any folder a cloud provider carries to the
//! person's other devices (iCloud Drive, Dropbox, Google Drive…), so one transport serves them all and needs no
//! account of ours, no server and no Apple entitlements.
//!
//! ```text
//! <folder>/LightCraft Sync/<device>/<record name>.json   one file per record, written by that device only
//! ```
//!
//! A provider can't order writes, and two devices writing one file make it keep "conflicted copies". So a device writes
//! only its own directory, and the cloud's version of a record is the newest (`modified`, then the device name) of the
//! files of that name in all the directories. A removal is a tombstone in place of the record, so an older file of
//! another device can't bring the record back. Everything else (what changed on both sides, held records, the epoch)
//! is the engine's, as for CloudKit: [`Session::icloud_sync_folder`] is the one-pass host loop of `icloud.changes` →
//! `icloud.records` → `icloud.sent` and `icloud.fetched`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{BATCH_MAX, DATA_MAX, Incoming, Sent, now_ms};
use crate::cmd::{CommandSpec, bad, cmd};
use crate::{EngineError, Result, Session};

/// The directory of the shared folder this sync keeps its files in.
const DIR: &str = "LightCraft Sync";
/// A device directory's name is this long at most.
const DEVICE_MAX: usize = 100;
/// A record file is this large at most (a record's data and its envelope).
const FILE_MAX: u64 = DATA_MAX as u64 + (64 << 10);

/// A record file: the record, or a tombstone.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordFile {
    name: String,
    /// When the writing device changed it (ms since 1970).
    modified: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deleted: bool,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    data: String,
}

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FolderReport {
    /// Records from other devices that changed something here.
    pub changed: usize,
    /// Records written, and removals written as tombstones.
    pub sent: usize,
    pub removed: usize,
    /// Files left out: unreadable, too large, or not what their name says.
    pub skipped: usize,
    /// Files the provider has not brought to this device yet (a pass later has them).
    pub waiting: usize,
    /// This pass took only part of what there is to write: another is due at once.
    pub more: bool,
}

/// A new device name: random (the hasher's keys come from the OS), not derived from anything the person sees.
fn new_device() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::hash::RandomState::new().build_hasher();
    h.write_i64(now_ms());
    format!("{:016x}", h.finish())
}

fn device_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= DEVICE_MAX && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A regular file's size (a link or a directory is `None`: nothing outside the folder is followed).
fn file_len(p: &Path) -> Option<u64> {
    let m = std::fs::symlink_metadata(p).ok()?;
    m.is_file().then_some(m.len())
}

/// The newest version of every record the other devices wrote: the records, and the names whose newest version is a
/// removal. A record this device wrote itself is the newest of its name: nothing to take.
fn read_others(root: &Path, device: &str, report: &mut FolderReport) -> (Vec<Incoming>, Vec<String>) {
    // name → (modified, device, file)
    let mut newest: HashMap<String, (i64, String, RecordFile)> = HashMap::new();
    let devices = std::fs::read_dir(root).into_iter().flatten().flatten();
    for dev in devices {
        let dev_name = dev.file_name().to_string_lossy().into_owned();
        let dev_path = dev.path();
        if !device_ok(&dev_name) || !std::fs::symlink_metadata(&dev_path).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        for entry in std::fs::read_dir(&dev_path).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if file.ends_with(".icloud") {
                report.waiting += 1;
                continue;
            }
            let Some(name) = file.strip_suffix(".json").filter(|n| !n.starts_with('.')) else { continue };
            let path = entry.path();
            let rec = file_len(&path)
                .filter(|len| *len <= FILE_MAX)
                .and_then(|_| std::fs::read(&path).ok())
                .and_then(|b| serde_json::from_slice::<RecordFile>(&b).ok())
                .filter(|r| r.name == name);
            let Some(rec) = rec else {
                report.skipped += 1;
                continue;
            };
            let key = (rec.modified, dev_name.clone());
            if newest.get(name).is_none_or(|(m, d, _)| key > (*m, d.clone())) {
                newest.insert(name.to_string(), (rec.modified, dev_name.clone(), rec));
            }
        }
    }
    let (mut records, mut deleted) = (Vec::new(), Vec::new());
    let mut names: Vec<_> = newest.into_iter().filter(|(_, (_, d, _))| d != device).collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, (modified, _, r)) in names {
        if r.deleted {
            deleted.push(name);
        } else {
            records.push(Incoming { name, kind: r.kind, key: r.key, data: r.data, modified });
        }
    }
    (records, deleted)
}

/// Write `rec` as `<dir>/<name>.json`, whole or not at all (a provider could otherwise upload half a file).
fn write_record(dir: &Path, rec: &RecordFile) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(rec).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!(".{}.tmp", rec.name));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dir.join(format!("{}.json", rec.name)))
}

/// What one pass has to write, taken from the session ([`Session::icloud_folder_task`]) so that the file work can run
/// anywhere ([`run`]).
pub struct FolderTask {
    root: PathBuf,
    device: String,
    /// Records to write (what `icloud.sent` is told about them), and removals to write as tombstones.
    writes: Vec<(Sent, RecordFile)>,
    removals: Vec<RecordFile>,
    /// More is waiting than this pass takes.
    more: bool,
}

/// What a pass found and did ([`run`]), to hand back to the session ([`Session::icloud_folder_done`]).
pub struct FolderDone {
    sent: Vec<Sent>,
    gone: Vec<String>,
    records: Vec<Incoming>,
    deleted: Vec<String>,
    report: FolderReport,
    error: Option<String>,
}

/// Do a pass's file work: write what this device changed, then read what the others wrote. No session needed, so a
/// host runs it on a thread of its own.
pub fn run(task: FolderTask) -> FolderDone {
    let mut done =
        FolderDone { sent: Vec::new(), gone: Vec::new(), records: Vec::new(), deleted: Vec::new(), report: FolderReport::default(), error: None };
    done.report.more = task.more;
    let mine = task.root.join(&task.device);
    if let Err(e) = std::fs::create_dir_all(&mine) {
        done.error = Some(format!("{}: {e}", mine.display()));
        return done;
    }
    for (sent, rec) in &task.writes {
        match write_record(&mine, rec) {
            Ok(()) => done.sent.push(sent.clone()),
            Err(e) => {
                log::warn!("cloud folder: {}: {e}", rec.name);
                done.report.skipped += 1;
            }
        }
    }
    for rec in &task.removals {
        match write_record(&mine, rec) {
            Ok(()) => done.gone.push(rec.name.clone()),
            Err(e) => {
                log::warn!("cloud folder: {}: {e}", rec.name);
                done.report.skipped += 1;
            }
        }
    }
    done.report.sent = done.sent.len();
    done.report.removed = done.gone.len();
    (done.records, done.deleted) = read_others(&task.root, &task.device, &mut done.report);
    done
}

/// The last pass, for `icloud.status`.
#[derive(Clone, Debug)]
pub(super) struct Last {
    at: i64,
    report: FolderReport,
    error: Option<String>,
}

impl Session {
    /// This library's device name in the shared folder (made on first use, kept with the iCloud state).
    fn folder_device(&mut self) -> Option<String> {
        self.with_icloud(|_, ic| {
            if !device_ok(&ic.saved.device) {
                ic.saved.device = new_device();
                ic.saved_dirty = true;
            }
            ic.saved.device.clone()
        })
    }

    /// The folder this library syncs through (`icloud.useFolder`), if any.
    pub fn icloud_folder(&self) -> Option<PathBuf> {
        self.icloud.as_ref()?.saved.folder.as_deref().map(PathBuf::from)
    }

    /// Sync through `dir`, a folder a cloud provider carries to the other devices (`None`: stop, and turn the sync off
    /// if it was only on for the folder). Turns the iCloud sync on. A different folder is a new cloud: this device's
    /// records all go there, and what it holds of the old one is forgotten.
    pub fn icloud_use_folder(&mut self, dir: Option<&Path>) -> Result<()> {
        let Some(dir) = dir else {
            if self.icloud_folder().is_some() {
                self.icloud_enable(false)?;
            }
            return Ok(());
        };
        if !dir.is_absolute() || !dir.is_dir() {
            return Err(EngineError::Other(format!("{} is not a folder", dir.display())));
        }
        self.icloud_enable(true)?;
        let text = dir.to_string_lossy().into_owned();
        self.with_icloud(|_, ic| {
            if ic.saved.folder.as_deref() != Some(&text) {
                ic.forget();
                ic.saved.folder = Some(text);
                ic.bump();
            }
        });
        Ok(())
    }

    /// The first half of a pass through `dir` (default: the chosen folder): what this device has to write. At most
    /// [`BATCH_MAX`] records a pass, and the report says when more is waiting.
    pub fn icloud_folder_task(&mut self, dir: Option<&Path>) -> Result<FolderTask> {
        let dir =
            dir.map(Path::to_path_buf).or_else(|| self.icloud_folder()).ok_or_else(|| EngineError::Other("no cloud folder is chosen".into()))?;
        let device = self.folder_device().ok_or_else(|| EngineError::Other("iCloud sync is off for this library".into()))?;
        let changes = self.icloud_changes();
        let more = changes.save.len() + changes.delete.len() > BATCH_MAX;
        let mut writes = Vec::new();
        for r in self.icloud_records(&changes.save[..changes.save.len().min(BATCH_MAX)]) {
            let text = |k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
            let (Some(name), Some(kind), Some(key), Some(data), Some(hash)) = (text("name"), text("kind"), text("key"), text("data"), text("hash"))
            else {
                continue;
            };
            let modified = r.get("modified").and_then(Value::as_i64).unwrap_or(0);
            writes.push((
                Sent { name: name.clone(), kind: kind.clone(), key: key.clone(), hash },
                RecordFile { name, modified, deleted: false, kind, key, data },
            ));
        }
        let room = BATCH_MAX.saturating_sub(writes.len());
        let mut removals = Vec::new();
        for name in changes.delete.iter().take(room) {
            let modified = self.with_icloud(|_, ic| ic.saved.changed.get(name).copied().unwrap_or_else(|| (ic.clock)())).unwrap_or(0);
            removals.push(RecordFile { name: name.clone(), modified, deleted: true, kind: String::new(), key: String::new(), data: String::new() });
        }
        Ok(FolderTask { root: dir.join(DIR), device, writes, removals, more })
    }

    /// The second half of a pass: tell the engine what was written, then merge what the other devices wrote. (Written
    /// first: a record of theirs that is newer than this device's change then replaces it as the cloud's, not as a
    /// conflict with something the cloud never had.)
    pub fn icloud_folder_done(&mut self, done: FolderDone) -> FolderReport {
        let FolderDone { sent, gone, mut records, mut deleted, mut report, error } = done;
        self.icloud_sent(&sent, &gone);
        // (every pass reads the newest version of every record again: only what is new to this device goes to the engine,
        // whose merge counts as a change - a host that runs a pass when `rev` changes would never rest)
        self.with_icloud(|_, ic| {
            records.retain(|r| ic.saved.known.get(&r.name).is_none_or(|k| k.hash != super::hash_of(&r.data)));
            deleted.retain(|n| ic.saved.known.contains_key(n) || ic.ours.contains_key(n));
        });
        for chunk in records.chunks(BATCH_MAX) {
            report.changed += self.icloud_fetched(chunk, &[]);
        }
        for chunk in deleted.chunks(BATCH_MAX) {
            report.changed += self.icloud_fetched(&[], chunk);
        }
        let last = Last { at: now_ms(), report: report.clone(), error };
        self.with_icloud(|_, ic| ic.folder_last = Some(last));
        report
    }

    /// One whole pass through `dir`, here and now (the CLI, tests; a frontend runs [`run`] on a thread instead).
    pub fn icloud_sync_folder(&mut self, dir: Option<&Path>) -> Result<FolderReport> {
        let task = self.icloud_folder_task(dir)?;
        let done = run(task);
        if let Some(e) = done.error.clone() {
            return Err(EngineError::Other(e));
        }
        Ok(self.icloud_folder_done(done))
    }

    /// The chosen folder and how the last pass went (`{path, last: {at, report, error} | null}`), for a frontend's settings.
    pub fn icloud_folder_info(&self) -> Option<Value> {
        Session::folder_status(self.icloud.as_ref()?)
    }

    /// What `icloud.status` says of the folder sync: the folder, and how the last pass went.
    pub(super) fn folder_status(ic: &super::ICloud) -> Option<Value> {
        let folder = ic.saved.folder.as_ref()?;
        let last = ic.folder_last.as_ref().map(|l| json!({"at": l.at, "report": l.report, "error": l.error}));
        Some(json!({"path": folder, "last": last}))
    }
}

/// The folders of cloud providers on this computer that exist, `(provider, path)`, for the person to pick from.
pub fn cloud_folders(home: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();
    let mut add = |provider, p: PathBuf| {
        if p.is_dir() {
            out.push((provider, p));
        }
    };
    add("iCloud Drive", home.join("Library/Mobile Documents/com~apple~CloudDocs"));
    add("Dropbox", home.join("Dropbox"));
    let storage = home.join("Library/CloudStorage");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&storage).into_iter().flatten().flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with("Dropbox") {
            add("Dropbox", p);
        } else if name.starts_with("GoogleDrive-") {
            add("Google Drive", p.join("My Drive"));
        }
    }
    add("Google Drive", home.join("Google Drive"));
    out
}

/// [`cloud_folders`] of this computer's home folder.
pub fn detected_cloud_folders() -> Vec<(&'static str, PathBuf)> {
    home().map(|h| cloud_folders(&h)).unwrap_or_default()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).filter(|p| p.is_absolute())
}

fn cloud_folders_cmd(_: &mut Session, _: &Value) -> Result<Value> {
    let folders: Vec<Value> = home()
        .map(|h| cloud_folders(&h))
        .unwrap_or_default()
        .into_iter()
        .map(|(provider, path)| json!({"provider": provider, "path": path}))
        .collect();
    Ok(json!({"folders": folders}))
}

fn sync_folder_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "icloud.syncFolder";
    let path = match p.get("path") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) => Some(Path::new(t)).filter(|p| p.is_absolute() && p.is_dir()),
        Some(_) => None,
    };
    if p.get("path").is_some_and(|v| !v.is_null()) && path.is_none() {
        return Err(bad(C, "`path` must be an existing folder's absolute path"));
    }
    let report = s.icloud_sync_folder(path)?;
    Ok(serde_json::to_value(report).unwrap_or_default())
}

fn use_folder_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "icloud.useFolder";
    match p.get("path") {
        Some(Value::String(t)) => s.icloud_use_folder(Some(Path::new(t)))?,
        Some(Value::Null) => s.icloud_use_folder(None)?,
        _ => return Err(bad(C, "`path` must be a folder's absolute path or null")),
    }
    Ok(s.icloud_status(false))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "icloud.cloudFolders", "Cloud Folders", [], None,
            "{}: {folders: [{provider, path}]}: the folders of iCloud Drive, Dropbox and Google Drive on this computer (the ones that exist), to pick the one to sync through",
            super::always, cloud_folders_cmd),
        cmd!(query "icloud.useFolder", "Sync Through a Cloud Folder", [], None,
            "{path: string | null}: keep this library's presets, settings and edits in step with the other devices through the folder `path` (one from icloud.cloudFolders, or any folder a provider carries to them), in `path/LightCraft Sync/`; turns iCloud sync on. A frontend then runs a pass now and then (apphost does). null stops, and turns the sync off. Answers icloud.status, whose `cloudFolder` has the path and how the last pass went",
            super::always, use_folder_cmd),
        cmd!(query "icloud.syncFolder", "Sync Now Through the Cloud Folder", [], None,
            "{path?}: one whole pass of the iCloud record sync through the folder `path` (default: the one icloud.useFolder chose): writes what changed here, then takes what the other devices wrote. {changed, sent, removed, skipped, waiting, more}. Not for a CloudKit host",
            super::on_and_idle, sync_folder_cmd),
    ]
}
