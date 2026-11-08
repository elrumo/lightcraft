//! Whether a sync reload of a library keeps one copy of its photos (HB-236): each photo of the library's catalog goes
//! the way the server sends it (its shared form, in compact pages) and is read back over the open catalog as a reload
//! reads it. A photo that comes back as the open one's copy costs nothing; one that differs is a second copy. Prints how
//! many are shared and which fields keep the others apart.
//!
//! `cargo run --release -p lightcraft-catalog --example reload_share -- <library folder>`
//!
//! Nothing is written, but run it on a copy of a library's `catalog.snap` and `catalog.log` all the same.

use std::collections::BTreeMap;

use lightcraft_catalog::sync::{compact_page, portable, proto, share_unchanged};
use lightcraft_catalog::{Catalog, FsStore, Journal, Op};
use serde_json::Value;

/// The fields (`meta.*` one level down) whose values differ between two photos.
fn differing(a: &Value, b: &Value, prefix: &str, out: &mut Vec<String>) {
    let (Value::Object(a), Value::Object(b)) = (a, b) else { return out.push(prefix.trim_end_matches('.').to_string()) };
    for k in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
        let (x, y) = (a.get(k).unwrap_or(&Value::Null), b.get(k).unwrap_or(&Value::Null));
        if x == y {
            continue;
        }
        if k == "meta" {
            differing(x, y, "meta.", out);
        } else {
            out.push(format!("{prefix}{k}"));
        }
    }
}

fn main() {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("usage: reload_share <library folder>");
        std::process::exit(2);
    };
    let opened = FsStore::open(&dir).map_err(|e| e.to_string()).and_then(|s| Journal::open(Box::new(s)).map_err(|e| e.to_string()));
    let (_journal, open, _) = match opened {
        Ok(o) => o,
        Err(e) => {
            eprintln!("can't open {dir}: {e}");
            std::process::exit(1);
        }
    };
    let photos: Vec<_> = open.photos().filter(|p| !p.local).collect();
    let (mut shared, mut sent) = (0usize, 0usize);
    let mut why: BTreeMap<String, usize> = BTreeMap::new();
    let mut example: BTreeMap<String, (Value, Value)> = BTreeMap::new();
    for chunk in photos.chunks(2000) {
        let mut page = Catalog::default();
        for p in chunk {
            if page.apply(Op::AddPhoto { photo: Box::new(portable(p)) }).is_ok() {
                sent += 1;
            }
        }
        let snap = proto::Snapshot { defaults: None, library: "lib".into(), seq: 1, catalog: page, next: None, total: None };
        let text = match compact_page(snap).and_then(|s| serde_json::to_string(&s)) {
            Ok(t) => t,
            Err(e) => return eprintln!("page: {e}"),
        };
        let mut back = match proto::Snapshot::from_json(&text) {
            Ok(b) => b,
            Err(e) => return eprintln!("reading the page back: {e}"),
        };
        share_unchanged(&open, &mut back.catalog);
        for p in back.catalog.photos() {
            let Some(old) = open.photo(p.id) else { continue };
            if std::sync::Arc::ptr_eq(old, p) {
                shared += 1;
                continue;
            }
            let (a, b) = (serde_json::to_value(&**old).unwrap_or_default(), serde_json::to_value(&**p).unwrap_or_default());
            let mut fields = Vec::new();
            differing(&a, &b, "", &mut fields);
            if fields.is_empty() {
                // equal as JSON, unequal as values: a float that prints the same (e.g. -0.0 and 0.0) or a NaN
                fields.push("(same JSON)".into());
            }
            for f in fields {
                example.entry(f.clone()).or_insert_with(|| (a.get(&f).cloned().unwrap_or_default(), b.get(&f).cloned().unwrap_or_default()));
                *why.entry(f).or_default() += 1;
            }
        }
    }
    let local = open.len() - photos.len();
    println!("{} photos ({local} Local records, which a server never has), {sent} sent", open.len());
    println!("{shared} came back as the open copy, {} as a second copy", sent.saturating_sub(shared));
    let cut = |v: &Value| v.to_string().chars().take(120).collect::<String>();
    for (f, n) in &why {
        let Some((a, b)) = example.get(f) else { continue };
        println!("  {f:30} differs in {n} photos  (open {}, read back {})", cut(a), cut(b));
    }
}
