#if os(iOS)
import LightCraftCore
import SwiftUI

/// The kinds of mask and mask part: their names and symbols.
enum MaskKind {
    /// Create New Mask, as Lightroom's mobile app lists it.
    static let create: [(kind: String, label: String)] = [
        ("subject", "Select Subject"), ("sky", "Select Sky"), ("background", "Select Background"), ("linear", "Linear Gradient"),
        ("radial", "Radial Gradient"), ("luminanceRange", "Luminance Range"), ("colorRange", "Color Range"), ("brush", "Brush"),
    ]
    /// What a mask can be added to, taken away from or intersected with.
    static let parts = ["brush", "linear", "radial", "sky", "subject", "luminanceRange"]

    static func name(_ kind: String) -> String {
        switch kind {
        case "brush": "Brush"
        case "linear": "Linear Gradient"
        case "radial": "Radial Gradient"
        case "colorRange": "Color Range"
        case "luminanceRange": "Luminance Range"
        case "depthRange": "Depth Range"
        case "subject": "Subject"
        case "sky": "Sky"
        case "background": "Background"
        case "object": "Object"
        case "prompt": "Describe"
        case "people": "People"
        case "landscape": "Landscape"
        default: "Mask"
        }
    }

    static func symbol(_ kind: String) -> String {
        switch kind {
        case "brush": "paintbrush.pointed"
        case "linear": "rectangle.tophalf.filled"
        case "radial": "circle.circle"
        case "colorRange": "eyedropper"
        case "luminanceRange": "sun.lefthalf.filled"
        case "depthRange": "square.3.layers.3d"
        case "subject", "object", "prompt": "person.crop.rectangle"
        case "people": "person.2"
        case "sky", "landscape": "cloud.sun"
        case "background": "photo"
        default: "circle.dashed.inset.filled"
        }
    }
}

/// The photo's masks (the develop settings' `masks`) and the selected one.
enum MaskList {
    static func all(_ model: AppModel) -> [JSONValue] {
        model.editor.setting("masks").array ?? []
    }

    static func selected(_ model: AppModel) -> JSONValue? {
        guard let id = model.state.activeMask else { return nil }
        return all(model).first { $0["id"].int == Int64(id) }
    }

    /// The kind of a mask's first part.
    static func kind(_ mask: JSONValue) -> String {
        mask["components"].array?.first?["shape"]["kind"].string ?? ""
    }
}

/// Masking, under the photo: the brush's controls while it is in hand, the open group's sliders for
/// the selected mask (Mask: amount, invert, its parts' ranges, refine edges; then Light, Color,
/// Effects, Detail, Optics), the groups, and ✕ Masking ✓. The masks themselves are in the column
/// over the photo ([`MaskColumn`]).
struct MaskingTools: View {
    @Environment(AppModel.self) private var model
    @State private var group: String?

    var body: some View {
        let mask = MaskList.selected(model)
        VStack(spacing: 8) {
            if model.editor.maskTool == "brush" {
                BrushControls()
            }
            if let mask, let group {
                MaskGroupSheet(mask: mask, group: group)
                    .frame(maxHeight: 230)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            } else if MaskList.all(model).isEmpty {
                Text("Tap + to make a mask: the subject, the sky, a gradient, a brush…")
                    .font(.footnote).foregroundStyle(.secondary)
                    .padding(.horizontal, 16)
            }
            MaskGroupRow(selection: $group, enabled: mask != nil)
        }
        .padding(.vertical, 8)
        .animation(.snappy, value: group)
        .onChange(of: mask == nil) { _, none in
            if none { group = nil }
        }
    }
}

/// Mask, Light, Color, Effects, Detail, Optics: a tap opens the group's sliders, again closes them.
private struct MaskGroupRow: View {
    @Binding var selection: String?
    let enabled: Bool

    static let groups: [(id: String, label: String, symbol: String)] = [
        ("mask", "Mask", "circle.dashed.inset.filled"), ("light", "Light", "sun.max"), ("color", "Color", "drop"),
        ("effects", "Effects", "circle.circle"), ("detail", "Detail", "triangle"), ("optics", "Optics", "camera.aperture"),
    ]
    /// Each group's local adjustments (`mask.controls` ids).
    static let ids: [String: [String]] = [
        "light": ["exposure", "contrast", "highlights", "shadows", "whites", "blacks"],
        "color": ["temp", "tint", "hue", "saturation"],
        "effects": ["texture", "clarity", "dehaze"],
        "detail": ["sharpness", "noise", "moire"],
        "optics": ["defringe"],
    ]

    var body: some View {
        GroupChips(
            items: Self.groups.map { .init(id: $0.id, label: $0.label, symbol: $0.symbol) },
            selected: selection,
            enabled: enabled,
            choose: { selection = selection == $0 ? nil : $0 }
        )
        .opacity(enabled ? 1 : 0.4)
        .haptic(.selection, trigger: selection)
    }
}

/// The open group's sliders for the selected mask.
private struct MaskGroupSheet: View {
    let mask: JSONValue
    let group: String

    var body: some View {
        ScrollView {
            if group == "mask" {
                MaskSettings(mask: mask)
            } else {
                MaskAdjustments(mask: mask["id"], ids: MaskGroupRow.ids[group] ?? [])
            }
        }
        .scrollIndicators(.hidden)
    }
}

/// The Mask group: Amount, Invert, each part's own controls (a luminance range, a colour range's
/// refine, an AI selection's edge: `mask.shapeControls`), Refine Edges, the overlay's options.
private struct MaskSettings: View {
    @Environment(AppModel.self) private var model
    let mask: JSONValue
    @State private var shape: ShapeControls?
    @State private var overlayOptions = false

    var body: some View {
        @Bindable var prefs = model.prefs
        VStack(alignment: .leading, spacing: 6) {
            MaskAdjustments(mask: mask["id"], ids: ["amount"])
            Toggle("Invert", isOn: Binding(get: { mask["invert"].bool == true }, set: { _ in model.perform("mask.invert", ["id": mask["id"]]) }))
                .padding(.horizontal, 16)
            ForEach(shape?.components ?? [], id: \.component) { part in
                if !part.controls.isEmpty {
                    Text(LocalizedStringKey(MaskKind.name(part.kind)))
                        .font(.footnote.weight(.semibold)).foregroundStyle(.secondary)
                        .padding(.horizontal, 16).padding(.top, 6)
                    ForEach(part.controls) { spec in
                        ShapeSlider(mask: mask["id"], component: part.component, spec: spec)
                    }
                    if part.kind == "luminanceRange" {
                        // the photo in black & white with the range tinted
                        Toggle("Show Luminance Map", isOn: Binding(get: { prefs.maskOverlay && prefs.maskOverlayMode == "colorOnBw" }, set: { on in
                            prefs.maskOverlayMode = on ? "colorOnBw" : "color"
                            if on { prefs.maskOverlay = true }
                        }))
                        .padding(.horizontal, 16)
                    }
                }
            }
            if let refine = shape?.refine {
                ShapeSlider(mask: mask["id"], component: nil, spec: refine)
            }
            Button("Overlay Options", systemImage: "eye.circle") { overlayOptions = true }
                .padding(.horizontal, 16).padding(.vertical, 6)
        }
        .sheet(isPresented: $overlayOptions) { MaskOverlayOptions().presentationDetents([.medium]) }
        .task(id: Key(mask: mask["id"], revision: model.state.catalogRevision)) {
            guard let engine = model.engine else { return }
            shape = try? await engine.execute("mask.shapeControls", ["id": mask["id"]], as: ShapeControls.self)
        }
    }

    private nonisolated struct Key: Hashable {
        let mask: JSONValue
        let revision: UInt64
    }
}

/// What `mask.shapeControls` gives: Refine Edges, and each part's own controls.
private nonisolated struct ShapeControls: Decodable {
    var refine: ControlSpec?
    var components: [Part]

    nonisolated struct Part: Decodable {
        var component: Int
        var kind: String
        var controls: [ControlSpec]
    }
}

/// One of a mask's own controls (`mask.setShapeControl`): of a part, or of the mask (Refine Edges).
private struct ShapeSlider: View {
    @Environment(AppModel.self) private var model
    let mask: JSONValue
    let component: Int?
    let spec: ControlSpec
    @State private var local: Double?

    var body: some View {
        ControlSlider(
            spec: spec,
            value: local ?? spec.value ?? spec.default,
            set: { v in
                local = v
                model.engine?.send("engine.execute", ["command": "mask.setShapeControl", "params": params(v)])
                model.requestDraft()
            },
            editing: { began in
                model.engine?.send("engine.execute", ["command": began ? "develop.beginInteraction" : "develop.endInteraction", "params": ["label": .string(spec.label)]])
            },
            reset: {
                local = spec.default
                model.perform("mask.setShapeControl", params(spec.default))
            }
        )
        .padding(.horizontal, 16)
        .onChange(of: spec.value) { local = nil }
    }

    private func params(_ v: Double) -> JSONValue {
        var p: [String: JSONValue] = ["id": mask, "control": .string(spec.id), "value": .double(v)]
        if let component { p["component"] = .int(Int64(component)) }
        return .object(p)
    }
}

/// The selected mask's adjustments in `ids` (`mask.controls` specs; `mask.adjust` values), as
/// sliders or dials (the Edit tool's style).
struct MaskAdjustments: View {
    @Environment(AppModel.self) private var model
    @Environment(\.toolColumn) private var column
    let mask: JSONValue
    let ids: [String]
    @State private var specs: [ControlSpec] = []
    @State private var local: [String: Double] = [:]

    var body: some View {
        let shown = specs.filter { ids.contains($0.id) }
        Group {
            if model.prefs.dials(column: column) {
                DialAdjuster(dials: shown.map(Dial.control), value: value, set: set, editing: editing, reset: reset)
                    .id(ids)
            } else {
                VStack(alignment: .leading) {
                    ForEach(shown) { spec in
                        ControlSlider(spec: spec, value: value(spec), set: { set(spec, $0) }, editing: { editing(spec, $0) }, reset: { reset(spec) })
                    }
                }
                .padding(.horizontal, 16)
            }
        }
        .task(id: Key(mask: mask, revision: model.state.catalogRevision)) {
            guard let engine = model.engine, let r = try? await engine.execute("mask.controls", ["id": mask], as: [ControlSpec].self) else { return }
            specs = r
            local = [:]
        }
    }

    private func value(_ spec: ControlSpec) -> Double {
        local[spec.id] ?? spec.value ?? spec.default
    }

    private func set(_ spec: ControlSpec, _ v: Double) {
        local[spec.id] = v
        model.engine?.send("engine.execute", ["command": "mask.adjust", "params": ["id": mask, "values": .object([spec.id: .double(v)])]])
        model.requestDraft()
    }

    private func editing(_ spec: ControlSpec, _ began: Bool) {
        model.engine?.send("engine.execute", ["command": began ? "develop.beginInteraction" : "develop.endInteraction", "params": ["label": .string(spec.label)]])
    }

    private func reset(_ spec: ControlSpec) {
        local[spec.id] = spec.default
        model.perform("mask.adjust", ["id": mask, "values": .object([spec.id: .double(spec.default)])])
    }

    private nonisolated struct Key: Hashable {
        let mask: JSONValue
        let revision: UInt64
    }
}

/// The masks over the photo, as Lightroom's mobile app has them: + (Create New Mask), a tile per
/// mask (a tap selects it; on the selected one, its menu), the selected mask's parts (a tap for
/// theirs; a painted part puts the brush in hand) and ± to add to it, take from it or intersect it.
struct MaskColumn: View {
    @Environment(AppModel.self) private var model
    @State private var renaming: Renaming?
    @State private var name = ""

    private struct Renaming {
        let mask: JSONValue
        let component: Int?
    }

    var body: some View {
        ViewThatFits(in: .vertical) {
            column
            ScrollView { column }.scrollIndicators(.hidden)
        }
        .frame(width: 80)
        .glassEffect(.regular, in: .rect(cornerRadius: 24))
        .alert("Rename", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: $name)
            Button("Rename") { rename() }
            Button("Cancel", role: .cancel) {}
        }
        .haptic(.selection, trigger: model.state.activeMask)
    }

    private var column: some View {
        let active = model.state.activeMask.map(Int64.init)
        return VStack(spacing: 10) {
            CreateMaskMenu()
            ForEach(Array(MaskList.all(model).enumerated()), id: \.offset) { _, mask in
                let chosen = mask["id"].int == active
                let tile = MaskTile(symbol: MaskKind.symbol(MaskList.kind(mask)), name: mask["name"].string ?? String(localized: "Mask"),
                                    chosen: chosen, visible: mask["visible"].bool != false, side: 52, badge: nil)
                if chosen {
                    Menu { maskMenu(mask) } label: { tile }
                    parts(mask)
                } else {
                    Button { select(mask) } label: { tile }
                }
            }
        }
        .buttonStyle(.plain)
        .padding(.vertical, 12)
    }

    @ViewBuilder private func parts(_ mask: JSONValue) -> some View {
        let parts = mask["components"].array ?? []
        ForEach(Array(parts.enumerated()), id: \.offset) { k, part in
            let kind = part["shape"]["kind"].string ?? ""
            let op = part["op"].string ?? "add"
            let tile = MaskTile(symbol: MaskKind.symbol(kind), name: part["name"].string ?? String(localized: String.LocalizationValue(MaskKind.name(kind))),
                                chosen: false, visible: true, side: 40, badge: op == "subtract" ? "−" : op == "intersect" ? "∩" : nil)
            if kind == "brush" {
                Menu { partMenu(mask, k, part, count: parts.count) } label: { tile } primaryAction: {
                    model.editor.maskTool = "brush"
                    model.editor.brushErase = false
                }
            } else {
                Menu { partMenu(mask, k, part, count: parts.count) } label: { tile }
            }
        }
        CombineMenu()
    }

    @ViewBuilder private func maskMenu(_ mask: JSONValue) -> some View {
        let id = mask["id"]
        let visible = mask["visible"].bool != false
        Button(visible ? "Hide Mask" : "Show Mask", systemImage: visible ? "eye.slash" : "eye") { model.perform("mask.visible", ["id": id]) }
        Button("Invert Mask", systemImage: "circle.righthalf.filled") { model.perform("mask.invert", ["id": id]) }
        Button("Duplicate Mask", systemImage: "plus.square.on.square") { model.perform("mask.duplicate", ["id": id]) }
        Button("Duplicate and Invert Mask", systemImage: "square.on.square.intersection.dashed") { model.perform("mask.duplicate", ["id": id, "invert": true]) }
        Button("Rename…", systemImage: "pencil") {
            name = mask["name"].string ?? ""
            renaming = Renaming(mask: id, component: nil)
        }
        Divider()
        Button("Delete Mask", systemImage: "trash", role: .destructive) {
            model.perform("mask.delete", ["id": id])
            model.buzz(.warning)
            model.editor.maskTool = nil
        }
    }

    @ViewBuilder private func partMenu(_ mask: JSONValue, _ k: Int, _ part: JSONValue, count: Int) -> some View {
        let run = { (p: [String: JSONValue]) in
            var p = p
            p["id"] = mask["id"]
            p["component"] = .int(Int64(k))
            model.perform("mask.component", .object(p))
        }
        Button("Invert", systemImage: "circle.righthalf.filled") { run(["action": "invert"]) }
        Button("Duplicate", systemImage: "plus.square.on.square") { run(["action": "duplicate"]) }
        if k > 0 {
            Picker("Mode", selection: Binding(get: { part["op"].string ?? "add" }, set: { run(["action": "op", "op": .string($0)]) })) {
                Text("Add").tag("add")
                Text("Subtract").tag("subtract")
                Text("Intersect").tag("intersect")
            }
        }
        Button("Rename…", systemImage: "pencil") {
            name = part["name"].string ?? ""
            renaming = Renaming(mask: mask["id"], component: k)
        }
        Divider()
        Button(count == 1 ? "Delete (and the Mask)" : "Delete", systemImage: "trash", role: .destructive) {
            run(["action": "delete"])
            model.buzz(.warning)
            model.editor.maskTool = nil
        }
    }

    private func select(_ mask: JSONValue) {
        model.perform("mask.select", ["id": mask["id"]])
        // the brush stays in hand for a mask that is painted
        let painted = (mask["components"].array ?? []).contains { $0["shape"]["kind"] == "brush" }
        if !painted || model.editor.maskTool != "brush" {
            model.editor.maskTool = nil
        }
    }

    private func rename() {
        guard let r = renaming else { return }
        if let k = r.component {
            model.perform("mask.component", ["id": r.mask, "component": .int(Int64(k)), "action": "rename", "name": .string(name)])
        } else {
            model.perform("mask.rename", ["id": r.mask, "name": .string(name)])
        }
        renaming = nil
    }
}

/// A tile of the masks' column: a symbol on a rounded square, ringed when chosen, dimmed when
/// hidden, with a badge (− subtracts, ∩ intersects) in its corner, and the name under it.
private struct MaskTile: View {
    let symbol: String
    let name: String
    let chosen: Bool
    let visible: Bool
    let side: CGFloat
    let badge: String?

    var body: some View {
        VStack(spacing: 3) {
            Image(systemName: symbol)
                .font(.system(size: side * 0.4))
                .frame(width: side, height: side)
                .background(RoundedRectangle(cornerRadius: side / 4).fill(Color(uiColor: .tertiarySystemFill)))
                .overlay {
                    RoundedRectangle(cornerRadius: side / 4).strokeBorder(chosen ? Color.accentColor : .clear, lineWidth: 2)
                }
                .overlay(alignment: .topTrailing) {
                    if let badge {
                        Text(badge).font(.caption.bold())
                            .frame(width: 16, height: 16)
                            .background(Circle().fill(Color(uiColor: .systemBackground)))
                            .offset(x: 4, y: -4)
                    }
                }
            Text(name).font(.caption2).lineLimit(1).frame(maxWidth: 72)
        }
        .foregroundStyle(chosen ? Color.accentColor : .primary)
        .opacity(visible ? 1 : 0.5)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(chosen ? .isSelected : [])
    }
}

/// + : Create New Mask.
private struct CreateMaskMenu: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Menu {
            Section("Create New Mask") {
                ForEach(MaskKind.create, id: \.kind) { k in
                    Button(LocalizedStringKey(k.label), systemImage: MaskKind.symbol(k.kind)) { create(k.kind) }
                }
            }
        } label: {
            Image(systemName: "plus").font(.title3.weight(.semibold))
                .frame(width: 36, height: 36)
        }
        .buttonStyle(.glassProminent)
        .buttonBorderShape(.circle)
        .accessibilityLabel("Create New Mask")
    }

    private func create(_ kind: String) {
        let editor = model.editor
        Task {
            if kind == "brush" {
                // the first stroke makes the mask
                await model.run("mask.select", ["id": .null])
            } else {
                await model.run("mask.add", ["kind": .string(kind)])
            }
            editor.maskTool = kind == "brush" || kind == "colorRange" ? kind : nil
            editor.brushErase = false
            if kind == "colorRange" { model.show(String(localized: "Tap the photo to pick a colour")) }
            if kind == "brush" { model.show(String(localized: "Paint the area to adjust")) }
        }
    }
}

/// ± : add a part to the selected mask, take one away from it, or intersect it with one.
private struct CombineMenu: View {
    @Environment(AppModel.self) private var model

    private static let ops: [(op: String, title: String, symbol: String)] = [
        ("add", "Add to Mask", "plus"), ("subtract", "Subtract from Mask", "minus"), ("intersect", "Intersect with Mask", "circle.righthalf.filled"),
    ]

    var body: some View {
        Menu {
            ForEach(Self.ops, id: \.op) { o in
                Menu(LocalizedStringKey(o.title), systemImage: o.symbol) {
                    // painting only adds or takes away
                    ForEach(MaskKind.parts.filter { !(o.op == "intersect" && $0 == "brush") }, id: \.self) { kind in
                        Button(LocalizedStringKey(MaskKind.name(kind)), systemImage: MaskKind.symbol(kind)) { add(kind, o.op) }
                    }
                }
            }
        } label: {
            Image(systemName: "plus.forwardslash.minus").font(.title3)
                .frame(width: 40, height: 40)
                .background(Circle().fill(Color(uiColor: .tertiarySystemFill)))
        }
        .accessibilityLabel("Add or Subtract")
    }

    private func add(_ kind: String, _ op: String) {
        let editor = model.editor
        Task {
            guard await model.run("mask.addComponent", ["kind": .string(kind), "op": .string(op)]) != nil else { return }
            if kind == "brush" {
                editor.maskTool = "brush"
                editor.brushErase = false
            }
        }
    }
}

/// Show or hide the selected mask's overlay (the top bar, while masking).
struct MaskOverlayButton: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var prefs = model.prefs
        Button(prefs.maskOverlay ? "Hide Overlay" : "Show Overlay", systemImage: prefs.maskOverlay ? "eye" : "eye.slash") {
            prefs.maskOverlay.toggle()
        }
        .haptic(.selection, trigger: prefs.maskOverlay)
    }
}

/// "…" while masking: Show Pins, Overlay Options, Delete All Masks.
struct MaskMoreMenu: View {
    @Environment(AppModel.self) private var model
    @State private var overlayOptions = false
    @State private var confirmDelete = false

    var body: some View {
        @Bindable var prefs = model.prefs
        Menu {
            Toggle("Show Pins", systemImage: "mappin", isOn: $prefs.maskPins)
            Button("Overlay Options", systemImage: "eye.circle") { overlayOptions = true }
            Divider()
            Button("Delete All Masks", systemImage: "trash", role: .destructive) { confirmDelete = true }
                .disabled(MaskList.all(model).isEmpty)
        } label: {
            Label("More", systemImage: "ellipsis")
        }
        .sheet(isPresented: $overlayOptions) { MaskOverlayOptions().presentationDetents([.medium]) }
        .confirmationDialog("Delete All Masks?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete All Masks", role: .destructive) {
                model.perform("mask.deleteAll")
                model.buzz(.warning)
                model.editor.maskTool = nil
            }
        }
    }
}

/// How the selected mask is drawn over the photo: the view, its colour and opacity.
struct MaskOverlayOptions: View {
    @Environment(AppModel.self) private var model

    private static let views: [(String, LocalizedStringKey)] = [
        ("color", "Color Overlay"), ("colorOnBw", "Color Overlay on B&W"), ("imageOnBlack", "Image on Black"),
        ("imageOnWhite", "Image on White"), ("whiteOnBlack", "White on Black"),
    ]

    var body: some View {
        @Bindable var prefs = model.prefs
        NavigationStack {
            Form {
                Picker("Show As", selection: $prefs.maskOverlayMode) {
                    ForEach(Self.views, id: \.0) { Text($0.1).tag($0.0) }
                }
                ColorPicker("Color", selection: color, supportsOpacity: false)
                LabeledContent("Opacity") {
                    Slider(value: $prefs.maskOverlayOpacity, in: 0...100, step: 1)
                    Text("\(Int(prefs.maskOverlayOpacity))").monospacedDigit().frame(width: 32, alignment: .trailing)
                }
                Toggle("Show Overlay", isOn: $prefs.maskOverlay)
            }
            .navigationTitle("Overlay Options")
            .navigationBarTitleDisplayMode(.inline)
        }
    }

    /// The overlay colour as SwiftUI's (sRGB 0–255 in the prefs).
    private var color: Binding<Color> {
        Binding {
            let c = model.prefs.maskOverlayColor + [0, 0, 0]
            return Color(.sRGB, red: Double(c[0]) / 255, green: Double(c[1]) / 255, blue: Double(c[2]) / 255)
        } set: { new in
            let r = new.resolve(in: EnvironmentValues())
            model.prefs.maskOverlayColor = [r.red, r.green, r.blue].map { Int((min(max(Double($0), 0), 1) * 255).rounded()) }
        }
    }
}

/// The brush: paint or erase, size, feather, flow, and Auto Mask (follows the photo's edges).
struct BrushControls: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var e = model.editor
        VStack(spacing: 0) {
            HStack {
                Picker("Brush", selection: $e.brushErase) {
                    Label("Paint", systemImage: "paintbrush.pointed").tag(false)
                    Label("Erase", systemImage: "eraser").tag(true)
                }
                .pickerStyle(.segmented)
                Toggle("Auto Mask", isOn: $e.brushAutoMask).toggleStyle(.button)
                Button("Done") { e.maskTool = nil }
            }
            ControlSlider(spec: ControlSpec(id: "brush.size", label: "Size", section: "brush", min: 1, max: 100, default: 12, step: 1, decimals: 0),
                          value: e.brushSize * 400, set: { e.brushSize = $0 / 400 }, editing: { _ in }, reset: { e.brushSize = 0.03 })
            ControlSlider(spec: ControlSpec(id: "brush.feather", label: "Feather", section: "brush", min: 0, max: 100, default: 50, step: 1, decimals: 0),
                          value: e.brushFeather, set: { e.brushFeather = $0 }, editing: { _ in }, reset: { e.brushFeather = 50 })
            ControlSlider(spec: ControlSpec(id: "brush.flow", label: "Flow", section: "brush", min: 1, max: 100, default: 100, step: 1, decimals: 0),
                          value: e.brushFlow, set: { e.brushFlow = $0 }, editing: { _ in }, reset: { e.brushFlow = 100 })
        }
        .padding(.horizontal, 16)
    }
}
#endif
