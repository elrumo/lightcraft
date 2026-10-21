//! Dependency layering rules (plan/architecture.md §3).
//!
//! The rule engine works on a small, metadata-independent model so it can be
//! unit-tested; `from_metadata` builds that model from `cargo metadata`.

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;

/// Where a workspace crate sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Regular layered crate.
    Layer(u8),
    /// Layer 0, and additionally may depend on no workspace crate at all.
    Standalone,
    /// Test tooling: may depend on anything up to L5 (it sits at L6 for rule
    /// purposes); other crates may use it only as a dev-dependency.
    Testkit,
    /// Binaries and build tooling: exempt from the rules.
    Exempt,
}

impl Class {
    fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) => Some(l),
            Class::Standalone => Some(0),
            Class::Testkit => Some(5),
            Class::Exempt => None,
        }
    }
}

/// The layering table. Names are package names without the `lightcraft-`
/// prefix.
pub const TABLE: &[(&str, Class)] = &[
    ("geom", Class::Layer(0)),
    ("color", Class::Layer(0)),
    ("raster", Class::Layer(0)),
    ("tiff", Class::Layer(0)),
    ("sysmem", Class::Layer(0)),
    // cameras and cards through ImageCaptureCore (iOS, macOS; objc2): `ui-egui`'s camera host there
    ("capture", Class::Layer(0)),
    ("fetch", Class::Layer(0)),
    ("denoise-core", Class::Layer(1)),
    ("denoise", Class::Layer(1)),
    ("heif", Class::Layer(0)),
    // MP4 / MOV clips: probe and poster frames over FilmCraft's demuxer and decoders (the only crate that may use them)
    ("video", Class::Layer(1)),
    ("raw", Class::Layer(1)),
    ("codecs", Class::Layer(1)),
    ("meta", Class::Layer(1)),
    ("geo", Class::Layer(1)),
    ("develop", Class::Layer(1)),
    ("lensdb", Class::Layer(2)),
    ("inpaint", Class::Layer(1)),
    ("faces", Class::Layer(1)),
    ("geocode", Class::Layer(1)),
    ("scenes", Class::Layer(1)),
    ("slideshow", Class::Layer(1)),
    ("pipeline", Class::Layer(2)),
    ("gpu", Class::Layer(3)),
    ("catalog", Class::Layer(3)),
    ("preview", Class::Layer(3)),
    ("export", Class::Layer(3)),
    ("merge", Class::Layer(3)),
    ("segment", Class::Layer(3)),
    ("enhance", Class::Layer(3)),
    ("erase", Class::Layer(3)),
    ("diffuse", Class::Layer(3)),
    ("depth", Class::Layer(3)),
    ("vision", Class::Layer(3)),
    ("engine", Class::Layer(4)),
    // the frontend runtime shared by every UI (render scheduling, background pumps, the engine
    // thread a native frontend talks to); above `engine` in the L4 chain below
    ("apphost", Class::Layer(4)),
    ("ui-egui", Class::Layer(5)),
    ("mcp", Class::Layer(5)),
    ("testkit", Class::Testkit),
    // L6 apps and tooling
    ("lightcraft", Class::Exempt),
    ("cli", Class::Exempt),
    ("web", Class::Exempt),
    ("server", Class::Exempt),
    ("ios", Class::Exempt),
    // the iOS apps' native glue (UIKit, PhotosUI, ImageIO through objc2)
    ("ios-host", Class::Exempt),
    ("xtask", Class::Exempt),
];

/// Explicit orderings *within* a layer (earlier may be used by later).
/// L0: `raster` builds on `color` and `geom`; `color` uses `geom` for matrices.
/// L1: `raw` and `codecs` read metadata through `meta`; `develop` uses `meta` for XMP.
/// L3: `erase` and `depth` load their ONNX models with `vision`'s fixes; `diffuse` paints through
/// `erase`'s `Painter`; `enhance` downloads `erase`'s, `depth`'s and `diffuse`'s models.
/// L4: `apphost` runs frontends on top of `engine`.
pub const INTRA_LAYER_ORDER: &[&[&str]] = &[
    &["geom", "color", "raster"],
    &["vision", "erase", "diffuse", "enhance"],
    &["vision", "depth", "enhance"],
    &["tiff", "raster"],
    &["denoise-core", "denoise"],
    &["meta", "raw"],
    &["meta", "codecs"],
    &["meta", "develop"],
    &["develop", "scenes"],
    &["codecs", "raw"],
    &["engine", "apphost"],
];

fn intra_layer_allowed(from: &str, to: &str) -> bool {
    let (from, to) = (short_name(from), short_name(to));
    INTRA_LAYER_ORDER.iter().any(|chain| match (chain.iter().position(|n| *n == from), chain.iter().position(|n| *n == to)) {
        (Some(f), Some(t)) => t < f,
        _ => false,
    })
}

/// External crates that constitute a UI toolkit / windowing dependency.
/// Entries ending in `*` are prefixes.
pub const UI_CRATES: &[&str] = &["craft-ui", "egui", "eframe", "winit", "egui_kittest", "rfd", "bevy*"];

/// First layer allowed to use UI crates.
pub const UI_MIN_LAYER: u8 = 5;

/// Only this crate may depend on FilmCraft's crates (`filmcraft-*`, one git pin for all of them, docs/video.md): their API
/// churn stays in one place.
pub const FILMCRAFT_USERS: &[&str] = &["video"];

/// FilmCraft crates that must not be used at all: `filmcraft-codecs` pulls symphonia (MPL-2.0).
pub const FILMCRAFT_BANNED: &[&str] = &["filmcraft-codecs"];

pub fn short_name(pkg: &str) -> &str {
    pkg.strip_prefix("lightcraft-").unwrap_or(pkg)
}

pub fn classify(pkg: &str) -> Option<Class> {
    let s = short_name(pkg);
    TABLE.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
}

fn is_ui_crate(name: &str) -> bool {
    UI_CRATES.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
    /// `true` if the dependency is a workspace member.
    pub workspace: bool,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unregistered { krate: String },
    Upward { krate: String, dep: String, from: u8, to: u8, kind: DepKind },
    StandaloneHasWorkspaceDep { krate: String, dep: String },
    TestkitAsNormalDep { krate: String },
    UiBelowL5 { krate: String, dep: String, layer: u8 },
    FilmcraftOutsideVideo { krate: String, dep: String },
    FilmcraftBanned { krate: String, dep: String },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered { krate } => {
                write!(f, "{krate}: unknown workspace crate; register it in xtask/src/layers.rs TABLE (see plan/architecture.md §3)")
            }
            Violation::Upward { krate, dep, from, to, kind } => {
                write!(f, "{krate} (L{from}) -> {dep} (L{to}) [{kind:?}]: may only depend on strictly lower layers")
            }
            Violation::StandaloneHasWorkspaceDep { krate, dep } => {
                write!(f, "{krate}: standalone crate must not depend on workspace crate {dep}")
            }
            Violation::TestkitAsNormalDep { krate } => {
                write!(f, "{krate}: lightcraft-testkit may only be a dev-dependency")
            }
            Violation::UiBelowL5 { krate, dep, layer } => {
                write!(f, "{krate} (L{layer}) depends on UI crate `{dep}`; UI toolkits are only allowed in L5+")
            }
            Violation::FilmcraftOutsideVideo { krate, dep } => {
                write!(f, "{krate}: depends on FilmCraft's `{dep}`; only lightcraft-video may (docs/video.md)")
            }
            Violation::FilmcraftBanned { krate, dep } => {
                write!(f, "{krate}: `{dep}` is not allowed (it pulls symphonia, MPL-2.0; docs/video.md)")
            }
        }
    }
}

/// Check all rules. Returns violations sorted for stable output.
pub fn check(crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates {
        // (apps and tooling too)
        for d in c.deps.iter().filter(|d| d.name.starts_with("filmcraft-")) {
            if FILMCRAFT_BANNED.contains(&d.name.as_str()) {
                out.push(Violation::FilmcraftBanned { krate: c.name.clone(), dep: d.name.clone() });
            } else if !FILMCRAFT_USERS.contains(&short_name(&c.name)) {
                out.push(Violation::FilmcraftOutsideVideo { krate: c.name.clone(), dep: d.name.clone() });
            }
        }
        let Some(class) = classify(&c.name) else {
            out.push(Violation::Unregistered { krate: c.name.clone() });
            continue;
        };
        if class == Class::Exempt {
            continue;
        }
        let layer = class.layer().unwrap_or(0);
        for d in &c.deps {
            // Self dev-dependencies (e.g. to enable features in tests) are fine.
            if d.name == c.name {
                continue;
            }
            if d.workspace {
                if class == Class::Standalone {
                    out.push(Violation::StandaloneHasWorkspaceDep { krate: c.name.clone(), dep: d.name.clone() });
                    continue;
                }
                match classify(&d.name) {
                    // Unregistered deps are reported on their own entry.
                    None => {}
                    Some(Class::Testkit) if d.kind != DepKind::Dev => out.push(Violation::TestkitAsNormalDep { krate: c.name.clone() }),
                    Some(Class::Testkit) => {}
                    Some(dc) => {
                        let to = dc.layer().unwrap_or(u8::MAX);
                        if to >= layer && !(to == layer && intra_layer_allowed(&c.name, &d.name)) {
                            out.push(Violation::Upward {
                                krate: c.name.clone(),
                                dep: d.name.clone(),
                                from: layer,
                                to: if to == u8::MAX { 7 } else { to },
                                kind: d.kind,
                            });
                        }
                    }
                }
            } else if layer < UI_MIN_LAYER && is_ui_crate(&d.name) {
                out.push(Violation::UiBelowL5 { krate: c.name.clone(), dep: d.name.clone(), layer });
            }
        }
    }
    out.sort_by_key(|v| v.to_string());
    out.dedup();
    out
}

/// Drop the workspace packages whose manifest git does not track (`tracked`: repository-relative paths): a checkout
/// nested under `apps/` that is not part of this repository is not covered by its rules. A manifest outside `root`
/// is kept.
pub fn retain_tracked(meta: &mut Value, root: &Path, tracked: &HashSet<String>) {
    let Some(pkgs) = meta["packages"].as_array_mut() else { return };
    pkgs.retain(|p| {
        let rel = p["manifest_path"].as_str().and_then(|m| Path::new(m).strip_prefix(root).ok());
        rel.is_none_or(|r| tracked.contains(r.to_string_lossy().as_ref()))
    });
}

/// Build the model from `cargo metadata --format-version 1 --no-deps`.
pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let members: Vec<&str> = pkgs.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in pkgs {
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let mut deps = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let dname = d["name"].as_str().unwrap_or_default().to_owned();
            let kind = match d["kind"].as_str() {
                Some("dev") => DepKind::Dev,
                Some("build") => DepKind::Build,
                _ => DepKind::Normal,
            };
            let workspace = members.contains(&dname.as_str()) || d["path"].is_string();
            deps.push(Dep { name: dname, kind, workspace });
        }
        out.push(Crate { name, deps });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn describe(class: Option<Class>) -> String {
    match class {
        Some(Class::Layer(l)) => format!("L{l}"),
        Some(Class::Standalone) => "L0 standalone".into(),
        Some(Class::Testkit) => "testkit".into(),
        Some(Class::Exempt) => "exempt".into(),
        None => "UNREGISTERED".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, deps: &[(&str, DepKind, bool)]) -> Crate {
        Crate { name: name.into(), deps: deps.iter().map(|(n, k, w)| Dep { name: (*n).into(), kind: *k, workspace: *w }).collect() }
    }
    use DepKind::*;

    #[test]
    fn clean_downward_graph_passes() {
        let g = [
            c("lightcraft-geom", &[("serde", Normal, false)]),
            c("lightcraft-develop", &[("lightcraft-geom", Normal, true)]),
            c("lightcraft-engine", &[("lightcraft-develop", Normal, true), ("lightcraft-testkit", Dev, true)]),
            c("lightcraft-ui-egui", &[("lightcraft-engine", Normal, true), ("egui", Normal, false)]),
            c("lightcraft-cli", &[("lightcraft-ui-egui", Normal, true)]),
        ];
        assert!(check(&g).is_empty(), "{:?}", check(&g));
    }

    #[test]
    fn upward_dependency_flagged() {
        let v = check(&[c("lightcraft-develop", &[("lightcraft-engine", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 1, to: 4, .. }]));
    }

    #[test]
    fn sideways_dependency_flagged() {
        let v = check(&[c("lightcraft-catalog", &[("lightcraft-preview", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 3, to: 3, .. }]));
    }

    #[test]
    fn l0_chain_allowed_one_way() {
        assert!(check(&[c("lightcraft-raster", &[("lightcraft-color", Normal, true)])]).is_empty());
        assert!(!check(&[c("lightcraft-color", &[("lightcraft-raster", Normal, true)])]).is_empty());
    }

    #[test]
    fn ui_crates_forbidden_below_l5() {
        for dep in ["craft-ui", "egui", "eframe", "winit", "rfd"] {
            let v = check(&[c("lightcraft-engine", &[(dep, Normal, false)])]);
            assert!(matches!(v[..], [Violation::UiBelowL5 { .. }]), "{dep}");
        }
        assert!(check(&[c("lightcraft-mcp", &[("winit", Normal, false)])]).is_empty());
    }

    #[test]
    fn unregistered_and_testkit_rules() {
        let v = check(&[c("lightcraft-mystery", &[])]);
        assert!(matches!(&v[..], [Violation::Unregistered { krate }] if krate == "lightcraft-mystery"));
        assert!(!check(&[c("lightcraft-pipeline", &[("lightcraft-testkit", Normal, true)])]).is_empty());
        assert!(check(&[c("lightcraft-pipeline", &[("lightcraft-testkit", Dev, true)])]).is_empty());
    }

    #[test]
    fn apphost_sits_above_engine() {
        assert!(check(&[c("lightcraft-apphost", &[("lightcraft-engine", Normal, true), ("lightcraft-preview", Normal, true)])]).is_empty());
        assert!(!check(&[c("lightcraft-engine", &[("lightcraft-apphost", Normal, true)])]).is_empty());
        assert!(check(&[c("lightcraft-ui-egui", &[("lightcraft-apphost", Normal, true)])]).is_empty());
        let v = check(&[c("lightcraft-apphost", &[("egui", Normal, false)])]);
        assert!(matches!(v[..], [Violation::UiBelowL5 { layer: 4, .. }]), "{v:?}");
    }

    #[test]
    fn untracked_workspace_crates_are_not_covered() {
        let pkg = |name: &str, path: &str| serde_json::json!({ "name": name, "manifest_path": path, "dependencies": [] });
        let mut meta = serde_json::json!({ "packages": [
            pkg("lightcraft-geom", "/repo/crates/geom/Cargo.toml"),
            pkg("nested-private-app", "/repo/apps/nested/Cargo.toml"),
            pkg("elsewhere", "/other/place/Cargo.toml"),
        ] });
        let tracked: HashSet<String> = ["crates/geom/Cargo.toml".to_string()].into();
        retain_tracked(&mut meta, Path::new("/repo"), &tracked);
        let names: Vec<String> = from_metadata(&meta).unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["elsewhere", "lightcraft-geom"]);
    }

    #[test]
    fn filmcraft_is_for_the_video_crate_alone() {
        assert!(check(&[c("lightcraft-video", &[("filmcraft-h264", Normal, false), ("filmcraft-h264enc", Dev, false)])]).is_empty());
        let v = check(&[c("lightcraft-engine", &[("filmcraft-isobmff", Normal, false)])]);
        assert!(matches!(v[..], [Violation::FilmcraftOutsideVideo { .. }]), "{v:?}");
        let v = check(&[c("lightcraft-server", &[("filmcraft-h264", Normal, false)])]);
        assert!(matches!(v[..], [Violation::FilmcraftOutsideVideo { .. }]), "apps too: {v:?}");
        let v = check(&[c("lightcraft-video", &[("filmcraft-codecs", Normal, false)])]);
        assert!(matches!(v[..], [Violation::FilmcraftBanned { .. }]), "{v:?}");
    }

    #[test]
    fn apps_exempt() {
        for app in ["lightcraft", "lightcraft-cli", "lightcraft-web", "lightcraft-ios", "xtask"] {
            assert!(check(&[c(app, &[("egui", Normal, false), ("lightcraft-ui-egui", Normal, true)])]).is_empty());
        }
    }
}
