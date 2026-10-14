//! `activity.list` / `activity.cancel` / `activity.external`: the background tasks in flight (see [`crate::activity`]).

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::activity::{Cancel, Unit};

/// Most rows an app may hold at once through `activity.external` (it reports few things).
const MAX_EXTERNAL: usize = 8;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "activity.list",
            "Background Tasks",
            [],
            None,
            "{} → {tasks: [{id, kind, label, done, total, unit: count|bytes|percent, detail, cancellable, cancelling, ageMs}]} — the long-running tasks in flight (imports, exports, preview builds, downloads, the face scan…), oldest first; total 0 = not known yet",
            always,
            |s, _| Ok(json!({"tasks": s.activity.list()}))
        ),
        cmd!(
            "activity.cancel",
            "Cancel Background Task",
            [],
            None,
            "{id} | {all: true} → {cancelled: n} — ask a task (or every cancellable task) to stop; it stops at its next file or photo. An unknown id (it may have just finished) or a task that can't be cancelled is an error",
            always,
            |s, p| {
                const C: &str = "activity.cancel";
                if p.get("all").and_then(Value::as_bool) == Some(true) {
                    return Ok(json!({"cancelled": s.activity.cancel_all()}));
                }
                let id = p.get("id").and_then(Value::as_u64).ok_or_else(|| bad(C, "id or all"))?;
                s.activity.cancel(id).map_err(|e| bad(C, e))?;
                Ok(json!({"cancelled": 1}))
            }
        ),
        cmd!(
            query "activity.external",
            "Report Background Task",
            [],
            None,
            "{action: start, label, cancellable?: bool} → {id} | {action: progress, id, done, total?: 100} | {action: end, id} — a row in the activity stack for work the app does by itself (the SwiftUI app's Apple model download: kind `download`); the app watches `activity.list` for `cancelling` on its id and ends the row when it stops",
            always,
            |s, p| {
                const C: &str = "activity.external";
                let id_of = |p: &Value| p.get("id").and_then(Value::as_u64).ok_or_else(|| bad(C, "missing `id`"));
                match p.get("action").and_then(Value::as_str) {
                    Some("start") => {
                        if s.external_tasks.len() >= MAX_EXTERNAL {
                            return Err(bad(C, "too many tasks are reported at once"));
                        }
                        let label = p.get("label").and_then(Value::as_str).filter(|l| !l.is_empty()).ok_or_else(|| bad(C, "missing `label`"))?;
                        let cancel = if p.get("cancellable").and_then(Value::as_bool) == Some(true) { Cancel::Yes } else { Cancel::No };
                        let guard = s.activity.start("download", label, cancel);
                        guard.set_unit(Unit::Percent);
                        guard.progress(0, 100);
                        let id = guard.id();
                        s.external_tasks.insert(id, guard);
                        Ok(json!({"id": id}))
                    }
                    Some("progress") => {
                        let guard = s.external_tasks.get(&id_of(p)?).ok_or_else(|| bad(C, "no such task (it ended)"))?;
                        let done = p.get("done").and_then(Value::as_u64).unwrap_or(0);
                        guard.progress(done, p.get("total").and_then(Value::as_u64).unwrap_or(100));
                        if let Some(detail) = p.get("detail").and_then(Value::as_str) {
                            guard.detail(detail);
                        }
                        Ok(json!({}))
                    }
                    Some("end") => {
                        // (an end for a row that is already gone is fine: the app ended it twice, or the session restarted)
                        s.external_tasks.remove(&id_of(p)?);
                        Ok(json!({}))
                    }
                    _ => Err(bad(C, "`action` is start, progress or end")),
                }
            }
        ),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;
    use crate::activity::Cancel;

    #[test]
    fn list_and_cancel_through_execute() {
        let mut s = Session::new();
        let g = s.activity.start("export", "Exporting", Cancel::Yes);
        let r = s.execute("activity.list", &json!({})).unwrap();
        assert_eq!(r["tasks"][0]["kind"], "export");
        assert_eq!(r["tasks"][0]["cancellable"], true);
        let r = s.execute("activity.cancel", &json!({"id": g.id()})).unwrap();
        assert_eq!(r["cancelled"], 1);
        assert!(g.is_cancelled());
    }

    #[test]
    fn an_external_row_shows_progresses_is_cancelled_and_ends() {
        let mut s = Session::new();
        let id = s
            .execute("activity.external", &json!({"action": "start", "label": "Downloading Apple's selection model", "cancellable": true}))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let row = |s: &mut Session| s.execute("activity.list", &json!({})).unwrap()["tasks"][0].clone();
        assert_eq!(
            (row(&mut s)["kind"].as_str(), row(&mut s)["unit"].as_str(), row(&mut s)["total"].as_u64()),
            (Some("download"), Some("percent"), Some(100))
        );
        s.execute("activity.external", &json!({"action": "progress", "id": id, "done": 40})).unwrap();
        assert_eq!(row(&mut s)["done"], 40);
        // ✕ in the stack: the app sees `cancelling` and stops
        s.execute("activity.cancel", &json!({"id": id})).unwrap();
        assert_eq!(row(&mut s)["cancelling"], true);
        s.execute("activity.external", &json!({"action": "end", "id": id})).unwrap();
        assert!(s.activity.list().is_empty());
        // ending again, or a row that never was, is not an error; progress on it is
        s.execute("activity.external", &json!({"action": "end", "id": id})).unwrap();
        assert!(s.execute("activity.external", &json!({"action": "progress", "id": id})).is_err());
    }

    #[test]
    fn external_rows_are_capped_and_need_a_label() {
        let mut s = Session::new();
        assert!(s.execute("activity.external", &json!({"action": "start"})).is_err());
        assert!(s.execute("activity.external", &json!({"action": "nonsense"})).is_err());
        for _ in 0..super::MAX_EXTERNAL {
            s.execute("activity.external", &json!({"action": "start", "label": "x"})).unwrap();
        }
        assert!(s.execute("activity.external", &json!({"action": "start", "label": "x"})).is_err());
        drop(s);
    }

    #[test]
    fn cancel_all_and_errors() {
        let mut s = Session::new();
        let (_a, _b) = (s.activity.start("export", "E", Cancel::Yes), s.activity.start("faces", "F", Cancel::No));
        assert_eq!(s.execute("activity.cancel", &json!({"all": true})).unwrap()["cancelled"], 1);
        assert!(s.execute("activity.cancel", &json!({"id": 999})).is_err());
        assert!(s.execute("activity.cancel", &json!({})).is_err());
    }
}
