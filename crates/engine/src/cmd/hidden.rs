//! The Hidden album: hide and unhide photos, and its lock (`crate::hidden`). The password commands are
//! queries, so a password never lands in the command journal.

use lightcraft_catalog::{Op, PhotoId};
use serde_json::json;

use super::library::select_after_deletion;
use super::{CommandSpec, always, bool_or, cmd, has_selection, str_param};
use crate::{Result, Session};

/// The selection holds a photo that isn't hidden.
fn can_hide(s: &Session) -> std::result::Result<(), String> {
    has_selection(s)?;
    if s.selected().iter().any(|id| s.catalog.photo(*id).is_some_and(|p| !p.hidden)) { Ok(()) } else { Err("the photos are hidden already".into()) }
}

/// The selection holds a hidden photo.
fn can_unhide(s: &Session) -> std::result::Result<(), String> {
    has_selection(s)?;
    if s.selected().iter().any(|id| s.catalog.photo(*id).is_some_and(|p| p.hidden)) { Ok(()) } else { Err("no hidden photo selected".into()) }
}

/// Hide (`true`) or show `ids` as one undo step; the photos leave the view, the selection moves on.
pub(crate) fn set_hidden(s: &mut Session, ids: &[PhotoId], hidden: bool) -> Result<usize> {
    if hidden {
        crate::cloud_sync::check_server_keeps_hidden(s)?;
    }
    let ops: Vec<Op> =
        ids.iter().filter(|id| s.catalog.photo(**id).is_some_and(|p| p.hidden != hidden)).map(|id| Op::SetHidden { id: *id, hidden }).collect();
    let n = ops.len();
    if n > 0 {
        let before = s.visible_cloned();
        s.commit(if hidden { "Hide" } else { "Unhide" }, Op::Batch { ops })?;
        select_after_deletion(s, &before);
    }
    Ok(n)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "photo.markSensitive",
            "Mark as Sensitive",
            ["Photo"],
            None,
            "{ids?} — the photos count as possibly sensitive whatever the model scored (listed under Possibly Sensitive, blurred in the grid); saved and synced with the library → {changed}",
            always,
            |s, p| {
                let ids = s.targets(p)?;
                Ok(json!({"changed": s.mark_sensitive(&ids, true)?}))
            }
        ),
        cmd!(
            "photo.unmarkSensitive",
            "Mark as Not Sensitive",
            ["Photo"],
            None,
            "{ids?} — the photos no longer count as possibly sensitive, whatever the model scored (not listed, not blurred); saved and synced with the library → {changed}",
            always,
            |s, p| {
                let ids = s.targets(p)?;
                Ok(json!({"changed": s.mark_sensitive(&ids, false)?}))
            }
        ),
        cmd!(
            "photo.hide",
            "Hide Photo",
            ["Photo"],
            Some("Cmd+L"),
            "{ids?} — into the Hidden album: left out of All Photos, albums, search and People until Hidden is unlocked",
            can_hide,
            |s, p| {
                let ids = s.targets(p)?;
                Ok(json!({"changed": set_hidden(s, &ids, true)?}))
            }
        ),
        cmd!("photo.unhide", "Unhide Photo", ["Photo"], Some("Cmd+Shift+L"), "{ids?} — back with the rest of the library", can_unhide, |s, p| {
            let ids = s.targets(p)?;
            Ok(json!({"changed": set_hidden(s, &ids, false)?}))
        }),
        cmd!(query "hidden.status", "Hidden Status", [], None, "{} — {locked, hasPassword, showHidden, count (while unlocked)}", always, |s, _| Ok(s.hidden_status())),
        cmd!(query "hidden.unlock", "Unlock Hidden", [], None, "{password} — the library's Hidden password (error kind `locked` when wrong; five wrong ones make it wait 30 s)", always, |s, p| {
            s.hidden_unlock(str_param(p, "password").unwrap_or(""))?;
            Ok(s.hidden_status())
        }),
        cmd!(query "hidden.lock", "Lock Hidden", [], None, "{} — hidden photos leave every view; the Hidden album shows All Photos", always, |s, _| {
            s.hidden_lock();
            Ok(s.hidden_status())
        }),
        cmd!(query "hidden.setPassword", "Set Hidden Password", [], None, "{password, current?} — at least 4 characters; changing it needs the current one (or Hidden unlocked). Synced with the library", always, |s, p| {
            let password = str_param(p, "password").unwrap_or("");
            s.hidden_set_password(password, str_param(p, "current"))?;
            Ok(s.hidden_status())
        }),
        cmd!(query "library.showHidden", "Show Hidden", [], None, "{on} — hidden photos shown with the rest of every view (needs Hidden unlocked; error kind `locked`)", always, |s, p| {
            s.hidden_show(bool_or(p, "on", true))?;
            Ok(json!({"count": s.visible().len(), "hidden": s.hidden_status()}))
        }),
    ]
}
