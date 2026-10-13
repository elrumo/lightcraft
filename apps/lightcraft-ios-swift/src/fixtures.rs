//! The protocol's shapes as the Swift tests read them: one of every event and request kind, written
//! here (`LIGHTCRAFT_BLESS=1 cargo test -p lightcraft-ios-swift fixtures` rewrites the file) and
//! decoded by `LightCraftCoreTests`, so a change on one side that the other doesn't follow fails.

use lightcraft_apphost::host::{ErrorBody, Event, HostState, OpenConfig, Request};
use lightcraft_engine::activity::{TaskInfo, Unit};
use lightcraft_engine::catalog::PhotoId;
use serde_json::{Value, json};

const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/LightCraft/Tests/LightCraftCoreTests/Fixtures/protocol.json");

fn fixtures() -> Value {
    let task = TaskInfo {
        id: 3,
        kind: "export",
        label: "Exporting 2 photos".into(),
        done: 1,
        total: 2,
        unit: Unit::Count,
        detail: "IMG_0001.jpg".into(),
        cancellable: true,
        cancelling: false,
        age_ms: 1200,
    };
    let state = HostState {
        catalog_revision: 12,
        visible_gen: 40,
        visible_count: 30,
        library_generation: 2,
        active: Some(PhotoId(7)),
        selection_count: 1,
        undo: Some("Exposure".into()),
        redo: None,
        interaction: None,
        unsaved: 0,
        sync: Some(
            json!({"state": "syncing", "server": "https://photos.example", "pending": 2, "uploads": 1, "downloads": 0, "reading": null, "error": null}),
        ),
    };
    let events = vec![
        Event::ok(1, json!({"photos": [], "total": 0})),
        Event::err(2, ErrorBody::new("unknownCommand", "unknown command `no.such`")),
        Event::Image {
            slot: "main".into(),
            photo: PhotoId(7),
            key: 18_446_744_073_709_551_000,
            token: 99,
            width: 1600,
            height: 1067,
            is_final: false,
            draft: true,
            quick: None,
            ms: 12.5,
            histogram: Some(json!({"r": [0], "g": [0], "b": [0], "luma": [0], "total": 1})),
        },
        Event::Image {
            slot: "thumb:7".into(),
            photo: PhotoId(7),
            key: 5,
            token: 100,
            width: 256,
            height: 171,
            is_final: false,
            draft: false,
            quick: Some("embedded".into()),
            ms: 3.0,
            histogram: None,
        },
        Event::RenderFailed { slot: "thumb:8".into(), photo: PhotoId(8), key: 6, error: "the file is missing".into() },
        Event::State { state },
        Event::Activity { tasks: vec![task] },
        Event::Message { text: "Saved".into(), level: "info".into() },
        Event::UiRequest { id: 4, method: "ui.screenshot".into(), params: json!({"path": "/tmp/shot.png"}) },
        Event::Closed,
        Event::Copy { text: "https://photos.example/s/abc".into() },
        Event::Ask { dialog: "syncChoice".into(), params: json!({}) },
    ];
    let request = Request { id: 5, method: "render.view".into(), params: json!({"photo": 7, "width": 1600, "height": 1067, "draft": true}) };
    let config =
        OpenConfig { home: Some("/home".into()), tmp: Some("/tmp".into()), demo: true, device_name: Some("iPhone".into()), ..Default::default() };
    json!({
        "events": events.iter().map(Event::to_value).collect::<Vec<_>>(),
        "request": request,
        "openConfig": config,
    })
}

#[test]
fn fixtures_match_the_protocol() {
    let want = serde_json::to_string_pretty(&fixtures()).unwrap() + "\n";
    if std::env::var_os("LIGHTCRAFT_BLESS").is_some() {
        std::fs::write(PATH, &want).unwrap();
    }
    let have = std::fs::read_to_string(PATH).unwrap_or_default();
    assert!(
        have == want,
        "{PATH} is out of date: run `LIGHTCRAFT_BLESS=1 cargo test -p lightcraft-ios-swift fixtures` and update the Swift side to match"
    );
}
