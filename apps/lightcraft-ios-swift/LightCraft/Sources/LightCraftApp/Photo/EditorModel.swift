#if os(iOS)
import Foundation
import LightCraftCore
import Observation

/// The tools under a photo, as Lightroom's mobile app has them.
nonisolated enum Tool: String, CaseIterable, Identifiable {
    case presets, crop, edit, masking, remove, info, redEye
    var id: String { rawValue }

    /// The tool bar's tools (Red Eye opens from the photo's "…" menu).
    static let bar: [Tool] = [.presets, .crop, .edit, .masking, .remove, .info]

    var label: String {
        switch self {
        case .presets: "Presets"
        case .crop: "Crop"
        case .edit: "Edit"
        case .masking: "Masking"
        case .remove: "Remove"
        case .info: "Info"
        case .redEye: "Red Eye"
        }
    }

    var symbol: String {
        switch self {
        case .presets: "circle.lefthalf.filled.righthalf.striped.horizontal"
        case .crop: "crop.rotate"
        case .edit: "slider.horizontal.3"
        case .masking: "circle.dashed.inset.filled"
        case .remove: "eraser"
        case .info: "info.circle"
        case .redEye: "eye"
        }
    }

    /// Crop, Masking, Remove and Red Eye take the whole screen (their own bars).
    var hasScreen: Bool { self == .crop || self == .masking || self == .remove || self == .redEye }

    /// The navigation bar's title while its screen is open.
    var screenTitle: LocalizedStringResource {
        switch self {
        case .crop: "Crop & Rotate"
        case .masking: "Masking"
        case .remove: "Remove"
        case .redEye: "Red Eye"
        case .presets: "Presets"
        case .edit: "Edit"
        case .info: "Info"
        }
    }
}

/// The Edit tool's groups, one shown at a time (the desktop's sections).
nonisolated enum EditGroup: String, CaseIterable, Identifiable {
    case profile, light, color, effects, detail, optics, calibration
    var id: String { rawValue }

    var label: String {
        switch self {
        case .profile: "Profile"
        case .light: "Light"
        case .color: "Color"
        case .effects: "Effects"
        case .detail: "Detail"
        case .optics: "Optics"
        case .calibration: "Calibration"
        }
    }

    var symbol: String {
        switch self {
        case .profile: "square.grid.2x2"
        case .light: "sun.max"
        case .color: "drop"
        case .effects: "circle.circle"
        case .detail: "triangle"
        case .optics: "camera.aperture"
        case .calibration: "scope"
        }
    }

    /// The engine sections whose sliders it shows.
    var sections: [String] {
        switch self {
        case .profile: ["profile"]
        case .light: ["light"]
        case .color: ["color"]
        case .effects: ["effects", "vignette", "grain"]
        case .detail: ["detail"]
        case .optics: ["optics"]
        case .calibration: ["calibration"]
        }
    }
}

/// What the photo screen's tools show and do: the open tool and group, the sliders of the open
/// group with the active photo's values, and the active photo's full develop settings (curves,
/// grading, white balance mode…). A slider drag goes to the engine as one interaction (one undo
/// step), drafts rendering while it moves.
@Observable
final class EditorModel {
    /// The open tool (`nil`: the photo alone).
    var tool: Tool?
    /// The undo stack's depth when a tool screen opened (its Cancel goes back there).
    var screenBase: Int?
    /// The bottom panel's height step (0: short … 2: tall).
    var detent = 1
    /// Sliders by engine section (`light`, `color`, `vignette`…).
    private(set) var controls: [String: [ControlSpec]] = [:]
    /// The active photo's develop settings (`develop.get`, snake_case).
    private(set) var settings: JSONValue = .null
    /// The photo they are of.
    private(set) var photo: UInt64?
    /// Values moved here and not yet read back from the engine.
    private var local: [String: Double] = [:]
    /// The control being dragged.
    private(set) var dragging: String?
    private var dirty: String?
    private var inFlight = false
    /// A tap on the photo picks something for a tool (white balance, a point colour).
    var picking: Pick?
    /// The Remove tool: its mode and brush (size as a fraction of the photo's long edge).
    var removeMode = "remove"
    var removeSize = 0.02
    var removeFeather = 50.0
    var removeOpacity = 100.0
    /// Remove's Visualize Spots: the photo as an edge map where dust stands out, at this threshold.
    var visualizeSpots = false
    var spotsThreshold = 50.0
    /// The AI Remove model is here and turned on (`models.list`, id `migan`).
    var eraseReady = false
    /// The Masking tool's tool in hand on the photo (`brush`, `colorRange`) and the brush.
    var maskTool: String?
    var brushSize = 0.03
    var brushFeather = 50.0
    var brushFlow = 100.0
    var brushErase = false
    var brushAutoMask = false
    /// Masking: every visible mask's region over the photo at once, not just the selected one's.
    var maskOverlayAll = false
    /// Before / After: the photo as it was beside it as it is.
    var beforeAfter = false
    /// Tag a Face: the next tap on the photo marks a face there.
    var tagFace = false
    /// Crop & Rotate's line tools: `straighten` (one line, then back to the frame) or `guides`
    /// (Guided Upright, until Done).
    var cropDraw: String?
    /// Reference View: the reference photo beside the one being edited.
    var referenceView = false
    /// Soft proofing: the photo as `proofSpace` (srgb, displayP3, adobeRgb, proPhoto, rec2020) holds
    /// it, with what it can't marked when `proofWarning`.
    var softProof = false
    var proofSpace = "srgb"
    var proofWarning = false
    /// Light ▸ HDR ▸ Visualize: what the HDR rendition puts above SDR white, painted by how far.
    var visualizeHdr = false
    /// Red Eye: a pet's eyes, and the correction chosen (its sliders).
    var eyePet = false
    var eye = 0

    enum Pick {
        case whiteBalance, pointColor
    }

    /// Sections wanted (the open group's, plus flyouts).
    var wanted: Set<String> = ["light"]

    /// Read the open group's sliders and the settings again (the photo changed, an edit landed).
    func reload(_ app: AppModel) async {
        guard dragging == nil, let engine = app.engine else { return }
        let active = app.state.active
        var next: [String: [ControlSpec]] = [:]
        for section in wanted {
            if let specs = try? await engine.controls(section: section) {
                next[section] = specs
            }
        }
        let got = (try? await engine.execute("develop.get")) ?? .null
        guard dragging == nil else { return }
        controls = next
        settings = got
        photo = active
        local = [:]
    }

    /// Show `sections` too (a flyout opened), reading them now.
    func want(_ sections: [String], _ app: AppModel) {
        let new = Set(sections).subtracting(wanted)
        guard !new.isEmpty else { return }
        wanted.formUnion(new)
        Task { await reload(app) }
    }

    func specs(_ section: String) -> [ControlSpec] {
        controls[section] ?? []
    }

    func spec(_ id: String) -> ControlSpec? {
        controls.values.lazy.flatMap { $0 }.first { $0.id == id }
    }

    func value(_ spec: ControlSpec) -> Double {
        local[spec.id] ?? spec.value ?? spec.default
    }

    /// A slider was grabbed (`true`) or let go.
    func editing(_ spec: ControlSpec, _ began: Bool, _ app: AppModel) {
        guard let engine = app.engine else { return }
        if began {
            dragging = spec.id
            engine.send("engine.execute", ["command": "develop.beginInteraction", "params": ["label": .string(spec.label)]])
        } else {
            // requests run in order: the last value, then the end of the drag (one undo step)
            engine.send("engine.execute", ["command": "develop.set", "params": ["control": .string(spec.id), "value": .double(value(spec))]])
            engine.send("engine.execute", ["command": "develop.endInteraction"])
            dragging = nil
            dirty = nil
        }
    }

    /// A slider moved to `v`.
    func set(_ spec: ControlSpec, _ v: Double, _ app: AppModel) {
        local[spec.id] = v
        guard dragging == spec.id else {
            // a tap on the track (no drag): one step of its own
            Task { await app.run("develop.set", ["control": .string(spec.id), "value": .double(v)]) }
            return
        }
        dirty = spec.id
        pump(app)
    }

    /// Back to its default (a double tap).
    func reset(_ spec: ControlSpec, _ app: AppModel) {
        local[spec.id] = spec.default
        Task { await app.run("develop.set", ["control": .string(spec.id), "value": .double(spec.default)]) }
    }

    /// Several controls at once as one drag step (a grading wheel moves hue and saturation).
    func setMany(_ values: [String: Double], label: String, began: Bool, ended: Bool, _ app: AppModel) {
        guard let engine = app.engine else { return }
        for (k, v) in values { local[k] = v }
        if began {
            dragging = values.keys.first
            engine.send("engine.execute", ["command": "develop.beginInteraction", "params": ["label": .string(label)]])
        }
        var obj: [String: JSONValue] = [:]
        for (k, v) in values { obj[k] = .double(v) }
        engine.send("engine.execute", ["command": "develop.set", "params": ["values": .object(obj)]])
        if ended {
            engine.send("engine.execute", ["command": "develop.endInteraction"])
            dragging = nil
        } else {
            app.requestDraft()
        }
    }

    /// Send the newest value if nothing is in flight; a draft render follows each.
    private func pump(_ app: AppModel) {
        guard !inFlight, let id = dirty, let engine = app.engine else { return }
        inFlight = true
        dirty = nil
        let v = local[id] ?? 0
        Task {
            _ = try? await engine.execute("develop.set", ["control": .string(id), "value": .double(v)])
            inFlight = false
            if dragging != nil {
                app.requestDraft()
                pump(app)
            }
        }
    }

    /// A value from the settings by a snake_case path (`["wb", "mode"]`).
    func setting(_ path: String...) -> JSONValue {
        path.reduce(settings) { $0[$1] }
    }

    /// The photo is black and white (`treatment`).
    var isBW: Bool { setting("treatment").string == "bw" }
    var hdr: Bool { setting("hdr", "enabled").bool == true }
}
#endif
