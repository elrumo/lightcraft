//! Sharing one library between devices through a server (no I/O here: the engine and the server
//! move the bytes).
//!
//! - **One order.** The server keeps the single log of every device's ops ([`ServerCore`]). A
//!   device pushes its changes on top of the newest op it has seen; behind, it pulls first.
//! - **Keyed outbox.** A device's unpushed changes are kept per value they set ([`key_of`]: a
//!   photo's rating, an album's photos…), each with the value it replaced ([`Outbox`]). An op from
//!   another device that sets a value with a pending change is merged with it ([`merge3`]):
//!   Exposure edited here and Contrast there both survive, so do photos added to one album on two
//!   devices. Everything else is applied as it arrives. A removal beats a pending change.
//! - **Device-local state stays local**: browsing Local folders, renames and relinks of files on
//!   disk, and edit History never leave the device ([`key_of`] is `None`). Pushed photos point at
//!   their content (`web/<hash>/<name>`, see [`original_path`]), not at a path on this disk.
//! - **Ids** don't collide: each device allocates in its own id space ([`Catalog::set_id_space`]).
//! - **Repair**: when an op can't be applied the same way here and on the server, the device
//!   reloads the server's state ([`ServerCore::snapshot`]), keeps its device-local state
//!   ([`carry_local`]) and replays its pending changes on top ([`Outbox::rebase`]).

use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::journal::Journal;
use crate::store::Store;
use crate::{Catalog, CatalogError, Op, Photo, PhotoId, Result, Source};

/// Prefix of the catalog paths of photos stored by content: `web/<content hash>/<file name>`
/// (the web build's browser storage, and synced photos whose file isn't on this disk).
pub const PATH_PREFIX: &str = "web/";

/// Array fields merged member by member (album and stack photos, keywords, versions); other
/// arrays (curves, masks…) are one value.
const SET_FIELDS: [&str; 3] = ["photos", "keywords", "versions"];

/// Nesting depth [`merge3`] and [`Outbox::record`] follow (deeper is taken whole).
const MAX_DEPTH: usize = 32;

/// Catalog path for a photo stored by content.
pub fn original_path(hash: &str, name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or("photo");
    format!("{PATH_PREFIX}{hash}/{name}")
}

/// The content hash in a path made by [`original_path`].
pub fn hash_of_path(path: &str) -> Option<&str> {
    let (hash, _) = path.strip_prefix(PATH_PREFIX)?.split_once('/')?;
    (!hash.is_empty()).then_some(hash)
}

/// The value an op sets, as `p<id>.<field>` / `a<id>.<field>` / `s<id>.<field>` /
/// `label.<colour>`: two ops with the same key replace the same value. `None` for ops that are
/// never synced: device-local ones and batches (synced op by op).
pub fn key_of(op: &Op) -> Option<String> {
    let p = |id: &PhotoId, f: &str| Some(format!("p{}.{f}", id.0));
    let a = |id: &crate::AlbumId, f: &str| Some(format!("a{}.{f}", id.0));
    match op {
        Op::AddPhoto { photo } => p(&photo.id, "add"),
        Op::RemovePhoto { id } => p(id, "remove"),
        Op::SetRating { id, .. } => p(id, "rating"),
        Op::SetFlag { id, .. } => p(id, "flag"),
        Op::SetLabel { id, .. } => p(id, "label"),
        Op::SetDevelop { id, .. } => p(id, "develop"),
        Op::SetMeta { id, .. } => p(id, "meta"),
        Op::SetDeleted { id, .. } => p(id, "deleted"),
        Op::SetVersions { id, .. } => p(id, "versions"),
        Op::SetCaptured { id, .. } => p(id, "captured"),
        Op::SetAnalysis { id, .. } => p(id, "analysis"),
        Op::SetContent { id, .. } => p(id, "content"),
        Op::SetServerPath { id, .. } => p(id, "serverPath"),
        Op::AddAlbum { album } => a(&album.id, "add"),
        Op::RemoveAlbum { id } => a(id, "remove"),
        Op::RenameAlbum { id, .. } => a(id, "name"),
        Op::MoveAlbum { id, .. } => a(id, "parent"),
        Op::SetAlbumPhotos { id, .. } => a(id, "photos"),
        Op::SetAlbumCover { id, .. } => a(id, "cover"),
        Op::SetAlbumRules { id, .. } => a(id, "rules"),
        Op::AddStack { stack } => Some(format!("s{}.add", stack.id.0)),
        Op::RemoveStack { id } => Some(format!("s{}.remove", id.0)),
        Op::SetStack { id, .. } => Some(format!("s{}.set", id.0)),
        Op::SetLabelName { label, .. } => Some(format!("label.{label:?}")),
        Op::SetLocal { .. }
        | Op::SetHistory { .. }
        | Op::PushHistory { .. }
        | Op::SetFile { .. }
        | Op::Relink { .. }
        | Op::SetBrowsed { .. }
        | Op::Batch { .. } => None,
    }
}

/// The key prefix of every value of the record a removal deletes (`p7.`).
fn removed_prefix(op: &Op) -> Option<String> {
    match op {
        Op::RemovePhoto { id } => Some(format!("p{}.", id.0)),
        Op::RemoveAlbum { id } => Some(format!("a{}.", id.0)),
        Op::RemoveStack { id } => Some(format!("s{}.", id.0)),
        _ => None,
    }
}

/// The photo a photo op changes.
fn photo_of(op: &Op) -> Option<PhotoId> {
    match op {
        Op::SetRating { id, .. }
        | Op::SetFlag { id, .. }
        | Op::SetLabel { id, .. }
        | Op::SetDevelop { id, .. }
        | Op::SetMeta { id, .. }
        | Op::SetDeleted { id, .. }
        | Op::SetVersions { id, .. }
        | Op::SetCaptured { id, .. }
        | Op::SetAnalysis { id, .. }
        | Op::SetContent { id, .. }
        | Op::SetServerPath { id, .. } => Some(*id),
        _ => None,
    }
}

/// A photo as other devices get it: pointing at its content instead of a path on this disk, and
/// without this device's History and Local bookkeeping.
pub fn portable(p: &Photo) -> Photo {
    let mut p = p.clone();
    if let (Source::File { path }, Some(hash)) = (&p.source, &p.content_hash)
        && !path.starts_with(PATH_PREFIX)
    {
        p.source = Source::File { path: original_path(hash, &p.file_name) };
    }
    p.history.clear();
    p.local = false;
    p.local_baseline = None;
    p
}

/// Three-way merge of JSON values: what `ours` changed since `base`, applied over `theirs`.
/// Objects merge field by field and the set-like arrays (`photos`, `keywords`, `versions`)
/// member by member; any other value changed on both sides is ours (pushed after theirs, it
/// would win on the server anyway).
pub fn merge3(base: &Value, ours: &Value, theirs: &Value) -> Value {
    merge3_at(base, ours, theirs, "", 0)
}

fn merge3_at(base: &Value, ours: &Value, theirs: &Value, field: &str, depth: usize) -> Value {
    if ours == base {
        return theirs.clone();
    }
    if theirs == base || theirs == ours || depth > MAX_DEPTH {
        return ours.clone();
    }
    match (base, ours, theirs) {
        (Value::Object(b), Value::Object(o), Value::Object(t)) => {
            let mut out = t.clone();
            for k in o.keys().chain(b.keys()) {
                let (bv, ov) = (b.get(k).unwrap_or(&Value::Null), o.get(k).unwrap_or(&Value::Null));
                if ov == bv {
                    continue;
                }
                let m = merge3_at(bv, ov, t.get(k).unwrap_or(&Value::Null), k, depth + 1);
                if m.is_null() && !o.contains_key(k) {
                    out.remove(k);
                } else {
                    out.insert(k.clone(), m);
                }
            }
            Value::Object(out)
        }
        (Value::Array(b), Value::Array(o), Value::Array(t)) if SET_FIELDS.contains(&field) => {
            let set = |v: &[Value]| v.iter().map(Value::to_string).collect::<HashSet<String>>();
            let (bs, os) = (set(b), set(o));
            // theirs without what we removed, then what we added
            let kept = t.iter().filter(|v| {
                let s = v.to_string();
                !bs.contains(&s) || os.contains(&s)
            });
            let added = o.iter().filter(|v| !bs.contains(&v.to_string()));
            let mut seen = HashSet::new();
            let out: Vec<Value> = kept.chain(added).filter(|v| seen.insert(v.to_string())).cloned().collect();
            Value::Array(out)
        }
        _ => ours.clone(),
    }
}

/// [`merge3`] of three ops setting the same value (`ours` if the merge doesn't read back as an op).
fn merge_ops(base: &Op, ours: &Op, theirs: &Op) -> Op {
    let v = |op: &Op| serde_json::to_value(op).unwrap_or(Value::Null);
    serde_json::from_value(merge3(&v(base), &v(ours), &v(theirs))).unwrap_or_else(|_| ours.clone())
}

/// One pending change: the latest op setting a value, and the value it replaced (the server's
/// value as far as this device knows).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Entry {
    key: String,
    base: Op,
    latest: Op,
    /// Bumped on every change, so a push acknowledged after a newer change keeps the entry.
    rev: u64,
    /// What the last push sent for this value: seen coming back from the server (its
    /// acknowledgement was lost), it is this device's own change, not another device's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sent: Option<Op>,
}

/// What a push sent ([`Outbox::take_push`]): entries by key and revision, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pushed(Vec<(String, u64)>);

impl Pushed {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A device's changes the server hasn't acknowledged, one per value, in the order they must be
/// applied (a changed entry moves to the end: after whatever it depends on, like an album's new
/// photos after the photos).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Outbox {
    entries: Vec<Entry>,
    rev: u64,
}

impl Outbox {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Note an op this device applied (`inverse` is what [`Catalog::apply`] returned, `after` the
    /// catalog once it was applied). Batches are recorded op by op; device-local ops and changes
    /// to Local records are left out; a Local photo added to the library is pushed whole.
    pub fn record(&mut self, op: &Op, inverse: &Op, after: &Catalog) {
        self.record_at(op, inverse, after, 0);
    }

    fn record_at(&mut self, op: &Op, inverse: &Op, after: &Catalog, depth: usize) {
        if let (Op::Batch { ops }, Op::Batch { ops: invs }) = (op, inverse) {
            if depth < MAX_DEPTH {
                // the inverse batch undoes in reverse order
                for (o, i) in ops.iter().zip(invs.iter().rev()) {
                    self.record_at(o, i, after, depth + 1);
                }
            }
            return;
        }
        let (latest, base) = match op {
            Op::SetLocal { id, local: false } => match after.photo(*id) {
                Some(p) => (Op::AddPhoto { photo: Box::new(portable(p)) }, Op::RemovePhoto { id: *id }),
                None => return,
            },
            // back out of the library: gone for other devices
            Op::SetLocal { id, local: true } => (Op::RemovePhoto { id: *id }, Op::RemovePhoto { id: *id }),
            Op::AddPhoto { photo } if !photo.local => (Op::AddPhoto { photo: Box::new(portable(photo)) }, Op::RemovePhoto { id: photo.id }),
            Op::RemovePhoto { .. } if matches!(inverse, Op::AddPhoto { photo } if !photo.local) => (op.clone(), inverse.clone()),
            Op::AddPhoto { .. } | Op::RemovePhoto { .. } => return,
            _ => {
                if let Some(id) = photo_of(op)
                    && after.photo(id).is_none_or(|p| p.local)
                {
                    return;
                }
                (op.clone(), inverse.clone())
            }
        };
        self.upsert(latest, base);
    }

    /// Queue `latest` (which replaced `base`): a new entry, or the pending one for the same value
    /// updated and moved to the end.
    fn upsert(&mut self, latest: Op, base: Op) {
        let Some(key) = key_of(&latest) else { return };
        self.rev += 1;
        let rev = self.rev;
        match self.entries.iter().position(|e| e.key == key) {
            Some(i) => {
                let mut e = self.entries.remove(i);
                e.latest = latest;
                e.rev = rev;
                self.entries.push(e);
            }
            None => self.entries.push(Entry { key, base, latest, rev, sent: None }),
        }
    }

    /// Queue a whole library, for a server that has none yet (the first device uploading its
    /// library): photos, then folders before what they hold, albums, stacks and label names.
    /// Local records stay on this device.
    pub fn seed(&mut self, c: &Catalog) {
        let shared = |id: &PhotoId| c.photo(*id).is_some_and(|p| !p.local);
        for p in c.photos().filter(|p| !p.local) {
            self.upsert(Op::AddPhoto { photo: Box::new(portable(p)) }, Op::RemovePhoto { id: p.id });
        }
        let depth = |a: &crate::Album| {
            let mut d = 0;
            let mut cur = a.parent;
            while let Some(id) = cur.filter(|_| d < MAX_DEPTH) {
                d += 1;
                cur = c.album(id).and_then(|a| a.parent);
            }
            d
        };
        let mut albums: Vec<&crate::Album> = c.albums().collect();
        albums.sort_by_key(|a| depth(a));
        for a in albums {
            let mut a = a.clone();
            a.photos.retain(shared);
            let id = a.id;
            self.upsert(Op::AddAlbum { album: a }, Op::RemoveAlbum { id });
        }
        for st in c.stacks.values().filter(|st| st.photos.iter().all(shared)) {
            self.upsert(Op::AddStack { stack: st.clone() }, Op::RemoveStack { id: st.id });
        }
        for (label, name) in &c.label_names {
            self.upsert(Op::SetLabelName { label: *label, name: Some(name.clone()) }, Op::SetLabelName { label: *label, name: None });
        }
    }

    /// An op from the server (another device's, or one of ours whose acknowledgement was lost):
    /// what to apply here, if anything. An op setting a value with a pending change here is
    /// merged with it (the merge is pushed later); a removal drops pending changes to what it
    /// removes; our own op coming back settles its entry.
    pub fn merge_remote(&mut self, remote: &Op) -> Option<Op> {
        let key = key_of(remote)?;
        if let Some(prefix) = removed_prefix(remote) {
            self.entries.retain(|e| e.key == key || !e.key.starts_with(&prefix));
        }
        let Some(i) = self.entries.iter().position(|e| e.key == key) else { return Some(remote.clone()) };
        if self.entries[i].latest == *remote {
            self.entries.remove(i);
            return None;
        }
        if self.entries[i].sent.as_ref() == Some(remote) {
            // our earlier push (since changed again here): the server now has it
            self.entries[i].base = remote.clone();
            return None;
        }
        self.rev += 1;
        let rev = self.rev;
        let e = &mut self.entries[i];
        let merged = merge_ops(&e.base, &e.latest, remote);
        e.base = remote.clone();
        e.rev = rev;
        if merged == e.latest {
            return None;
        }
        e.latest = merged.clone();
        Some(merged)
    }

    /// The ops to push, in order, and what they were (for [`Outbox::acked`]).
    pub fn take_push(&mut self, limit: usize) -> (Vec<Op>, Pushed) {
        let (mut ops, mut pushed) = (Vec::new(), Vec::new());
        for e in self.entries.iter_mut().take(limit) {
            e.sent = Some(e.latest.clone());
            ops.push(e.latest.clone());
            pushed.push((e.key.clone(), e.rev));
        }
        (ops, Pushed(pushed))
    }

    /// The server applied a push: its entries are done, unless they changed since.
    pub fn acked(&mut self, pushed: &Pushed) {
        let done: HashSet<&(String, u64)> = pushed.0.iter().collect();
        self.entries.retain(|e| !done.contains(&(e.key.clone(), e.rev)));
    }

    /// The server refused op `index` of a push (it can't be applied to the server's state): drop
    /// it, so the rest can go.
    pub fn rejected(&mut self, pushed: &Pushed, index: usize) {
        if let Some((key, rev)) = pushed.0.get(index) {
            self.entries.retain(|e| !(e.key == *key && e.rev == *rev));
        }
    }

    /// Replay the pending changes on `catalog` (the server's state, after [`carry_local`]), each
    /// merged with the server's value like [`Outbox::merge_remote`] does; changes that no longer
    /// apply are dropped. Returns how many were dropped.
    pub fn rebase(&mut self, catalog: &mut Catalog) -> usize {
        let before = self.entries.len();
        self.entries.retain_mut(|e| {
            // applying ours hands back the server's value (its inverse)
            let Ok(theirs) = catalog.apply(e.latest.clone()) else { return false };
            let merged = merge_ops(&e.base, &e.latest, &theirs);
            if merged != e.latest {
                let back = catalog.apply(theirs.clone()).and_then(|_| catalog.apply(merged.clone()));
                match back {
                    Ok(_) => e.latest = merged,
                    // (can't happen: the same value set three times) keep ours, as before the merge
                    Err(_) => {
                        let _ = catalog.apply(e.latest.clone());
                    }
                }
            }
            e.base = theirs;
            e.sent = None;
            true
        });
        before - self.entries.len()
    }
}

/// Bring this device's local state from `from` (the catalog it had) into `to` (the server's
/// state it is switching to): Local records, where each photo's file is on this disk and its
/// file name, its History, and which Local folders were browsed.
pub fn carry_local(from: &Catalog, to: &mut Catalog) {
    for (id, old) in &from.photos {
        if old.local {
            to.photos.entry(*id).or_insert_with(|| old.clone());
            continue;
        }
        if let Some(p) = to.photos.get_mut(id) {
            let p = Arc::make_mut(p);
            p.source = old.source.clone();
            p.file_name = old.file_name.clone();
            p.history = old.history.clone();
            p.local_baseline = old.local_baseline;
        }
    }
    to.browsed = from.browsed.clone();
}

/// Make a catalog from another device or the server safe to use here (it is untrusted input):
/// stacks without a photo go.
pub fn sanitize(c: &mut Catalog) {
    c.stacks.retain(|_, st| !st.photos.is_empty());
}

/// Why the server didn't take a push.
#[derive(Clone, Debug, PartialEq)]
pub enum PushError {
    /// Other devices pushed since `base`: pull up to `head` first.
    Behind { head: u64 },
    /// Op `index` can't be applied to the server's state (nothing was applied).
    Rejected { index: usize, error: String },
    /// Writing the log failed (nothing was applied).
    Storage(String),
}

/// Ops after a sequence number ([`ServerCore::since`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Pull {
    /// `(seq, op)`, oldest first.
    Ops(Vec<(u64, Op)>),
    /// The log no longer goes back that far: reload from [`ServerCore::snapshot`].
    Gone,
}

/// The server compacts its log into a snapshot once it holds this much.
pub const COMPACT_BYTES: u64 = 64 << 20;

/// A user's library on the server: the single log of every device's ops and the state it
/// leads to (to validate pushes and to start new devices from). Once the log holds
/// [`COMPACT_BYTES`] it is compacted into a snapshot: a device further behind than that reloads
/// the library ([`Pull::Gone`]) instead of pulling ops.
// ponytail: pulls read the log whole when behind; bounded by the compaction size.
pub struct ServerCore {
    journal: Journal,
    catalog: Catalog,
    compact_bytes: u64,
}

impl ServerCore {
    pub fn open(store: Box<dyn Store>) -> Result<ServerCore> {
        let (journal, catalog, _) = Journal::open(store)?;
        Ok(ServerCore { journal, catalog, compact_bytes: COMPACT_BYTES })
    }

    /// Compact the log at this size instead of [`COMPACT_BYTES`] (tests).
    pub fn set_compact_bytes(&mut self, bytes: u64) {
        self.compact_bytes = bytes.max(1);
    }

    /// Sequence number of the newest op.
    pub fn head(&self) -> u64 {
        self.journal.seq()
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// A new photo id in id space `space` (photos the server adds itself, from the user's library
    /// folders, are in a space no device is given).
    pub fn alloc_photo_id(&mut self, space: u32) -> PhotoId {
        self.catalog.set_id_space(space);
        self.catalog.alloc_photo_id()
    }

    /// Append a device's ops, made on top of op `base`: all of them or none.
    pub fn push(&mut self, base: u64, ops: &[Op]) -> std::result::Result<u64, PushError> {
        if base != self.head() {
            return Err(PushError::Behind { head: self.head() });
        }
        let mut inverses = Vec::with_capacity(ops.len());
        let mut fail = None;
        for (index, op) in ops.iter().enumerate() {
            let r = if key_of(op).is_none() { Err(CatalogError::Invalid("not an op devices share".into())) } else { self.catalog.apply(op.clone()) };
            match r {
                Ok(inv) => inverses.push(inv),
                Err(e) => {
                    fail = Some(PushError::Rejected { index, error: e.to_string() });
                    break;
                }
            }
        }
        if fail.is_none()
            && let Err(e) = self.journal.append(ops)
        {
            fail = Some(PushError::Storage(e.to_string()));
        }
        match fail {
            Some(e) => {
                for inv in inverses.into_iter().rev() {
                    let _ = self.catalog.apply(inv);
                }
                Err(e)
            }
            None => {
                if self.journal.log_bytes() >= self.compact_bytes
                    && let Err(e) = self.journal.snapshot(&self.catalog)
                {
                    // nothing lost: the log is whole, it is tried again after the next push
                    log::warn!("sync: compacting the server's log: {e}");
                }
                Ok(self.head())
            }
        }
    }

    /// The ops after `after` (at most `limit`).
    pub fn since(&mut self, after: u64, limit: usize) -> Result<Pull> {
        if after >= self.head() {
            return Ok(Pull::Ops(Vec::new()));
        }
        if after < self.journal.snapshot_seq() {
            return Ok(Pull::Gone);
        }
        Ok(Pull::Ops(self.journal.records_since(after, limit)?))
    }

    /// The current state as a snapshot (JSON, [`Catalog::from_snapshot`]) and the op it's at.
    pub fn snapshot(&self) -> (u64, String) {
        (self.head(), self.catalog.to_snapshot())
    }
}

/// The server API's JSON bodies (`/api/…`; every route but `login` wants
/// `Authorization: Bearer <token>`).
pub mod proto {
    use serde::{Deserialize, Serialize};

    use crate::{Catalog, Op};

    /// `POST /api/login`.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Login {
        pub user: String,
        pub password: String,
        /// A name for this device (shown on the server).
        pub device: String,
    }

    /// The reply to [`Login`]: this device's token, and the id space it allocates in.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Device {
        pub token: String,
        pub device: u64,
        pub space: u32,
        /// Identifies the user's library on this server (a device only resumes the same one).
        pub library: String,
    }

    /// `GET /api/snapshot`: the library after op `seq`.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Snapshot<C = Catalog> {
        pub library: String,
        pub seq: u64,
        pub catalog: C,
    }

    /// `GET /api/ops?since=N&limit=M`: the ops after `N` (`410 Gone`: reload the snapshot).
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Ops {
        pub head: u64,
        pub ops: Vec<(u64, Op)>,
        /// Version of the presets document ([`Presets`]).
        #[serde(default)]
        pub presets: u64,
    }

    /// `POST /api/ops`: ops made on top of op `base`. `200` [`Head`]; `409` [`Head`] (behind:
    /// pull first); `422` [`Refused`] (nothing was applied).
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Push {
        pub base: u64,
        pub ops: Vec<Op>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Head {
        pub head: u64,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Refused {
        pub index: usize,
        pub error: String,
    }

    /// `GET` / `PUT /api/presets`: the user presets (a JSON array). `PUT` carries the version it
    /// changed; `412` when the server's is newer (get it, merge, put again).
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Presets {
        pub version: u64,
        pub presets: serde_json::Value,
    }

    /// `/api/blobs/<kind>/<hash>` (`HEAD`, `GET`, `PUT`): a photo's original, its smart preview
    /// (≤ 2560 px) and its mini preview (≤ 512 px, for thumbnails), by content hash.
    pub const BLOB_KINDS: [&str; 3] = ["original", "smart", "mini"];

    /// `POST /api/render`: render a photo whose original the server keeps (by content hash) with the edits sent
    /// here, and answer with the encoded image: the way a device that can't hold a full-size render in memory (a phone,
    /// a 48 MP photo) exports at full size. A server without the route answers `404`; one that is busy `503`.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Render {
        /// The original's content hash.
        pub hash: String,
        /// The original's file name: its extension tells the decoder what the file is.
        pub name: String,
        /// The photo's develop settings (`DevelopSettings` JSON; a partial document merges over the defaults).
        pub settings: serde_json::Value,
        /// Export options (`ExportOptions` JSON): format, size, quality, colour space…
        pub export: serde_json::Value,
    }

    /// A count of files and the bytes they take.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Files {
        pub files: u64,
        pub bytes: u64,
    }

    /// A file system's size and what is left on it.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Disk {
        pub total: u64,
        pub free: u64,
    }

    /// `GET /api/usage`: what the user's library takes on the server. An answer may be a few
    /// seconds old (the server walks every photo file).
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(default)]
    pub struct Usage {
        pub photos: u64,
        pub albums: u64,
        /// Signed-in devices.
        pub devices: u64,
        /// The files this server keeps by content: uploaded originals, smart previews, mini
        /// previews.
        pub original: Files,
        pub smart: Files,
        pub mini: Files,
        /// Photos in the user's library folders: read where they are on the server, never copied,
        /// so not part of what LightCraft stores.
        pub folders: Files,
        /// The disk the server keeps its data on (`None`: it can't tell).
        pub disk: Option<Disk>,
    }

    impl Usage {
        /// Bytes of the files LightCraft keeps for this user (not the library folders).
        pub fn stored(&self) -> u64 {
            self.original.bytes.saturating_add(self.smart.bytes).saturating_add(self.mini.bytes)
        }
    }
}
