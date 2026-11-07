//! What a library's catalog costs in memory once open, by part: the heap of the whole session after the open (as an app
//! opens it), then each part of the photo records measured by copying it under a counting allocator (a shared settings copy
//! counted once), and the peak of a second copy of the catalog read beside the first (what a sync reload does).
//!
//! `cargo run --release -p lightcraft-apphost --example mem_bench -- <library folder>`
//!
//! Run it on a copy of a library's catalog files (`catalog.snap`, `catalog.log`, `prefs.json`, `presets.json`), never with
//! its `sync.*` files: the engine would talk to the library's server.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use lightcraft_catalog::{Photo, Source};
use lightcraft_sysmem::{CountingAlloc, counted};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// The process's resident memory in bytes (`ps`), freed pages the allocator kept included.
fn rss() -> usize {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<usize>().ok())
        .map_or(0, |kb| kb * 1024)
}

fn mb(b: usize) -> f64 {
    b as f64 / 1_048_576.0
}

/// The heap `make` allocates (what is still alive when it returns, kept until the result drops).
fn cost<T>(make: impl FnOnce() -> T) -> usize {
    let before = counted().0;
    let kept = make();
    let used = counted().0.saturating_sub(before);
    drop(kept);
    used
}

#[derive(Default)]
struct Part {
    bytes: usize,
    count: usize,
}

fn main() {
    let Some(library) = std::env::args().nth(1) else {
        eprintln!("usage: mem_bench <library folder>");
        std::process::exit(2);
    };
    {
        use std::mem::size_of;
        println!(
            "sizes: Photo {} B, Meta {} B, Option<Analysis> {} B, Option<FocusPoint> {} B, Option<Box<EmbeddedLens>> {} B",
            size_of::<Photo>(),
            size_of::<lightcraft_catalog::Meta>(),
            size_of::<Option<lightcraft_catalog::Analysis>>(),
            std::mem::size_of_val(&lightcraft_catalog::Meta::default().focus),
            size_of::<Option<Box<lightcraft_develop::EmbeddedLens>>>()
        );
    }
    let start = counted().0;
    let t = Instant::now();
    let s = match lightcraft_apphost::startup::open_session_or_err(Some(std::path::Path::new(&library)), false) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("can't open {library}: {e}");
            std::process::exit(1);
        }
    };
    let (open, open_peak) = counted();
    let resident = rss();
    let returned = lightcraft_sysmem::release_free_memory();
    println!("resident after open {:8.1} MB, {:.1} MB after giving freed pages back ({:.1} MB returned)", mb(resident), mb(rss()), mb(returned));
    {
        use std::mem::size_of;
        println!(
            "sizes: Photo {} B, Meta {} B, Option<EmbeddedLens> {} B, DevelopSettings {} B, Source {} B, String {} B",
            size_of::<Photo>(),
            size_of::<lightcraft_catalog::Meta>(),
            size_of::<Option<lightcraft_develop::EmbeddedLens>>(),
            size_of::<lightcraft_develop::DevelopSettings>(),
            size_of::<Source>(),
            size_of::<String>()
        );
    }
    let photos: Vec<Arc<Photo>> = s.catalog.photos().cloned().collect();
    let n = photos.len().max(1);
    println!(
        "session open        {:8.1} MB heap ({:.0} B a photo), peak {:.1} MB, {} photos, {:.1} s",
        mb(open - start),
        (open - start) as f64 / n as f64,
        mb(open_peak),
        photos.len(),
        t.elapsed().as_secs_f64()
    );

    let mut parts: Vec<(&str, Part)> = Vec::new();
    let mut add = |name: &'static str, bytes: usize| {
        if let Some(p) = parts.iter_mut().find(|p| p.0 == name) {
            p.1.bytes += bytes;
            p.1.count += 1;
        } else {
            parts.push((name, Part { bytes, count: 1 }));
        }
    };
    let mut develops: HashSet<usize> = HashSet::new();
    let mut history_settings: HashSet<usize> = HashSet::new();
    let mut strings: HashMap<&'static str, HashSet<String>> = HashMap::new();
    let (mut edited, mut with_history, mut steps, mut remote) = (0usize, 0usize, 0usize, 0usize);
    let (mut lens, mut rights, mut regions, mut focus) = (0usize, 0usize, 0usize, 0usize);
    let defaults = lightcraft_develop::DevelopSettings::default();
    let (mut default_settings, mut default_settings_copies) = (0usize, HashSet::new());
    let mut by_value: HashSet<String> = HashSet::new();
    for p in &photos {
        add("Photo record (struct in its Arc)", std::mem::size_of::<Photo>() + 2 * std::mem::size_of::<usize>());
        if let Source::File { path } = &p.source {
            add("source path", cost(|| path.clone()));
            if !std::path::Path::new(path).exists() {
                remote += 1;
            }
        }
        add("file name, format, dates", cost(|| (p.file_name.clone(), p.format.clone(), p.captured.clone(), p.imported.clone(), p.edited.clone())));
        add("content hash", cost(|| p.content_hash.clone()));
        let m = &p.meta;
        add(
            "metadata text (camera, lens, place, rights…)",
            cost(|| {
                (
                    m.camera.clone(),
                    m.lens.clone(),
                    m.shutter.clone(),
                    (m.location.clone(), m.city.clone(), m.state.clone(), m.country.clone()),
                    (m.title.clone(), m.caption.clone(), m.alt_text.clone(), m.extended_description.clone()),
                    (m.copyright.clone(), m.usage_terms.clone(), m.copyright_url.clone(), m.creator.clone()),
                )
            }),
        );
        add("keywords", cost(|| m.keywords.clone()));
        lens += usize::from(p.embedded_lens.is_some());
        rights += usize::from(
            !(m.copyright.is_empty()
                && m.usage_terms.is_empty()
                && m.copyright_url.is_empty()
                && m.creator.is_empty()
                && m.alt_text.is_empty()
                && m.extended_description.is_empty()),
        );
        regions += usize::from(!m.regions.is_empty());
        focus += usize::from(m.focus.is_some());
        if *p.develop == defaults {
            default_settings += 1;
            default_settings_copies.insert(Arc::as_ptr(&p.develop) as usize);
        }
        add("face / person regions", cost(|| m.regions.clone()));
        for (name, value) in [("camera", &m.camera), ("lens", &m.lens), ("format", &p.format), ("shutter", &m.shutter)] {
            strings.entry(name).or_default().insert(value.clone());
        }
        if !develops.contains(&(Arc::as_ptr(&p.develop) as usize)) {
            by_value.insert(serde_json::to_string(&*p.develop).unwrap_or_default());
        }
        if develops.insert(Arc::as_ptr(&p.develop) as usize) {
            add(
                "develop settings (distinct copies)",
                cost(|| (*p.develop).clone()) + std::mem::size_of::<lightcraft_develop::DevelopSettings>() + 16,
            );
        }
        if !p.history.is_empty() {
            with_history += 1;
        }
        if p.edited.is_some() {
            edited += 1;
        }
        add(
            "history steps (vec + labels)",
            cost(|| p.history.iter().map(|h| h.label.clone()).collect::<Vec<_>>())
                + p.history.capacity() * std::mem::size_of::<lightcraft_catalog::HistoryStep>(),
        );
        for h in &p.history {
            steps += 1;
            if history_settings.insert(Arc::as_ptr(&h.settings) as usize) && !develops.contains(&(Arc::as_ptr(&h.settings) as usize)) {
                add(
                    "history settings (distinct copies)",
                    cost(|| (*h.settings).clone()) + std::mem::size_of::<lightcraft_develop::DevelopSettings>() + 16,
                );
            }
        }
        add("versions", cost(|| p.versions.clone()));
        if let Some(look) = &p.import_look
            && develops.insert(Arc::as_ptr(look) as usize)
        {
            add("import look (distinct copies)", cost(|| (**look).clone()) + std::mem::size_of::<lightcraft_develop::DevelopSettings>() + 16);
        }
    }
    let total: usize = parts.iter().map(|p| p.1.bytes).sum();
    println!("\nphoto records, by part (heap a copy of each takes):");
    let mut sorted: Vec<_> = parts.iter().collect();
    sorted.sort_by_key(|p| std::cmp::Reverse(p.1.bytes));
    for (name, p) in sorted {
        println!("  {:46} {:8.1} MB  {:6.0} B a photo  ({} copies)", name, mb(p.bytes), p.bytes as f64 / n as f64, p.count);
    }
    println!("  {:46} {:8.1} MB", "sum", mb(total));
    println!("  {:46} {:8.1} MB  (indexes, maps, albums, the engine's state)", "rest of the session", mb((open - start).saturating_sub(total)));
    println!(
        "\n{} photos edited, {} with history, {} history steps, {} distinct develop settings, {} distinct history settings, {} sources not on this disk",
        edited,
        with_history,
        steps,
        develops.len(),
        history_settings.len(),
        remote
    );
    println!(
        "{} with embedded lens data, {} with rights / alt text / long description, {} with face regions, {} with a focus point; {} with default settings in {} copies",
        lens,
        rights,
        regions,
        focus,
        default_settings,
        default_settings_copies.len()
    );
    println!("the {} distinct develop settings copies hold {} distinct values", develops.len(), by_value.len());
    for (name, set) in &strings {
        println!("distinct {name}: {}", set.len());
    }

    // a sync reload reads the downloaded snapshot into a second catalog beside the open one
    drop(photos);
    let before = counted().0;
    lightcraft_sysmem::reset_counted_peak();
    let t = Instant::now();
    let other = std::env::args().nth(2).unwrap_or_default();
    let second = lightcraft_apphost::startup::open_session_or_err(Some(std::path::Path::new(&other)), false);
    if let Err(e) = &second {
        println!("\nsecond copy ({other}): {e}");
    }
    let (after, peak) = counted();
    println!(
        "\na second session beside the first: +{:.1} MB heap, peak {:.1} MB above the first ({:.1} s)",
        mb(after.saturating_sub(before)),
        mb(peak.saturating_sub(before)),
        t.elapsed().as_secs_f64()
    );
    drop(second);
    drop(s);
}
