import Foundation

// The engine host's messages (crates/apphost/src/host/protocol.rs). LightCraftCoreTests decodes
// Fixtures/protocol.json, which a Rust test writes from the same types: keep the two in step.

/// A request: `{"id", "method", "params"}`.
public struct Request: Codable, Sendable {
    public var id: UInt64
    public var method: String
    public var params: JSONValue

    public init(id: UInt64, method: String, params: JSONValue) {
        self.id = id
        self.method = method
        self.params = params
    }
}

/// How to start the engine (`lc_open`). Unknown fields are refused by the engine.
public struct OpenConfig: Codable, Sendable {
    /// The app's home folder: the library goes in `Documents/`, models in `Library/Caches/`.
    public var home: String?
    /// The app's tmp folder: staging, the automation script and its replies.
    public var tmp: String?
    /// A library folder instead of the one in `home`.
    public var library: String?
    /// The in-memory demo library (nothing saved).
    public var demo: Bool = false
    public var deviceName: String?
    /// A control-protocol script to run at start (`LIGHTCRAFT_SCRIPT`), relative to `tmp`.
    public var script: String?
    public var renderThreads: Int?
    public var availableMemory: UInt64?

    public init(home: String? = nil, tmp: String? = nil, library: String? = nil, demo: Bool = false, deviceName: String? = nil, script: String? = nil, renderThreads: Int? = nil, availableMemory: UInt64? = nil) {
        self.home = home
        self.tmp = tmp
        self.library = library
        self.demo = demo
        self.deviceName = deviceName
        self.script = script
        self.renderThreads = renderThreads
        self.availableMemory = availableMemory
    }
}

/// What `lc_open` opened.
public struct Opened: Codable, Sendable {
    public var engine: UInt64
    public var library: String?
    public var photos: Int
    public var demo: Bool
    public var warnings: [String]
}

/// An error answer. `code`: the engine's error kind (`unknownCommand`, `disabled`, `badParams`,
/// `notSaved`…) or the host's (`badRequest`, `unknownMethod`, `panic`, `closed`, `timeout`).
public struct ErrorBody: Codable, Sendable, Error, Equatable, LocalizedError {
    public var code: String
    public var message: String

    public init(code: String, message: String) {
        self.code = code
        self.message = message
    }

    public var errorDescription: String? { message }
}

/// What the library and the view look like (a `state` event when any of it changes).
public struct HostState: Codable, Sendable, Equatable {
    public var catalogRevision: UInt64 = 0
    public var visibleGen: UInt64 = 0
    public var visibleCount: Int = 0
    public var libraryGeneration: UInt64 = 0
    /// Changes with the preset list (created, imported, renamed, or brought by the sync).
    public var presetsGeneration: UInt64 = 0
    public var active: UInt64?
    public var selectionCount: Int = 0
    public var undo: String?
    public var redo: String?
    /// A slider drag in progress (its label).
    public var interaction: String?
    /// The Masking tool's selected mask and the Remove tool's selected spot.
    public var activeMask: UInt32?
    public var activeSpot: Int?
    public var unsaved: Int = 0
    /// The server sync (`nil`: a local library with no server).
    public var sync: SyncSummary?
    /// Face recognition's background scan while it is on.
    public var faces: FaceScan?

    public init() {}
}

/// How far face recognition's background scan is.
public struct FaceScan: Codable, Sendable, Equatable {
    public var pendingPhotos: Int
    public var indexedFaces: Int
}

/// The server sync as the cloud icon shows it (`sync.status` has the rest).
public struct SyncSummary: Codable, Sendable, Equatable {
    /// `signedOut`, `conflict`, `paused`, `error`, `syncing` or `idle`.
    public var state: String
    public var server: String
    public var pending: Int = 0
    public var uploads: Int = 0
    public var downloads: Int = 0
    /// While the server's library is read: (photos read, photos it has).
    public var reading: [Int]?
    public var error: String?
}

/// A new picture for a slot (`thumb:42`, `thumbQuick:42`, `main`, `preview`, `before`…).
public struct ImageEvent: Codable, Sendable, Equatable {
    public var slot: String
    public var photo: UInt64
    public var key: UInt64
    public var token: UInt64
    public var width: Int
    public var height: Int
    /// False for a stand-in or a draft.
    public var isFinal: Bool
    public var draft: Bool
    /// Where a stand-in came from: `cached`, `embedded`, `small`.
    public var quick: String?
    public var ms: Double
    public var histogram: Histogram?
    /// For `region` (a zoom window): the zoomed frame it is cut from and where it goes in it.
    public var window: ZoomWindow?
    /// The same picture as float pixels (`render.hdrDisplay`, an HDR edit): its own lease.
    public var hdr: HdrRef?

    enum CodingKeys: String, CodingKey {
        case slot, photo, key, token, width, height, draft, quick, ms, histogram, window, hdr
        case isFinal = "final"
    }
}

/// Where an image event's HDR rendition is: a lease of RGBA16F pixels (`PixelImage.hdr`).
public struct HdrRef: Codable, Sendable, Equatable {
    public var token: UInt64
    public var width: Int
    public var height: Int
    /// The edit's headroom limit: stops above SDR white are `log2(peak)`.
    public var peak: Double
}

/// Where a zoom window goes: `full` is the zoomed frame's size in pixels, `x, y, w, h` the window
/// in it.
public struct ZoomWindow: Codable, Sendable, Equatable {
    public var full: [Int]
    public var x: Int
    public var y: Int
    public var w: Int
    public var h: Int
}

/// 256 bins per channel.
public struct Histogram: Codable, Sendable, Equatable {
    public var r: [UInt32]
    public var g: [UInt32]
    public var b: [UInt32]
    public var luma: [UInt32]
    public var total: UInt32
    /// The share of pixels clipped to black and to white (the clipping warnings).
    public var clipping: [Double]?
}

/// A background task (import, export, sync…), as `activity.list` gives it.
public struct ActivityTask: Codable, Sendable, Equatable, Identifiable {
    public var id: UInt64
    public var kind: String
    public var label: String
    public var done: UInt64
    public var total: UInt64
    public var unit: String
    public var detail: String
    public var cancellable: Bool
    public var cancelling: Bool
    public var ageMs: UInt64
}

/// An event from the engine.
public enum EngineEvent: Sendable {
    case reply(id: UInt64, result: Result<JSONValue, ErrorBody>)
    /// A picture and its pixels (`nil` when they were dropped before they could be read: a newer
    /// one for the slot is on its way).
    case image(ImageEvent, PixelImage?)
    case renderFailed(slot: String, photo: UInt64, error: String)
    case state(HostState)
    case activity([ActivityTask])
    case message(text: String, level: String)
    /// An automation script asks the UI: answer with `ui.reply {id, result | error}`.
    case uiRequest(id: UInt64, method: String, params: JSONValue)
    case closed
    /// Put `text` on the clipboard (a share link just made).
    case copy(String)
    /// Show a dialog the engine needs an answer from (`syncChoice`).
    case ask(dialog: String, params: JSONValue)
    /// A kind this build doesn't know (a newer engine).
    case unknown(kind: String)
}

/// One event as it arrives, before its pixels are fetched. Never fails to decode: a malformed or
/// unknown event becomes `.unknown`.
struct WireEvent: Decodable {
    let event: EngineEvent

    enum Keys: String, CodingKey {
        case kind, id, result, error, slot, photo, state, tasks, text, level, method, params, dialog
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        let kind = (try? c.decode(String.self, forKey: .kind)) ?? ""
        event = (try? Self.decode(kind, c, decoder)) ?? .unknown(kind: kind)
    }

    private static func decode(_ kind: String, _ c: KeyedDecodingContainer<Keys>, _ decoder: any Decoder) throws -> EngineEvent {
        switch kind {
        case "reply":
            let id = try c.decode(UInt64.self, forKey: .id)
            if let error = try c.decodeIfPresent(ErrorBody.self, forKey: .error) {
                return .reply(id: id, result: .failure(error))
            }
            return .reply(id: id, result: .success(try c.decodeIfPresent(JSONValue.self, forKey: .result) ?? .null))
        case "image":
            return .image(try ImageEvent(from: decoder), nil)
        case "renderFailed":
            return .renderFailed(slot: try c.decode(String.self, forKey: .slot), photo: try c.decode(UInt64.self, forKey: .photo), error: try c.decode(String.self, forKey: .error))
        case "state":
            return .state(try c.decode(HostState.self, forKey: .state))
        case "activity":
            return .activity(try c.decode([ActivityTask].self, forKey: .tasks))
        case "message":
            return .message(text: try c.decode(String.self, forKey: .text), level: try c.decode(String.self, forKey: .level))
        case "uiRequest":
            return .uiRequest(id: try c.decode(UInt64.self, forKey: .id), method: try c.decode(String.self, forKey: .method), params: try c.decodeIfPresent(JSONValue.self, forKey: .params) ?? .null)
        case "closed":
            return .closed
        case "copy":
            return .copy(try c.decode(String.self, forKey: .text))
        case "ask":
            return .ask(dialog: try c.decode(String.self, forKey: .dialog), params: try c.decodeIfPresent(JSONValue.self, forKey: .params) ?? .null)
        default:
            return .unknown(kind: kind)
        }
    }
}
