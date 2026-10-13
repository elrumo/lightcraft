//! `apps/lightcraft-ios-swift/PARITY.md`: what the SwiftUI iOS app does of what the egui app does —
//! the gate for removing the egui iOS app (`docs/ios-swift.md` → *Parity gate*). Checked by
//! `cargo xtask parity` (in `ci`):
//!
//! - every UI-only command of the egui app (`UI_COMMANDS` / `LANGUAGE_COMMANDS` in
//!   `crates/ui-egui/src/menus.rs`) has a `ui:<id>` row and every dialog (`state::Dialog`) a
//!   `dialog:<Variant>` row, and no row names one that is gone: a new egui UI command needs a decision
//!   here in the same change;
//! - ids are unique, phases are S0, S1 or S2, statuses ✅ 🟡 ⬜ 🚫;
//! - every cited path exists, a ✅ row cites its evidence and a 🚫 row says why.
//!
//! `--write` refreshes the per-phase summary; `--ios-gate S1` (or `S0`, `S2`) fails unless every row
//! of that phase and the ones before it is ✅ or 🚫.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::parity::Status;

pub const DOC: &str = "apps/lightcraft-ios-swift/PARITY.md";
const BEGIN: &str = "<!-- ios-parity:summary -->";
const END: &str = "<!-- /ios-parity:summary -->";
const PHASES: [&str; 3] = ["S0", "S1", "S2"];
/// Directories whose paths are checked.
const CHECKED_DIRS: &[&str] = &["crates/", "apps/", "docs/", "xtask/", "assets/"];

#[derive(Clone, Debug)]
pub struct Row {
    pub id: String,
    pub phase: String,
    pub status: Status,
    /// Paths cited in the evidence column.
    pub evidence: Vec<String>,
    pub notes: String,
    pub line: usize,
}

#[derive(Debug, Default)]
pub struct Doc {
    pub rows: Vec<Row>,
    /// (line, path) for every repository path cited anywhere in a row.
    pub paths: Vec<(usize, String)>,
    pub errors: Vec<String>,
}

fn is_row_id(s: &str) -> bool {
    ["ui:", "dialog:", "screen:"].iter().any(|p| s.strip_prefix(p).is_some_and(|rest| !rest.is_empty() && !rest.contains(' ')))
}

fn paths_in(cell: &str) -> Vec<String> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .filter(|s| CHECKED_DIRS.iter().any(|d| s.starts_with(d)) && !s.contains(' '))
        .map(|s| s.split_once(':').map_or(s, |(p, _)| p).to_string())
        .collect()
}

pub fn parse(md: &str) -> Doc {
    let mut doc = Doc::default();
    let mut seen = HashSet::new();
    for (i, line) in md.lines().enumerate() {
        let n = i + 1;
        let t = line.trim();
        if !t.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
        let Some(id) = cells.first().copied().filter(|c| is_row_id(c)) else { continue };
        if cells.len() != 5 {
            doc.errors.push(format!("{DOC}:{n}: {id}: expected 5 columns (Id | Phase | Status | Evidence | Notes)"));
            continue;
        }
        let phase = cells[1].to_string();
        if !PHASES.contains(&phase.as_str()) {
            doc.errors.push(format!("{DOC}:{n}: {id}: phase `{phase}` is not one of S0, S1, S2"));
        }
        let Some(status) = Status::parse(cells[2]) else {
            doc.errors.push(format!("{DOC}:{n}: {id}: status `{}` is not one of ✅ 🟡 ⬜ 🚫", cells[2]));
            continue;
        };
        if !seen.insert(id.to_string()) {
            doc.errors.push(format!("{DOC}:{n}: duplicate id {id}"));
        }
        let evidence = paths_in(cells[3]);
        for p in evidence.iter().cloned().chain(paths_in(cells[4])) {
            doc.paths.push((n, p));
        }
        doc.rows.push(Row { id: id.to_string(), phase, status, evidence, notes: cells[4].to_string(), line: n });
    }
    doc
}

/// The variants of `pub enum Dialog` in `crates/ui-egui/src/state.rs`.
pub fn dialog_variants(state_rs: &str) -> Vec<String> {
    let Some(start) = state_rs.find("pub enum Dialog {") else { return Vec::new() };
    let body = &state_rs[start..];
    let mut out = Vec::new();
    let mut depth = 0i32;
    for (i, line) in body.lines().enumerate() {
        let t = line.trim();
        if i > 0 && depth == 1 && !t.starts_with("//") && !t.starts_with('#') {
            let name: String = t.chars().take_while(char::is_ascii_alphanumeric).collect();
            let rest = t[name.len()..].trim_start();
            if name.starts_with(|c: char| c.is_ascii_uppercase()) && (rest.is_empty() || rest.starts_with([',', '{', '('])) {
                out.push(name);
            }
        }
        depth += i32::try_from(line.matches('{').count()).unwrap_or(0) - i32::try_from(line.matches('}').count()).unwrap_or(0);
        if i > 0 && depth <= 0 {
            break;
        }
    }
    out
}

/// Problems (empty = OK).
pub fn check(doc: &Doc, ui_commands: &[String], dialogs: &[String], exists: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = doc.errors.clone();
    let have: HashSet<&str> = doc.rows.iter().map(|r| r.id.as_str()).collect();
    let wanted: Vec<String> = ui_commands.iter().map(|id| format!("ui:{id}")).chain(dialogs.iter().map(|d| format!("dialog:{d}"))).collect();
    for id in &wanted {
        if !have.contains(id.as_str()) {
            out.push(format!("{DOC}: no row for `{id}` (a new egui UI command or dialog: decide its phase here)"));
        }
    }
    let wanted: HashSet<&str> = wanted.iter().map(String::as_str).collect();
    for r in &doc.rows {
        if !r.id.starts_with("screen:") && !wanted.contains(r.id.as_str()) {
            out.push(format!("{DOC}:{}: `{}` no longer exists in the egui app: remove the row", r.line, r.id));
        }
        if r.status == Status::Done && r.evidence.is_empty() {
            out.push(format!("{DOC}:{}: `{}` is ✅ without evidence (a path in the Evidence column)", r.line, r.id));
        }
        if r.status == Status::OutOfScope && r.notes.trim().is_empty() {
            out.push(format!("{DOC}:{}: `{}` is 🚫 without a reason (Notes)", r.line, r.id));
        }
    }
    for (n, p) in &doc.paths {
        if !exists(p) {
            out.push(format!("{DOC}:{n}: missing path `{p}`"));
        }
    }
    out
}

/// Counts per phase: (done, partial, missing, out of scope).
fn counts(doc: &Doc) -> BTreeMap<&str, [usize; 4]> {
    let mut m: BTreeMap<&str, [usize; 4]> = BTreeMap::new();
    for r in &doc.rows {
        let c = m.entry(r.phase.as_str()).or_default();
        let i = match r.status {
            Status::Done => 0,
            Status::Partial => 1,
            Status::Missing => 2,
            Status::OutOfScope => 3,
        };
        c[i] += 1;
    }
    m
}

pub fn summary_table(doc: &Doc) -> String {
    let mut s = String::from("| Phase | ✅ | 🟡 | ⬜ | 🚫 | Done or out of scope |\n|---|---:|---:|---:|---:|---:|\n");
    let mut total = [0usize; 4];
    for (phase, c) in counts(doc) {
        let n: usize = c.iter().sum();
        let ok = c[0] + c[3];
        s += &format!("| {phase} | {} | {} | {} | {} | {ok}/{n} |\n", c[0], c[1], c[2], c[3]);
        for (t, v) in total.iter_mut().zip(c) {
            *t += v;
        }
    }
    let n: usize = total.iter().sum();
    s += &format!("| **Total** | {} | {} | {} | {} | {}/{n} |\n", total[0], total[1], total[2], total[3], total[0] + total[3]);
    s
}

/// The rows of `phase` and the phases before it that are neither ✅ nor 🚫.
pub fn gate<'a>(doc: &'a Doc, phase: &str) -> Result<Vec<&'a Row>, String> {
    let upto = PHASES.iter().position(|p| *p == phase).ok_or_else(|| format!("--ios-gate: `{phase}` is not one of S0, S1, S2"))?;
    Ok(doc
        .rows
        .iter()
        .filter(|r| PHASES.iter().position(|p| *p == r.phase).is_some_and(|i| i <= upto))
        .filter(|r| !matches!(r.status, Status::Done | Status::OutOfScope))
        .collect())
}

fn with_summary(md: &str, table: &str) -> Option<String> {
    let b = md.find(BEGIN)? + BEGIN.len();
    let e = md[b..].find(END)? + b;
    Some(format!("{}\n{}{}", &md[..b], table, &md[e..]))
}

pub fn run(root: &Path, write: bool, gate_phase: Option<&str>) -> Result<(), String> {
    let path = root.join(DOC);
    let md = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc = parse(&md);
    let menus = std::fs::read_to_string(root.join("crates/ui-egui/src/menus.rs")).map_err(|e| format!("menus.rs: {e}"))?;
    let state = std::fs::read_to_string(root.join("crates/ui-egui/src/state.rs")).map_err(|e| format!("state.rs: {e}"))?;
    let ui = crate::parity::ui_command_ids(&menus);
    let dialogs = dialog_variants(&state);
    if dialogs.is_empty() {
        return Err("crates/ui-egui/src/state.rs: `pub enum Dialog` not found (the parity check reads it)".into());
    }
    let problems = check(&doc, &ui, &dialogs, |p| root.join(p).exists());
    let table = summary_table(&doc);
    println!("\nSwiftUI app parity ({DOC}): {} rows ({} UI commands, {} dialogs)\n", doc.rows.len(), ui.len(), dialogs.len());
    print!("{table}");
    if write {
        let new = with_summary(&md, &table).ok_or_else(|| format!("{DOC}: summary markers `{BEGIN}` … `{END}` not found"))?;
        if new != md {
            std::fs::write(&path, new).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("\nupdated the summary in {DOC}");
        }
    } else if with_summary(&md, &table).is_some_and(|new| new != md) {
        println!("\nnote: the summary in {DOC} is stale — run `cargo xtask parity --write`");
    }
    if !problems.is_empty() {
        println!();
        for p in &problems {
            println!("  - {p}");
        }
        return Err(format!("{} problem(s) in {DOC}", problems.len()));
    }
    println!("\nOK: every egui UI command and dialog has a row in {DOC}.");
    if let Some(phase) = gate_phase {
        let open = gate(&doc, phase)?;
        if !open.is_empty() {
            for r in open.iter().take(20) {
                println!("  - {} ({}) is not done", r.id, r.phase);
            }
            return Err(format!("{} row(s) up to {phase} are neither ✅ nor 🚫", open.len()));
        }
        println!("Gate {phase}: every row up to {phase} is done or out of scope.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# SwiftUI app parity

<!-- ios-parity:summary -->
old
<!-- /ios-parity:summary -->

| Id | Phase | Status | Evidence | Notes |
|---|---|---|---|---|
| screen:compact.grid | S0 | ✅ | `apps/x/Grid.swift` | |
| ui:view.detail | S0 | ✅ | `apps/x/Loupe.swift`, `apps/x/Gone.swift` | |
| ui:app.quit | S2 | 🚫 | | iOS apps aren't quit |
| ui:view.map | S1 | ⬜ | | |
| ui:view.gone | S1 | ⬜ | | |
| dialog:Export | S3 | 🟡 | | |
| dialog:About | S1 | ✅ | | |
| ui:app.quit | S2 | 🚫 | | |
| ui:short | S1 |
";

    #[test]
    fn parses_rows_and_reports_their_mistakes() {
        let d = parse(SAMPLE);
        assert_eq!(d.rows.len(), 8, "{:?}", d.rows);
        assert!(d.errors.iter().any(|e| e.contains("phase `S3`")), "{:?}", d.errors);
        assert!(d.errors.iter().any(|e| e.contains("duplicate id ui:app.quit")));
        assert!(d.errors.iter().any(|e| e.contains("ui:short")));
        assert_eq!(d.rows[1].evidence, ["apps/x/Loupe.swift", "apps/x/Gone.swift"]);
    }

    #[test]
    fn every_command_and_dialog_needs_a_row_and_no_row_outlives_its_command() {
        let d = parse(SAMPLE);
        let ui = ["view.detail", "app.quit", "view.map", "view.new"].map(String::from);
        let dialogs = ["Export", "About", "Settings"].map(String::from);
        let p = check(&d, &ui, &dialogs, |p| p != "apps/x/Gone.swift");
        let has = |s: &str| p.iter().any(|e| e.contains(s));
        assert!(has("no row for `ui:view.new`") && has("no row for `dialog:Settings`"), "{p:?}");
        assert!(has("`ui:view.gone` no longer exists"), "{p:?}");
        assert!(has("`dialog:About` is ✅ without evidence"), "{p:?}");
        assert!(has("`ui:app.quit` is 🚫 without a reason"), "the duplicate without notes: {p:?}");
        assert!(has("missing path `apps/x/Gone.swift`"), "{p:?}");
        assert!(!has("screen:compact.grid"), "screens are curated, not checked against the egui app");
    }

    #[test]
    fn the_gate_counts_the_phase_and_the_ones_before_it() {
        let d = parse(SAMPLE);
        assert!(gate(&d, "S0").unwrap().is_empty());
        let s1: Vec<&str> = gate(&d, "S1").unwrap().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(s1, ["ui:view.map", "ui:view.gone"]);
        assert!(gate(&d, "S9").is_err());
        let table = summary_table(&d);
        assert!(table.contains("| S0 | 2 | 0 | 0 | 0 | 2/2 |"), "{table}");
        assert!(with_summary(SAMPLE, &table).is_some_and(|m| !m.contains("\nold\n")));
    }

    #[test]
    fn reads_the_dialog_variants() {
        let src = "pub enum Dialog {\n    ContactSheet {\n        options: X,\n    },\n    /// doc\n    NewAlbum { name: String },\n    WhatsNew,\n    Merge(Box<M>),\n}\nenum Other { Nope }";
        assert_eq!(dialog_variants(src), ["ContactSheet", "NewAlbum", "WhatsNew", "Merge"]);
        let real = dialog_variants(include_str!("../../crates/ui-egui/src/state.rs"));
        assert!(real.len() >= 30 && real.contains(&"Export".to_string()) && real.contains(&"Settings".to_string()), "{real:?}");
    }
}
