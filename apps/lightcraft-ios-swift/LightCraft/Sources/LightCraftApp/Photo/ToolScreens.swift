#if os(iOS)
import LightCraftCore
import SwiftUI

/// The bar of a tool screen: ✕ (take back everything done since it opened), the tool's name, ✓.
struct ScreenBar: View {
    @Environment(AppModel.self) private var model
    let title: LocalizedStringKey
    /// The undo stack's depth when the screen opened.
    let base: Int?

    var body: some View {
        HStack {
            Button {
                Task {
                    if let base { await model.run("edit.undoTo", ["depth": .int(Int64(base))]) }
                    model.editor.tool = .edit
                }
            } label: {
                Image(systemName: "xmark").frame(width: 44, height: 44)
            }
            .buttonStyle(.glass)
            .buttonBorderShape(.circle)
            .accessibilityLabel("Cancel")
            Spacer()
            Text(title).font(.headline)
            Spacer()
            Button {
                model.perform("develop.endInteraction")
                model.editor.tool = .edit
            } label: {
                Image(systemName: "checkmark").frame(width: 44, height: 44)
            }
            .buttonStyle(.glassProminent)
            .buttonBorderShape(.circle)
            .accessibilityLabel("Done")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
    }
}

/// The undo stack's depth now (`edit.stack`), for a tool screen's ✕.
func undoDepth(_ model: AppModel) async -> Int? {
    guard let engine = model.engine, let r = try? await engine.execute("edit.stack") else { return nil }
    return r["depth"].int.map(Int.init)
}

/// Crop & Rotate: the aspect (Original, Free, ratios), straightening on a dial, auto-level, lock,
/// rotate, flips; the Geometry tab for Upright and the perspective sliders.
struct CropTools: View {
    @Environment(AppModel.self) private var model
    @State private var base: Int?
    @State private var tab = "aspect"
    @State private var dialStart: Double?

    private static let ratios: [(String, String)] = [
        ("1x1", "1 × 1"), ("4x5", "4 × 5"), ("3x4", "3 × 4"), ("2x3", "2 × 3"), ("5x7", "5 × 7"),
        ("9x16", "9 × 16"), ("10x16", "10 × 16"), ("8.5x11", "8.5 × 11"), ("1x2", "1 × 2"),
    ]

    var body: some View {
        let editor = model.editor
        let angle = editor.setting("crop", "geometry", "angle").double ?? 0
        let locked = editor.setting("crop", "aspect") != .null
        VStack(spacing: 10) {
            if tab == "aspect" {
                AngleDial(angle: angle) { new, phase in
                    switch phase {
                    case .began: model.engine?.send("engine.execute", ["command": "develop.beginInteraction", "params": ["label": "Straighten"]])
                    case .changed: break
                    case .ended: model.engine?.send("engine.execute", ["command": "develop.endInteraction"])
                    }
                    model.engine?.send("engine.execute", ["command": "crop.straighten", "params": ["angle": .double(new)]])
                    if phase == .changed { model.requestDraft() }
                }
                HStack(spacing: 18) {
                    toolButton("wand.and.stars", "Auto") {
                        Task {
                            let r = await model.run("crop.autoStraighten")
                            if r?["changed"].bool == false { model.show(String(localized: "No horizon or vertical lines found")) }
                        }
                    }
                    toolButton(locked ? "lock" : "lock.open", locked ? "Unlock Aspect" : "Lock Aspect") {
                        model.perform("crop.aspect", ["aspect": "toggle"])
                    }
                    toolButton("rotate.left", "Rotate") { model.perform("photo.rotateLeft") }
                    Menu {
                        Button("Flip Horizontal", systemImage: "arrow.left.and.right.righttriangle.left.righttriangle.right") { model.perform("photo.flipHorizontal") }
                        Button("Flip Vertical", systemImage: "arrow.up.and.down.righttriangle.up.righttriangle.down") { model.perform("photo.flipVertical") }
                        Button("Reset Crop", systemImage: "arrow.counterclockwise") { model.perform("crop.reset") }
                    } label: {
                        Image(systemName: "ellipsis").frame(width: 44, height: 44)
                    }
                    .buttonStyle(.glass)
                    .buttonBorderShape(.circle)
                    .accessibilityLabel("More")
                }
            }
            Picker("Tab", selection: $tab) {
                Text("Aspect").tag("aspect")
                Text("Geometry").tag("geometry")
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16)
            .onChange(of: tab) { _, t in if t == "geometry" { editor.want(["geometry", "optics"], model) } }
            if tab == "aspect" {
                ScrollView(.horizontal) {
                    HStack(spacing: 10) {
                        aspectTile("Original", "rectangle") { model.perform("crop.aspect", ["aspect": "original"]) }
                        aspectTile("Free", "crop") { model.perform("crop.aspect", ["aspect": "free"]) }
                        Menu {
                            ForEach(Self.ratios, id: \.0) { key, label in
                                Button(label) { model.perform("crop.aspect", ["aspect": .string(key)]) }
                            }
                            Button("Swap Orientation", systemImage: "rectangle.portrait.rotate") { model.perform("crop.rotateAspect") }
                        } label: {
                            tileLabel("Ratios", "aspectratio")
                        }
                        Menu {
                            Button("Square 1 × 1") { model.perform("crop.aspect", ["aspect": "1x1"]) }
                            Button("Portrait 4 × 5") { model.perform("crop.aspect", ["aspect": "4x5"]) }
                            Button("Story or Reel 9 × 16") { model.perform("crop.aspect", ["aspect": "9x16"]) }
                            Button("Landscape 1.91 × 1") { model.perform("crop.aspect", ["aspect": "100x191"]) }
                        } label: {
                            tileLabel("Social", "person.crop.square")
                        }
                    }
                    .padding(.horizontal, 16)
                }
                .scrollIndicators(.hidden)
            } else {
                ScrollView {
                    VStack(alignment: .leading) {
                        Text("Upright").font(.subheadline.weight(.semibold))
                        HStack {
                            ForEach([("auto", "Auto"), ("level", "Level"), ("vertical", "Vertical"), ("full", "Full")], id: \.0) { mode, label in
                                let on = editor.setting("geometry", "upright").string == mode
                                Button(LocalizedStringKey(label)) {
                                    model.perform("geometry.upright", ["mode": .string(on ? "off" : mode)])
                                }
                                .buttonStyle(.bordered)
                                .tint(on ? .accentColor : .secondary)
                            }
                        }
                        Sliders(section: "optics") { $0.id == "optics.distortion" }
                        Sliders(section: "geometry") { $0.id != "crop.angle" }
                        Toggle("Constrain Crop", isOn: Binding(get: { editor.setting("geometry", "constrain_crop").bool == true }, set: {
                            model.perform("develop.merge", ["settings": ["geometry": ["constrain_crop": .bool($0)]], "label": "Constrain Crop"])
                        }))
                    }
                    .padding(.horizontal, 16)
                }
                .frame(maxHeight: 236)
            }
            ScreenBar(title: "Crop & Rotate", base: base)
        }
        .padding(.top, 8)
        .task { base = await undoDepth(model) }
    }

    private func toolButton(_ symbol: String, _ label: LocalizedStringKey, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol).frame(width: 44, height: 44)
        }
        .buttonStyle(.glass)
        .buttonBorderShape(.circle)
        .accessibilityLabel(label)
    }

    private func aspectTile(_ label: LocalizedStringKey, _ symbol: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) { tileLabel(label, symbol) }
            .buttonStyle(.plain)
    }

    private func tileLabel(_ label: LocalizedStringKey, _ symbol: String) -> some View {
        VStack(spacing: 6) {
            Image(systemName: symbol).font(.title3)
            Text(label).font(.caption)
        }
        .frame(width: 72, height: 66)
        .background(Color(uiColor: .tertiarySystemFill), in: .rect(cornerRadius: 12))
    }
}

/// The straightening dial: a ruler that slides under a fixed mark, a tick per degree, the angle
/// above it. Dragging is one undo step.
struct AngleDial: View {
    enum Phase { case began, changed, ended }
    let angle: Double
    let changed: (Double, Phase) -> Void
    @State private var start: Double?
    @State private var shown: Double?
    /// Points per degree.
    private let perDegree: CGFloat = 7

    var body: some View {
        let a = shown ?? angle
        VStack(spacing: 2) {
            Text(String(format: "%.1f°", a)).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            Canvas { ctx, size in
                let mid = size.width / 2
                for deg in -45...45 {
                    let x = mid + (CGFloat(deg) - CGFloat(a)) * perDegree
                    guard x > 0, x < size.width else { continue }
                    let long = deg % 5 == 0
                    var p = Path()
                    p.move(to: CGPoint(x: x, y: size.height * (long ? 0.15 : 0.35)))
                    p.addLine(to: CGPoint(x: x, y: size.height * 0.85))
                    let fade = 1 - abs(x - mid) / mid
                    ctx.stroke(p, with: .color(.primary.opacity(0.25 + 0.6 * fade)), lineWidth: long ? 1.5 : 1)
                }
                var mark = Path()
                mark.move(to: CGPoint(x: mid, y: 0))
                mark.addLine(to: CGPoint(x: mid, y: size.height))
                ctx.stroke(mark, with: .color(.accentColor), lineWidth: 2)
            }
            .frame(height: 30)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 2)
                    .onChanged { g in
                        let from = start ?? angle
                        if start == nil {
                            start = angle
                            changed(angle, .began)
                        }
                        let v = min(max(((from - Double(g.translation.width / perDegree)) / 0.05).rounded() * 0.05, -45), 45)
                        if Int(v) != Int(shown ?? from) { tick += 1 }
                        shown = v
                        changed(v, .changed)
                    }
                    .onEnded { _ in
                        changed(shown ?? angle, .ended)
                        start = nil
                        shown = nil
                    }
            )
            .onTapGesture(count: 2) { changed(0, .began); changed(0, .ended) }
            .sensoryFeedback(.selection, trigger: tick)
        }
        .padding(.horizontal, 16)
        .accessibilityElement()
        .accessibilityLabel("Straighten")
        .accessibilityValue(String(format: "%.1f degrees", a))
        .accessibilityAdjustableAction { dir in
            let v = angle + (dir == .increment ? 0.5 : -0.5)
            changed(v, .began)
            changed(v, .ended)
        }
    }

    @State private var tick = 0
}

/// Remove: Remove, Heal or Clone, the brush's size, and the photo's spots (Find Dust, delete all).
struct RemoveTools: View {
    @Environment(AppModel.self) private var model
    @State private var base: Int?

    var body: some View {
        @Bindable var editor = model.editor
        let spots = model.editor.setting("spots").array ?? []
        VStack(spacing: 10) {
            HStack {
                Picker("Tool", selection: $editor.removeMode) {
                    Label("Remove", systemImage: "eraser").tag("remove")
                    Label("Heal", systemImage: "bandage").tag("heal")
                    Label("Clone", systemImage: "square.on.square").tag("clone")
                }
                .pickerStyle(.segmented)
                Menu {
                    Button("Find Dust Spots", systemImage: "sparkle.magnifyingglass") {
                        Task {
                            if let r = await model.run("spot.findDust", ["add": true]) {
                                model.show(String(localized: "Healed \(r["added"].int ?? 0) dust spots"))
                            }
                        }
                    }
                    Button("Delete All Spots", systemImage: "trash", role: .destructive) {
                        Task {
                            for i in stride(from: spots.count - 1, through: 0, by: -1) {
                                await model.run("spot.delete", ["index": .int(Int64(i))])
                            }
                        }
                    }
                    .disabled(spots.isEmpty)
                } label: {
                    Image(systemName: "ellipsis.circle").font(.title3)
                }
                .accessibilityLabel("More")
            }
            .padding(.horizontal, 16)
            VStack(spacing: 0) {
                ControlSlider(spec: ControlSpec(id: "remove.size", label: "Size", section: "remove", min: 1, max: 250, default: 20, step: 1, decimals: 0),
                              value: editor.removeSize * 1000, set: { editor.removeSize = $0 / 1000 }, editing: { _ in }, reset: { editor.removeSize = 0.02 })
                if editor.removeMode != "remove" {
                    ControlSlider(spec: ControlSpec(id: "remove.feather", label: "Feather", section: "remove", min: 0, max: 100, default: 50, step: 1, decimals: 0),
                                  value: editor.removeFeather, set: { editor.removeFeather = $0 }, editing: { _ in }, reset: { editor.removeFeather = 50 })
                    ControlSlider(spec: ControlSpec(id: "remove.opacity", label: "Opacity", section: "remove", min: 0, max: 100, default: 100, step: 1, decimals: 0),
                                  value: editor.removeOpacity, set: { editor.removeOpacity = $0 }, editing: { _ in }, reset: { editor.removeOpacity = 100 })
                }
            }
            .padding(.horizontal, 16)
            Group {
                if spots.isEmpty {
                    Text("Paint over what to take away.")
                } else {
                    Text("^[\(spots.count) spot](inflect: true) · tap a pin for more")
                }
            }
            .font(.footnote).foregroundStyle(.secondary)
            ScreenBar(title: "Remove", base: base)
        }
        .padding(.top, 8)
        .task { base = await undoDepth(model) }
    }
}
#endif
