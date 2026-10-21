//! What was found in the photos that travel ([`super::photos`], the Photos library's): their faces, the words in
//! them, their description vectors and sensitive-content scores, one `analysis` record per photo (by its content key,
//! beside its `edit` record). The parts themselves, and how a device keeps another's, are the vision module's
//! (`crate::vision::portable`); here is how the records move.
//!
//! - **Only filling in.** A device sends a photo's record when it has a part the cloud's record lacks (the cloud has
//!   none, or fewer parts), and takes the parts it lacks from one that comes. What it found itself always stays, so
//!   two devices that both worked a photo out never send it back and forth.
//! - **Never deleted for a photo that still travels**: a device that has nothing of a photo yet leaves the cloud's
//!   record for the others. It goes with the photo's `edit` record.
//! - **Rechecked** when the search indexes grow (a photo worked out here) and for photos that start to travel.

use std::collections::HashSet;

use super::{EDIT, ICloud, Mine, record_name};
use crate::Session;

/// What was found in a photo.
pub const ANALYSIS: &str = "analysis";

/// Bring this device's analysis records up to date with the photos that travel (`ic` is out of the session).
pub(super) fn refresh(s: &Session, ic: &mut ICloud) {
    let keys: Vec<String> = ic.ours.values().filter(|m| m.kind == EDIT).map(|m| m.key.clone()).collect();
    let names: HashSet<String> = keys.iter().map(|k| record_name(ANALYSIS, k)).collect();
    ic.ours.retain(|n, m| m.kind != ANALYSIS || names.contains(n));
    let sizes = s.analysis_sizes();
    let all = ic.analysis_seen != Some(sizes);
    // (a photo with nothing found yet is looked at again each time: a few lookups in each index)
    let check: Vec<String> = keys.into_iter().filter(|k| all || !ic.ours.contains_key(&record_name(ANALYSIS, k))).collect();
    if !check.is_empty() {
        for (key, parts) in s.analysis_parts(&check) {
            let name = record_name(ANALYSIS, &key);
            if parts == 0 {
                ic.ours.remove(&name);
            } else {
                ic.ours.insert(name, Mine { kind: ANALYSIS.to_string(), key, hash: format!("parts:{parts}"), parts });
            }
        }
    }
    ic.analysis_seen = Some(sizes);
}
