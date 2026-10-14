//! Face recognition's background scan (`faces.pump`), paced by what the user is doing so it never
//! gets in the way: shared by the egui app's frame loop and the engine host's tick.

use serde_json::Value;

/// The pace for an app that is in front (or not) and minimized / in the background (or not),
/// `worked` seconds after the user last dragged, typed or scrolled and `moved` seconds after they
/// last moved the pointer or touched, while they are (or are not) looking at the scan's progress:
/// paused while they drag, type or scroll; one photo at a time while they are around or the app is
/// away; half the machine once they have been idle for a few seconds, or while another app has the
/// keyboard but this one is still on screen; most of it while they watch the progress.
pub fn pace_for(focused: bool, minimized: bool, worked: f64, moved: f64, watching: bool) -> &'static str {
    if minimized {
        "light"
    } else if !focused {
        // still on screen, but the user is working in another app: no input here to get in the way of
        "normal"
    } else if worked < 0.4 {
        "pause"
    } else if moved < 3.0 {
        "light"
    } else if watching {
        "full"
    } else {
        "normal"
    }
}

/// Seconds until the next `faces.pump`, from its answer: soon while photos wait or are being read
/// (the next ones for the workers), every second while recognition is on with nothing to do, and
/// on every loop while it is off (cheap: whatever turns it on is seen at once).
pub fn next_pump(answer: &Value) -> f64 {
    let active = answer["active"] == true;
    let busy = active && (answer["pendingPhotos"].as_u64().unwrap_or(0) > 0 || answer["inFlight"].as_u64().unwrap_or(0) > 0);
    if busy {
        0.05
    } else if active {
        1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_scan_keeps_out_of_the_way() {
        assert_eq!(pace_for(true, false, 0.1, 0.1, false), "pause", "while dragging");
        assert_eq!(pace_for(true, false, 2.0, 1.0, false), "light", "the user is around");
        assert_eq!(pace_for(true, false, 9.0, 9.0, false), "normal");
        assert_eq!(pace_for(true, false, 9.0, 9.0, true), "full", "watching the progress");
        assert_eq!(pace_for(false, true, 0.0, 0.0, true), "light", "in the background");
        assert_eq!(pace_for(false, false, 0.0, 0.0, false), "normal", "another app in front");
        assert_eq!(next_pump(&json!({"active": true, "pendingPhotos": 3})), 0.05);
        assert_eq!(next_pump(&json!({"active": true, "pendingPhotos": 0, "inFlight": 0})), 1.0);
        assert_eq!(next_pump(&json!({"active": false})), 0.0);
    }
}
