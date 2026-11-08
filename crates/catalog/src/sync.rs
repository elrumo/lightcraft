//! Sharing one library between devices through a server (no I/O here: the engine and the server
//! move the bytes).
//!
//! - **One order.** The server keeps the single log of every device's ops ([`ServerCore`]). A
//!   device pushes its changes on top of the newest op it has seen; behind, it pulls first.
//! - **Keyed outbox.** A device's unpushed changes are kept per value they set ([`key_of`]: a
//!   photo's rating, an album's photos…), each with the value it replaced ([`Outbox`]). An op from
//!   another device that sets a value with a pending change is merged with it ([`merge3`]):
//!   Exposure edited here and Contrast there both survive, so do photos added to one album on two
//!   devices. Everything else is applied as it arrives. A removal beats a pending change. A photo
//!   added is queued by its id and goes as the catalog has it when it is pushed (a library's first
//!   sync queues all of them).
//! - **Device-local state stays local**: browsing Local folders, renames and relinks of files on
//!   disk, and edit History never leave the device ([`key_of`] is `None`). Pushed photos point at
//!   their content (`web/<hash>/<name>`, see [`original_path`]), not at a path on this disk.
//! - **Ids** don't collide: each device allocates in its own id space ([`Catalog::set_id_space`]).
//! - **Repair**: when an op can't be applied the same way here and on the server, the device
//!   reloads the server's state ([`ServerCore::snapshot`]), keeps its device-local state
//!   ([`carry_local`]) and replays its pending changes on top ([`Outbox::rebase`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
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

/// The server's key for a photo's files: its content hash (without the suffix that keeps converted
/// copies apart), when it is a 128-bit hex hash.
pub fn content_key(p: &Photo) -> Option<String> {
    let h = p.content_hash.as_deref()?.split(':').next()?;
    (h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| h.to_ascii_lowercase())
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
        Op::SetHidden { id, .. } => p(id, "hidden"),
        Op::SetVersions { id, .. } => p(id, "versions"),
        Op::SetCaptured { id, .. } => p(id, "captured"),
        Op::SetAnalysis { id, .. } => p(id, "analysis"),
        Op::SetContent { id, .. } => p(id, "content"),
        Op::SetServerPath { id, .. } => p(id, "serverPath"),
        Op::SetEmbeddedLens { id, .. } => p(id, "embeddedLens"),
        Op::SetAsShotWb { id, .. } => p(id, "asShotWb"),
        Op::AddAlbum { album } => a(&album.id, "add"),
        Op::RemoveAlbum { id } => a(id, "remove"),
        Op::RenameAlbum { id, .. } => a(id, "name"),
        Op::MoveAlbum { id, .. } => a(id, "parent"),
        Op::SetAlbumPhotos { id, .. } => a(id, "photos"),
        Op::SetAlbumCover { id, .. } => a(id, "cover"),
        Op::SetAlbumRules { id, .. } => a(id, "rules"),
        Op::SetAlbumOrder { id, .. } => a(id, "order"),
        Op::AddStack { stack } => Some(format!("s{}.add", stack.id.0)),
        Op::RemoveStack { id } => Some(format!("s{}.remove", id.0)),
        Op::SetStack { id, .. } => Some(format!("s{}.set", id.0)),
        Op::SetLabelName { label, .. } => Some(format!("label.{label:?}")),
        Op::SetKeyword { path, .. } => Some(format!("keyword.{}", crate::keywords::clean(path).to_lowercase())),
        // folder records are about folders on this device's disk
        Op::SetFolderRecord { .. }
        | Op::SetLocal { .. }
        | Op::SetHistory { .. }
        | Op::PushHistory { .. }
        | Op::SetHistoryCursor { .. }
        | Op::SetFile { .. }
        | Op::Relink { .. }
        | Op::SetBrowsed { .. }
        | Op::Batch { .. } => None,
    }
}

/// What a pending change is about ([`Outbox::changes`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Subject {
    Photo(PhotoId),
    Album(crate::AlbumId),
    Stack(crate::StackId),
    /// A colour label's name (`Red`).
    LabelName(String),
    /// A keyword, by its path in the keyword list.
    Keyword(String),
    /// A key this version doesn't know (an outbox written by a newer one).
    Other(String),
}

/// The record a [`key_of`] key is about and the value of it that changes (`rating`, `develop`, `add`…).
pub fn parse_key(key: &str) -> (Subject, &str) {
    let record = |prefix: char| {
        let (id, field) = key.strip_prefix(prefix)?.split_once('.')?;
        Some((id.parse::<u64>().ok()?, field))
    };
    if let Some((id, field)) = record('p') {
        return (Subject::Photo(PhotoId(id)), field);
    }
    if let Some((id, field)) = record('a') {
        return (Subject::Album(crate::AlbumId(id)), field);
    }
    if let Some((id, field)) = record('s') {
        return (Subject::Stack(crate::StackId(id)), field);
    }
    if let Some(label) = key.strip_prefix("label.") {
        return (Subject::LabelName(label.to_string()), "name");
    }
    if let Some(path) = key.strip_prefix("keyword.") {
        return (Subject::Keyword(path.to_string()), "keyword");
    }
    (Subject::Other(key.to_string()), "")
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
        | Op::SetHidden { id, .. }
        | Op::SetVersions { id, .. }
        | Op::SetCaptured { id, .. }
        | Op::SetAnalysis { id, .. }
        | Op::SetContent { id, .. }
        | Op::SetEmbeddedLens { id, .. }
        | Op::SetAsShotWb { id, .. }
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
    p.history_cursor = None;
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

/// A photo added to the library and waiting to go, by its id: what goes is the photo as the
/// catalog has it when the push is taken ([`Outbox::take_push`]), so a whole library queued at once
/// costs a few bytes a photo instead of a copy of each (its later changes are queued too, and go
/// after it). `sync.outbox` holds it as `[id, rev]`, or `[id, rev, hidden, sent]`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Added {
    id: PhotoId,
    rev: u64,
    /// It was hidden when it was queued ([`Outbox::carries_hidden`]).
    hidden: bool,
    /// The last push sent it: coming back from the server, it is this device's photo (only this
    /// device makes photos with its ids), and done.
    sent: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Slot {
    Added(Added),
    Change(Box<Entry>),
}

/// What the outbox finds a change by: its [`key_of`] key, a photo's addition by the photo.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Added(PhotoId),
    Other(Box<str>),
}

impl Key {
    fn of(key: &str) -> Key {
        match parse_key(key) {
            (Subject::Photo(id), "add") => Key::Added(id),
            _ => Key::Other(key.into()),
        }
    }
}

impl Slot {
    fn key(&self) -> Key {
        match self {
            Slot::Added(a) => Key::Added(a.id),
            Slot::Change(e) => Key::of(&e.key),
        }
    }

    fn rev(&self) -> u64 {
        match self {
            Slot::Added(a) => a.rev,
            Slot::Change(e) => e.rev,
        }
    }
}

impl Serialize for Slot {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Slot::Added(a) if !a.hidden && !a.sent => (a.id, a.rev).serialize(s),
            Slot::Added(a) => (a.id, a.rev, a.hidden, a.sent).serialize(s),
            Slot::Change(e) => e.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Slot {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Slot, D::Error> {
        struct Form;
        impl<'de> de::Visitor<'de> for Form {
            type Value = Slot;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a pending change, or a photo to add as [id, rev]")
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Slot, A::Error> {
                let (Some(id), Some(rev)) = (seq.next_element::<PhotoId>()?, seq.next_element::<u64>()?) else {
                    return Err(de::Error::custom("a photo to add without its id and revision"));
                };
                let hidden = seq.next_element()?.unwrap_or(false);
                let sent = seq.next_element()?.unwrap_or(false);
                // (what a later version adds is skipped)
                while seq.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(Slot::Added(Added { id, rev, hidden, sent }))
            }
            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> std::result::Result<Slot, A::Error> {
                Entry::deserialize(de::value::MapAccessDeserializer::new(map)).map(|e| Slot::Change(Box::new(e)))
            }
        }
        d.deserialize_any(Form)
    }
}

/// What a push sent ([`Outbox::take_push`]): entries by key and revision, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pushed(Vec<(Key, u64)>);

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
/// photos after the photos). Found by key, so queueing a whole library takes as long as its
/// photos, not their square.
#[derive(Clone, Debug, Default)]
pub struct Outbox {
    /// The changes in order; `None` where one was taken out (squeezed out once they are most of it).
    slots: Vec<Option<Slot>>,
    /// Where each change is in `slots`.
    at: HashMap<Key, usize>,
    rev: u64,
}

impl PartialEq for Outbox {
    fn eq(&self, other: &Outbox) -> bool {
        self.rev == other.rev && self.iter().eq(other.iter())
    }
}

impl Serialize for Outbox {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        struct Entries<'a>(&'a Outbox);
        impl Serialize for Entries<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                s.collect_seq(self.0.iter())
            }
        }
        let mut st = s.serialize_struct("Outbox", 2)?;
        st.serialize_field("entries", &Entries(self))?;
        st.serialize_field("rev", &self.rev)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for Outbox {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Outbox, D::Error> {
        #[derive(Deserialize)]
        struct Form {
            entries: Vec<Slot>,
            rev: u64,
        }
        let f = Form::deserialize(d)?;
        let mut out = Outbox { rev: f.rev, ..Outbox::default() };
        for s in f.entries {
            // (a key twice, which a sound file never has: the later one)
            out.take(&s.key());
            out.put(s);
        }
        out.tidy();
        Ok(out)
    }
}

impl Outbox {
    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }
    pub fn len(&self) -> usize {
        self.at.len()
    }

    fn iter(&self) -> impl Iterator<Item = &Slot> {
        self.slots.iter().flatten()
    }

    fn slot(&self, key: &Key) -> Option<&Slot> {
        self.at.get(key).and_then(|&i| self.slots.get(i)?.as_ref())
    }

    /// Take the change of `key` out.
    fn take(&mut self, key: &Key) -> Option<Slot> {
        let i = self.at.remove(key)?;
        self.slots.get_mut(i)?.take()
    }

    /// Queue `slot` last (its key has no change queued).
    fn put(&mut self, slot: Slot) {
        self.at.insert(slot.key(), self.slots.len());
        self.slots.push(Some(slot));
    }

    /// Keep the changes `keep` says yes to.
    fn retain(&mut self, mut keep: impl FnMut(&mut Slot) -> bool) {
        for cell in &mut self.slots {
            if let Some(s) = cell
                && !keep(s)
            {
                self.at.remove(&s.key());
                *cell = None;
            }
        }
        self.tidy();
    }

    /// Squeeze the gaps out once they are most of `slots` (each gap is then moved once), and give the
    /// memory back once nothing waits.
    fn tidy(&mut self) {
        if self.at.is_empty() {
            *self = Outbox { rev: self.rev, ..Outbox::default() };
            return;
        }
        if self.slots.len() < 64 || self.at.len() * 2 > self.slots.len() {
            return;
        }
        let mut moved = Vec::with_capacity(self.slots.len());
        let mut n = 0;
        for s in &self.slots {
            moved.push(n);
            n += usize::from(s.is_some());
        }
        for i in self.at.values_mut() {
            *i = moved.get(*i).copied().unwrap_or(*i);
        }
        self.slots.retain(Option::is_some);
        self.slots.shrink_to_fit();
        self.at.shrink_to_fit();
    }

    /// A change waiting to go that hides a photo (or adds one hidden): what a server older than the Hidden
    /// album would drop, showing the photo on every other device.
    pub fn carries_hidden(&self) -> bool {
        self.iter().any(|s| match s {
            Slot::Added(a) => a.hidden,
            Slot::Change(e) => match &e.latest {
                Op::SetHidden { hidden, .. } => *hidden,
                Op::AddPhoto { photo } => photo.hidden,
                _ => false,
            },
        })
    }

    /// The pending changes in the order they will be sent, each as the record it is about and the
    /// value of it that changes ([`parse_key`]).
    pub fn changes(&self) -> impl Iterator<Item = (Subject, &str)> {
        self.iter().map(|s| match s {
            Slot::Added(a) => (Subject::Photo(a.id), "add"),
            Slot::Change(e) => parse_key(&e.key),
        })
    }

    /// Note an op this device applied (`inverse` is what [`Catalog::apply`] returned, `after` the
    /// catalog once it was applied). Batches are recorded op by op; device-local ops and changes
    /// to Local records are left out; a Local photo added to the library is pushed whole.
    pub fn record(&mut self, op: &Op, inverse: &Op, after: &Catalog) {
        self.record_at(op, inverse, after, 0);
        self.tidy();
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
        if let (Op::RemovePhoto { .. }, Op::AddPhoto { photo }) = (op, inverse) {
            self.keep_whole(photo);
        }
        let (latest, base) = match op {
            Op::SetLocal { id, local: false } => {
                if let Some(p) = after.photo(*id) {
                    self.add_photo(p);
                }
                return;
            }
            // back out of the library: gone for other devices
            Op::SetLocal { id, local: true } => (Op::RemovePhoto { id: *id }, Op::RemovePhoto { id: *id }),
            Op::AddPhoto { photo } if !photo.local => {
                self.add_photo(photo);
                return;
            }
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

    /// Queue the addition of photo `p` (again: last, and not sent).
    fn add_photo(&mut self, p: &Photo) {
        self.rev += 1;
        let key = Key::Added(p.id);
        self.take(&key);
        self.put(Slot::Added(Added { id: p.id, rev: self.rev, hidden: p.hidden, sent: false }));
    }

    /// `photo` is leaving the catalog: if its addition still waits, it keeps the photo as it was, to
    /// go before the removal.
    fn keep_whole(&mut self, photo: &Photo) {
        let Some(&i) = self.at.get(&Key::Added(photo.id)) else { return };
        let Some(slot) = self.slots.get_mut(i).and_then(Option::as_mut) else { return };
        let Slot::Added(a) = *slot else { return };
        let latest = Op::AddPhoto { photo: Box::new(portable(photo)) };
        let key = key_of(&latest).unwrap_or_default();
        let sent = a.sent.then(|| latest.clone());
        *slot = Slot::Change(Box::new(Entry { key, base: Op::RemovePhoto { id: a.id }, latest, rev: a.rev, sent }));
    }

    /// Queue `latest` (which replaced `base`): a new entry, or the pending one for the same value
    /// updated and moved to the end.
    fn upsert(&mut self, latest: Op, base: Op) {
        let Some(key) = key_of(&latest) else { return };
        self.rev += 1;
        let rev = self.rev;
        let entry = match self.take(&Key::of(&key)) {
            Some(Slot::Change(mut e)) => {
                e.latest = latest;
                e.rev = rev;
                e
            }
            Some(Slot::Added(_)) | None => Box::new(Entry { key, base, latest, rev, sent: None }),
        };
        self.put(Slot::Change(entry));
    }

    /// Queue a whole library, for a server that has none yet (the first device uploading its
    /// library): photos, then folders before what they hold, albums, stacks, label names and the
    /// keyword list.
    /// Local records stay on this device.
    pub fn seed(&mut self, c: &Catalog) {
        let shared = |id: &PhotoId| c.photo(*id).is_some_and(|p| !p.local);
        for p in c.photos().filter(|p| !p.local) {
            self.add_photo(p);
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
        for k in c.listed_keywords() {
            self.upsert(Op::SetKeyword { path: k.path.clone(), info: Some(k.info.clone()) }, Op::SetKeyword { path: k.path.clone(), info: None });
        }
        self.tidy();
    }

    /// An op from the server (another device's, or one of ours whose acknowledgement was lost):
    /// what to apply here, if anything. An op setting a value with a pending change here is
    /// merged with it (the merge is pushed later); a removal drops pending changes to what it
    /// removes; our own op coming back settles its entry.
    pub fn merge_remote(&mut self, remote: &Op) -> Option<Op> {
        let key = key_of(remote)?;
        if let Some(prefix) = removed_prefix(remote) {
            // ponytail: one look at every pending change per removal pulled; an index by record if
            // other devices' removals ever meet a big queue here
            self.retain(|s| match s {
                Slot::Added(a) => !matches!(remote, Op::RemovePhoto { id } if *id == a.id),
                Slot::Change(e) => e.key == key || !e.key.starts_with(&prefix),
            });
        }
        let k = Key::of(&key);
        let Some(slot) = self.at.get(&k).and_then(|&i| self.slots.get_mut(i)).and_then(Option::as_mut) else {
            return Some(remote.clone());
        };
        let e = match slot {
            // our photo coming back: done once it was sent; ours goes as it is otherwise (a merge of
            // two additions of a photo is ours)
            Slot::Added(a) => {
                if a.sent {
                    self.take(&k);
                    self.tidy();
                }
                return None;
            }
            Slot::Change(e) => e,
        };
        if e.latest == *remote {
            self.take(&k);
            self.tidy();
            return None;
        }
        if e.sent.as_ref() == Some(remote) {
            // our earlier push (since changed again here): the server now has it
            e.base = remote.clone();
            return None;
        }
        self.rev += 1;
        let merged = merge_ops(&e.base, &e.latest, remote);
        e.base = remote.clone();
        e.rev = self.rev;
        if merged == e.latest {
            return None;
        }
        e.latest = merged.clone();
        Some(merged)
    }

    /// The ops to push, in order, and what they were (for [`Outbox::acked`]). A photo waiting to
    /// be added goes as `catalog` has it.
    pub fn take_push(&mut self, limit: usize, catalog: &Catalog) -> (Vec<Op>, Pushed) {
        let (mut ops, mut pushed, mut gone) = (Vec::new(), Vec::new(), Vec::new());
        for s in self.slots.iter_mut().flatten() {
            if ops.len() >= limit {
                break;
            }
            match s {
                Slot::Added(a) => match catalog.photo(a.id) {
                    Some(p) => {
                        a.sent = true;
                        ops.push(Op::AddPhoto { photo: Box::new(portable(p)) });
                        pushed.push((Key::Added(a.id), a.rev));
                    }
                    // (gone without its removal being noted here: nothing to send)
                    None => gone.push(Key::Added(a.id)),
                },
                Slot::Change(e) => {
                    e.sent = Some(e.latest.clone());
                    ops.push(e.latest.clone());
                    pushed.push((Key::of(&e.key), e.rev));
                }
            }
        }
        if !gone.is_empty() {
            log::warn!("sync: {} photo(s) waiting to be added are no longer in the library", gone.len());
            for k in &gone {
                self.take(k);
            }
            self.tidy();
        }
        (ops, Pushed(pushed))
    }

    /// The change of `key` is done, unless it changed since revision `rev`.
    fn done(&mut self, key: &Key, rev: u64) {
        if self.slot(key).is_some_and(|s| s.rev() == rev) {
            self.take(key);
        }
    }

    /// The server applied a push: its entries are done, unless they changed since.
    pub fn acked(&mut self, pushed: &Pushed) {
        for (key, rev) in &pushed.0 {
            self.done(key, *rev);
        }
        self.tidy();
    }

    /// The server refused op `index` of a push (it can't be applied to the server's state): drop
    /// it, so the rest can go.
    pub fn rejected(&mut self, pushed: &Pushed, index: usize) {
        if let Some((key, rev)) = pushed.0.get(index) {
            self.done(key, *rev);
            self.tidy();
        }
    }

    /// Replay the pending changes on `catalog` (the server's state, after [`carry_local`]), each
    /// merged with the server's value like [`Outbox::merge_remote`] does; changes that no longer
    /// apply are dropped. A photo waiting to be added is the one `from` (the catalog the changes
    /// were made in) has. Returns how many were dropped.
    pub fn rebase(&mut self, from: &Catalog, catalog: &mut Catalog) -> usize {
        let before = self.len();
        self.retain(|s| match s {
            Slot::Added(a) => {
                a.sent = false;
                from.photo(a.id).is_some_and(|p| catalog.apply(Op::AddPhoto { photo: Box::new(portable(p)) }).is_ok())
            }
            Slot::Change(e) => {
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
            }
        });
        before.saturating_sub(self.len())
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
            carry_photo(old, p);
        }
    }
    to.browsed = from.browsed.clone();
}

/// A page of the server's library read again over `from`, the same library (a reload): what
/// [`carry_local`] carries is carried now, and each photo that is then equal to `from`'s becomes
/// `from`'s copy. A reload of a big library then costs the map of its photos, not a second copy of
/// every one of them beside the open library's.
pub fn share_unchanged(from: &Catalog, page: &mut Catalog) {
    for (id, p) in &mut page.photos {
        if let Some(old) = from.photos.get(id).filter(|old| !old.local) {
            carry_photo(old, p);
        }
    }
}

/// `p` with `old`'s local state (where its file is on this disk, its file name, History), and `old`
/// itself when it is then the same photo.
fn carry_photo(old: &Arc<Photo>, p: &mut Arc<Photo>) {
    if Arc::ptr_eq(old, p) {
        return;
    }
    let q = Arc::make_mut(p);
    q.source = old.source.clone();
    q.file_name = old.file_name.clone();
    q.history = old.history.clone();
    q.history_cursor = cursor_kept(old, q);
    q.local_baseline = old.local_baseline;
    // (`==` takes `0.0` for `-0.0`: such a photo keeps this device's zero, which renders the same)
    if **p == **old {
        *p = old.clone();
    }
}

/// Where `old`'s History stands, for `p`, its copy from elsewhere with its History: the step it is at while the settings
/// are still that step's; edited elsewhere, History stands at its latest step.
pub(crate) fn cursor_kept(old: &Photo, p: &Photo) -> Option<usize> {
    old.history_cursor.filter(|_| p.develop == old.develop)
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

/// What a compaction leaves of the log, as a share of [`COMPACT_BYTES`] (a quarter: 16 MiB, tens
/// of thousands of ops): devices that are not further behind than that keep pulling.
const KEEP_DIVISOR: u64 = 4;

/// A user's library on the server: the single log of every device's ops and the state it
/// leads to (to validate pushes and to start new devices from). Once the log holds
/// [`COMPACT_BYTES`] it is compacted into a snapshot that leaves the newest quarter of the log
/// in place: a device further behind than that reloads the library ([`Pull::Gone`]) instead of
/// pulling ops.
// ponytail: compaction writes the snapshot under the caller's lock (every ~48 MiB of ops);
// `Journal::snapshot_in_background` is there if that pause ever shows.
pub struct ServerCore {
    journal: Journal,
    catalog: Catalog,
    compact_bytes: u64,
    /// The library as it was at op `seq`, kept while devices read it in pages
    /// ([`ServerCore::snapshot_page`]), oldest first. Photos are shared with the live catalog, so
    /// a copy costs its map, not its photos.
    pins: Vec<(u64, Catalog)>,
}

/// Copies of the library kept for devices reading it in pages: one more device starting to read
/// drops the oldest, whose reader starts again.
const PINS_MAX: usize = 4;

/// A page of the library holds about this much JSON (and at least one photo).
const PAGE_BYTES: usize = 8 << 20;

impl ServerCore {
    pub fn open(store: Box<dyn Store>) -> Result<ServerCore> {
        let (journal, catalog, _) = Journal::open(store)?;
        Ok(ServerCore { journal, catalog, compact_bytes: COMPACT_BYTES, pins: Vec::new() })
    }

    /// Compact the log at this size instead of [`COMPACT_BYTES`] (tests).
    pub fn set_compact_bytes(&mut self, bytes: u64) {
        self.compact_bytes = bytes.max(1);
    }

    /// Read the log `bytes` at a time when pulling (tests make it small).
    pub fn set_pull_window(&mut self, bytes: usize) {
        self.journal.set_pull_window(bytes);
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
                    && let Err(e) = self.journal.snapshot_keeping(&self.catalog, self.compact_bytes / KEEP_DIVISOR)
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
        if after < self.journal.log_floor() {
            return Ok(Pull::Gone);
        }
        Ok(Pull::Ops(self.journal.records_since(after, limit)?))
    }

    /// The current state as a snapshot (JSON, [`Catalog::from_snapshot`]) and the op it's at.
    pub fn snapshot(&self) -> (u64, String) {
        (self.head(), self.catalog.to_snapshot())
    }

    /// The library in pages of at most `limit` photos, all as it was when the first page was read,
    /// however the library changes meanwhile: `cursor` is `None` for the first page (which also
    /// has the albums, stacks and the rest), then the last page's [`proto::Snapshot::next`].
    /// `None`: that copy is gone (dropped for newer ones, or the server restarted and the library
    /// changed since): read it again from the first page. The answer's `library` is left empty.
    pub fn snapshot_page(&mut self, cursor: Option<&str>, limit: usize) -> Option<proto::Snapshot> {
        let (seq, after) = match cursor {
            None => (self.head(), None),
            Some(c) => {
                let (seq, after) = c.split_once('.')?;
                (seq.parse().ok()?, Some(PhotoId(after.parse().ok()?)))
            }
        };
        if !self.pins.iter().any(|(s, _)| *s == seq) {
            // (a later page whose copy is gone can go on only if nothing changed since)
            if seq != self.head() {
                return None;
            }
            if self.pins.len() >= PINS_MAX {
                self.pins.remove(0);
            }
            self.pins.push((seq, self.catalog.clone()));
        }
        let (_, all) = self.pins.iter().find(|(s, _)| *s == seq)?;
        let mut page = match after {
            Some(_) => Catalog::default(),
            None => Catalog {
                photos: Default::default(),
                albums: all.albums.clone(),
                next_photo: all.next_photo,
                next_album: all.next_album,
                stacks: all.stacks.clone(),
                next_stack: all.next_stack,
                label_names: all.label_names.clone(),
                browsed: all.browsed.clone(),
                keyword_list: all.keyword_list.clone(),
                folder_records: all.folder_records.clone(),
                revision: 0,
                ..Default::default()
            },
        };
        let from = after.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
        let mut rest = all.photos.range((from, std::ops::Bound::Unbounded)).peekable();
        let mut bytes = 0;
        while let Some((id, p)) = rest.next_if(|_| page.photos.len() < limit.max(1) && (page.photos.is_empty() || bytes < PAGE_BYTES)) {
            bytes += serde_json::to_vec(p).map_or(0, |j| j.len());
            page.photos.insert(*id, p.clone());
        }
        let next = rest.peek().and_then(|_| page.photos.last_key_value()).map(|(id, _)| format!("{seq}.{}", id.0));
        Some(proto::Snapshot { defaults: None, library: String::new(), seq, catalog: page, next, total: Some(all.photos.len()) })
    }
}

/// A page of the library for devices that read compact pages (`&compact=1`): each photo's develop
/// settings (its edit, import look and versions) are written as what differs from `defaults` —
/// these settings' defaults on this server — which come first in the page. Most photos are never
/// edited, so a photo takes a few hundred bytes instead of a few KB; and as the page says what its
/// photos are written against, defaults that change in a later version change no photo.
pub fn compact_page(page: proto::Snapshot) -> serde_json::Result<proto::Snapshot<Value>> {
    let defaults = serde_json::to_value(lightcraft_develop::DevelopSettings::default())?;
    let mut catalog = serde_json::to_value(&page.catalog)?;
    if let Some(Value::Object(photos)) = catalog.get_mut("photos") {
        for p in photos.values_mut() {
            each_settings(p, &mut |s| *s = diff(&defaults, s));
        }
    }
    Ok(proto::Snapshot { defaults: Some(defaults), library: page.library, seq: page.seq, catalog, next: page.next, total: page.total })
}

/// The develop settings a photo holds (in its JSON form).
fn each_settings(photo: &mut Value, f: &mut dyn FnMut(&mut Value)) {
    for k in ["develop", "import_look"] {
        if let Some(s) = photo.get_mut(k).filter(|s| !s.is_null()) {
            f(s);
        }
    }
    for k in ["versions", "history"] {
        if let Some(Value::Array(steps)) = photo.get_mut(k) {
            for s in steps.iter_mut().filter_map(|v| v.get_mut("settings")) {
                f(s);
            }
        }
    }
}

/// What turns `base` into `v` as a merge patch (RFC 7396): objects hold only the members that
/// differ, `null` for one `v` doesn't have; anything else is `v` whole. [`patch`] undoes it, up to
/// a `null` member being left out (the same to an `Option`).
pub(crate) fn diff(base: &Value, v: &Value) -> Value {
    let (Value::Object(b), Value::Object(o)) = (base, v) else { return v.clone() };
    let mut d = serde_json::Map::new();
    for (k, ov) in o {
        match b.get(k) {
            Some(bv) if same(bv, ov) => {}
            Some(bv @ Value::Object(_)) if ov.is_object() => {
                d.insert(k.clone(), diff(bv, ov));
            }
            _ => {
                d.insert(k.clone(), ov.clone());
            }
        }
    }
    for k in b.keys().filter(|k| !o.contains_key(*k)) {
        d.insert(k.clone(), Value::Null);
    }
    Value::Object(d)
}

/// `a` and `b` are the same JSON, numbers compared as written: `Value`'s `==` takes `-0.0` for `0.0`, so a diff by it
/// would lose the sign of a zero (a slider at `-0.0` read back as `0.0`).
pub(crate) fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(p), Some(q)) if x.is_f64() && y.is_f64() => p.to_bits() == q.to_bits(),
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| same(p, q))),
        _ => a == b,
    }
}

/// `base` with merge patch `p` ([`diff`]) applied.
pub(crate) fn patch(base: &Value, p: &Value) -> Value {
    let (Value::Object(b), Value::Object(ps)) = (base, p) else { return p.clone() };
    let mut out = b.clone();
    for (k, pv) in ps {
        if pv.is_null() {
            out.remove(k);
        } else if let Some(bv) = out.get_mut(k).filter(|bv| bv.is_object() && pv.is_object()) {
            *bv = patch(bv, pv);
        } else {
            out.insert(k.clone(), pv.clone());
        }
    }
    Value::Object(out)
}

/// The server API's JSON bodies (`/api/…`; every route but `login` wants
/// `Authorization: Bearer <token>`).
pub mod proto {
    use serde::{Deserialize, Serialize};

    use std::sync::Arc;

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

    /// `GET /api/snapshot`: the library after op `seq`. In pages with `?limit=N`, then
    /// `?limit=N&cursor=<next>` until `next` is `None` ([`super::ServerCore::snapshot_page`];
    /// `410`: that copy is gone, start again). A server older than paging answers the whole library
    /// at once, without `next`.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Snapshot<C = Catalog> {
        /// Compact pages ([`super::compact_page`], `&compact=1`): the develop settings the photos'
        /// settings are written against. First, so a device tells a compact page by its start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub defaults: Option<serde_json::Value>,
        pub library: String,
        pub seq: u64,
        pub catalog: C,
        /// Where the next page starts (paged reads, until the last page).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub next: Option<String>,
        /// Photos in the whole library (paged reads).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub total: Option<usize>,
    }

    impl Snapshot {
        /// A page or a whole library as a server sends it, compact pages filled in from their
        /// `defaults`.
        pub fn from_json(text: &str) -> serde_json::Result<Snapshot> {
            if !text.starts_with("{\"defaults\":") {
                return serde_json::from_str(text);
            }
            // the settings are read as diffs against the defaults as they come (as `catalog.snap` is, [`crate::compact`]):
            // no JSON tree of the page, and the untouched photos share one copy of their settings
            let head = serde_json::from_str::<Snapshot<serde::de::IgnoredAny>>(text)?;
            let reader = head.defaults.map(|d| crate::compact::Reader::new(d).map(Arc::new)).transpose()?;
            let _reading = crate::compact::Reading::enter(reader);
            Ok(Snapshot { defaults: None, ..serde_json::from_str(text)? })
        }

        /// The next page read from the same copy of the library: its photos join these. `false`
        /// (nothing added) when it is from another copy.
        pub fn add_page(&mut self, page: Snapshot) -> bool {
            if (&page.library, page.seq) != (&self.library, self.seq) {
                return false;
            }
            self.catalog.photos.extend(page.catalog.photos);
            self.next = page.next;
            true
        }
    }

    /// `GET /api/ops?since=N&limit=M`: the ops after `N` (`410 Gone`: reload the snapshot).
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Ops {
        pub head: u64,
        pub ops: Vec<(u64, Op)>,
        /// Version of the presets document ([`Presets`]).
        #[serde(default)]
        pub presets: u64,
        /// Versions of the other shared documents ([`Doc`]) that have been written, by name.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        pub docs: std::collections::BTreeMap<String, u64>,
        /// The catalog format the server keeps, as far as sync goes ([`crate::journal::SYNC_VERSION`]; 0: a server
        /// older than this field). A device holds back changes a server of an older format would drop (hidden photos).
        #[serde(default)]
        pub format: u32,
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

    /// A link to an album that anyone who has it can open in a browser, no account needed
    /// (`https://<server>/s/<token>`): `GET /api/shares` → `{shares: [Share]}`, `POST /api/shares` ([`NewShare`])
    /// → a `Share`, `DELETE /api/shares/<id>`. The owner's; the token is the secret.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Share {
        pub id: String,
        pub token: String,
        pub album: u64,
        /// The album's name when the link was made.
        pub name: String,
        /// Seconds since 1970.
        pub created: u64,
        /// When it ends (seconds since 1970); `None`: never, until revoked.
        pub expires: Option<u64>,
        /// Visitors may download the original files, not only see the pictures.
        pub originals: bool,
        /// Visitors type a password before they see anything (the server keeps only its hash). Servers
        /// from before passwords leave it out.
        #[serde(default)]
        pub password: bool,
        /// Visitors may add photos to the album (they become the owner's photos, in this album).
        #[serde(default)]
        pub upload: bool,
    }

    /// `POST /api/shares`.
    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    pub struct NewShare {
        pub album: u64,
        /// The link ends after this many days (`None`: it doesn't).
        pub expires_days: Option<u32>,
        pub originals: bool,
        /// Visitors must type this first (`None` or empty: no password). At least 4 characters.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub password: Option<String>,
        /// Visitors may add photos to the album.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        pub upload: bool,
    }

    /// `GET` / `PUT /api/docs/<name>`: one of the small documents a user's devices share beside the library
    /// (their export, metadata and filter presets, curve presets, label and keyword sets, LUT profiles, a few
    /// preferences): a JSON array the devices merge themselves, by item. `PUT` carries the version it changed;
    /// `412` with the current document when the server's is newer, `404` for a name the server doesn't keep.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Doc {
        pub version: u64,
        pub items: serde_json::Value,
    }

    /// `/api/blobs/<kind>/<hash>` (`HEAD`, `GET`, `PUT`): a photo's original, its smart preview
    /// (≤ 2560 px) and its mini preview (≤ 512 px, for thumbnails), by content hash; and for a video the lighter
    /// copy the server keeps (`video`: an MP4 named by the *original's* hash, so a `PUT` carries the copy's own
    /// digest in `X-LightCraft-Digest`, see `docs/sync.md`).
    pub const BLOB_KINDS: [&str; 4] = ["original", "smart", "mini", "video"];

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

    impl Files {
        /// These files and `other`'s together.
        pub fn plus(self, other: Files) -> Files {
            Files { files: self.files.saturating_add(other.files), bytes: self.bytes.saturating_add(other.bytes) }
        }
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
    #[serde(default, rename_all = "camelCase")]
    pub struct Usage {
        /// Photos in the library: not videos, not those in Recently Deleted, not virtual copies. (A server before
        /// videos counted every photo record here.)
        pub photos: u64,
        /// Videos in the library, counted the same way. A server that doesn't say (0) is read as none.
        pub videos: u64,
        pub albums: u64,
        /// Signed-in devices.
        pub devices: u64,
        /// The files this server keeps by content: uploaded originals, smart previews, mini
        /// previews.
        pub original: Files,
        pub smart: Files,
        pub mini: Files,
        /// Photos in the user's library folders: read where they are on the server and never
        /// copied, so LightCraft keeps only their previews. They are still the library's photos
        /// and take that room: [`Usage::total`] counts them.
        pub folders: Files,
        /// Of `folders`' bytes, those on the same disk as the server's data (`disk`): they use up
        /// that disk's room like the files LightCraft keeps there. The others are on another
        /// disk (a backup drive, a second NAS). A server that doesn't say (0) is read as none.
        pub folders_on_disk: u64,
        /// The disk the server keeps its data on (`None`: it can't tell).
        pub disk: Option<Disk>,
        /// The most the server keeps for this user, in bytes ([`Usage::stored`] counts against it; `None`: no
        /// limit). An upload that would go over it is refused with `507`.
        pub quota: Option<u64>,
        /// What the videos take, and what the lighter copies the server keeps of them saved.
        pub video: VideoUsage,
    }

    /// The videos of a library ([`Usage::video`]): the size of the originals (read from the catalog, wherever they
    /// are: kept here or in a library folder), the lighter copies the server keeps of them (under `blobs/video`, part of
    /// [`Usage::stored`]) and what those saved.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    pub struct VideoUsage {
        /// The videos' own files: how many have a known size, and those sizes added up.
        pub originals: Files,
        /// The lighter copies kept for them.
        pub encoded: Files,
        /// Bytes the lighter copies are smaller than the originals they stand for (copies that came out larger save nothing).
        pub saved: u64,
    }

    impl Usage {
        /// Bytes of the files LightCraft keeps for this user: uploaded originals, the previews and the lighter
        /// copies of videos (not the photos in library folders).
        pub fn stored(&self) -> u64 {
            self.original.bytes.saturating_add(self.smart.bytes).saturating_add(self.mini.bytes).saturating_add(self.video.encoded.bytes)
        }

        /// Bytes the user's library takes on the server in all: what LightCraft keeps and the
        /// photos it reads in place from library folders.
        pub fn total(&self) -> u64 {
            self.stored().saturating_add(self.folders.bytes)
        }

        /// Bytes the library takes of the server's `disk`: what LightCraft keeps there and the
        /// library folders that are on it.
        pub fn on_disk(&self) -> u64 {
            self.stored().saturating_add(self.folders_on_disk.min(self.folders.bytes))
        }
    }

    /// How far a batch of work on the server is.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    pub struct Progress {
        pub done: u64,
        pub total: u64,
        /// Seconds left at the rate so far (once the server can tell).
        pub eta_secs: Option<u64>,
    }

    impl Progress {
        /// Something is left to do.
        pub fn active(&self) -> bool {
            self.total > self.done
        }
    }

    /// The scan of the user's library folders ([`Activity::scan`]).
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    pub struct Scan {
        pub scanning: bool,
        /// `listing` (finding photo files; `files` counts up), `reading` (hashing and importing the
        /// new ones: `done` of `todo`) or `finishing`; empty when idle.
        pub phase: String,
        pub done: u64,
        pub todo: u64,
        /// Photo files found in the folders.
        pub files: u64,
        pub eta_secs: Option<u64>,
        /// When the last scan ended (unix seconds).
        pub last_scan: Option<u64>,
        /// Files that couldn't be read, and folders that couldn't be opened.
        pub problems: u64,
    }

    /// `GET /api/activity`: what the server is doing for this user right now. The cloud popover
    /// shows it; an answer is a snapshot (ask again for progress).
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    pub struct Activity {
        /// Scanning the library folders (`None`: the user has none).
        pub scan: Option<Scan>,
        /// Previews being built, of photos found in the library folders and of originals devices
        /// uploaded (`total` counts the batch being worked through).
        pub previews: Progress,
        /// Indexing photos for search by description; reading the text in them; finding the people
        /// in them (each `None`: not available for this user, or not installed).
        pub search: Option<Progress>,
        pub text: Option<Progress>,
        pub faces: Option<Progress>,
        /// Making lighter copies of the user's videos (`None`: the server's admin has the setting off, or there are
        /// no videos). `done` counts the clips that have a copy or need none; the ones that failed are left out of `total`.
        pub video: Option<Progress>,
    }

    impl Activity {
        /// The server is working for this user.
        pub fn busy(&self) -> bool {
            self.scan.as_ref().is_some_and(|s| s.scanning)
                || self.previews.active()
                || [self.search, self.text, self.faces, self.video].into_iter().flatten().any(|p| p.active())
        }
    }
}

#[cfg(test)]
mod usage_tests {
    use super::proto::{Disk, Files, Usage, VideoUsage};

    fn files(files: u64, bytes: u64) -> Files {
        Files { files, bytes }
    }

    /// 98 GB of smart and 5.8 GB of small previews, and 2.5 TB of photos read in place: the library
    /// takes 2.6 TB, though LightCraft itself keeps only 104 GB of it.
    #[test]
    fn the_total_counts_the_photos_read_in_place() {
        let u = Usage {
            smart: files(147_821, 98_000_000_000),
            mini: files(147_823, 5_800_000_000),
            folders: files(192_438, 2_500_000_000_000),
            folders_on_disk: 2_500_000_000_000,
            disk: Some(Disk { total: 28_800_000_000_000, free: 10_700_000_000_000 }),
            ..Default::default()
        };
        assert_eq!(u.stored(), 103_800_000_000, "what LightCraft keeps");
        assert_eq!(u.total(), 2_603_800_000_000, "what the library takes in all");
        assert_eq!(u.on_disk(), u.total(), "all of it is on the server's disk");
        // folders on another disk take room there, not on the server's
        let elsewhere = Usage { folders_on_disk: 0, ..u.clone() };
        assert_eq!((elsewhere.total(), elsewhere.on_disk()), (2_603_800_000_000, 103_800_000_000));
        // a server that says nothing of disks (an older one) is read as none on it
        let older: Usage = serde_json::from_str(r#"{"smart": {"files": 1, "bytes": 10}, "folders": {"files": 2, "bytes": 90}}"#).unwrap();
        assert_eq!((older.stored(), older.total(), older.on_disk()), (10, 100, 10));
        // never more on the disk than the folders hold, and sums that can't overflow
        let odd = Usage { folders: files(1, 5), folders_on_disk: 99, ..Default::default() };
        assert_eq!(odd.on_disk(), 5);
        let huge = Usage { original: files(1, u64::MAX), folders: files(1, 7), folders_on_disk: 7, ..Default::default() };
        assert_eq!((huge.total(), huge.on_disk()), (u64::MAX, u64::MAX));
        assert_eq!(files(1, u64::MAX).plus(files(2, 5)), files(3, u64::MAX));
    }

    /// A server from before videos says nothing of them, and still reads: no videos, no lighter copies, nothing saved.
    /// The lighter copies are part of what the server keeps, and the new names are camel case.
    #[test]
    fn a_server_from_before_videos_still_reads_and_the_video_fields_have_their_names() {
        let older: Usage = serde_json::from_str(r#"{"photos": 7, "original": {"files": 1, "bytes": 5}, "smart": {"files": 1, "bytes": 3}}"#).unwrap();
        assert_eq!((older.photos, older.videos, older.video), (7, 0, VideoUsage::default()));
        assert_eq!(older.stored(), 8);
        let u = Usage {
            photos: 4,
            videos: 2,
            original: files(1, 100),
            video: VideoUsage { originals: files(2, 9_000), encoded: files(1, 2_500), saved: 3_000 },
            ..Default::default()
        };
        assert_eq!(u.stored(), 2_600, "the lighter copies are among the files the server keeps");
        let json = serde_json::to_value(&u).unwrap();
        assert_eq!((json["videos"].clone(), json["video"]["originals"]["bytes"].clone()), (2.into(), 9_000.into()));
        assert_eq!((json["video"]["encoded"]["files"].clone(), json["video"]["saved"].clone()), (1.into(), 3_000.into()));
        assert_eq!(serde_json::from_value::<Usage>(json).unwrap(), u);
    }

    /// The new field is `foldersOnDisk` on the wire, like the other multi-word names of the protocol.
    #[test]
    fn the_new_field_is_camel_case_and_the_old_ones_keep_their_names() {
        let u = Usage { photos: 3, folders: files(2, 9), folders_on_disk: 4, ..Default::default() };
        let json = serde_json::to_value(&u).unwrap();
        assert_eq!(json["foldersOnDisk"], 4);
        assert_eq!((json["photos"].clone(), json["folders"]["bytes"].clone()), (3.into(), 9.into()));
        assert_eq!(serde_json::from_value::<Usage>(json).unwrap(), u);
    }

    /// A server from before lighter videos sends no `video` in its activity and still reads; the clips being copied keep a
    /// device's status popover busy, and the video blob kind is one the server serves.
    #[test]
    fn an_older_server_has_no_video_activity_and_the_copies_keep_the_popover_busy() {
        use super::proto::{Activity, BLOB_KINDS, Progress};
        let older: Activity = serde_json::from_str(r#"{"previews": {"done": 3, "total": 3}, "search": {"done": 1, "total": 2}}"#).unwrap();
        assert_eq!(older.video, None);
        assert!(older.busy(), "the search index is still being made");
        let copying = Activity { video: Some(Progress { done: 1, total: 4, eta_secs: Some(60) }), ..Default::default() };
        assert!(copying.busy());
        let done = Activity { video: Some(Progress { done: 4, total: 4, eta_secs: None }), ..Default::default() };
        assert!(!done.busy());
        let json = serde_json::to_value(&copying).unwrap();
        assert_eq!((json["video"]["done"].clone(), json["video"]["etaSecs"].clone()), (1.into(), 60.into()));
        assert!(BLOB_KINDS.contains(&"video"));
    }
}
