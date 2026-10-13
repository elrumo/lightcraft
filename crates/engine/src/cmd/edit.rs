use serde_json::{Value, json};

use super::{CommandSpec, always, bad, can_redo, can_undo, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("edit.undo", "Undo", ["Edit"], Some("Cmd+Z"), "{}", can_undo, |s, _| {
            s.end_interaction()?;
            Ok(json!({"undone": s.undo_step()?}))
        }),
        cmd!("edit.redo", "Redo", ["Edit"], Some("Cmd+Shift+Z"), "{}", can_redo, |s, _| Ok(json!({"redone": s.redo_step()?}))),
        cmd!(
            query "edit.stack",
            "Undo Stack",
            [],
            None,
            "{} → {depth, undo: [labels, newest first], redo: [labels, next first]} what Undo and Redo would take back and put back (depth: steps there are to undo, for `edit.undoTo`)",
            always,
            |s, _| {
                let undo: Vec<&str> = s.undo.iter().rev().map(|u| u.label.as_str()).collect();
                let redo: Vec<&str> = s.redo.iter().rev().map(|u| u.label.as_str()).collect();
                Ok(json!({"depth": s.undo.len(), "undo": undo, "redo": redo}))
            }
        ),
        cmd!(
            "edit.undoTo",
            "Undo to Step",
            [],
            None,
            "{depth} — undo until `depth` steps are left (a tool screen's Cancel takes back everything done since it opened, as `edit.stack` gave its depth); → {undone: n}",
            always,
            |s, p| {
                let depth = p.get("depth").and_then(Value::as_u64).ok_or_else(|| bad("edit.undoTo", "missing `depth`"))? as usize;
                s.end_interaction()?;
                let mut undone = 0;
                while s.undo.len() > depth {
                    s.undo_step()?;
                    undone += 1;
                }
                Ok(json!({"undone": undone}))
            }
        ),
    ]
}
