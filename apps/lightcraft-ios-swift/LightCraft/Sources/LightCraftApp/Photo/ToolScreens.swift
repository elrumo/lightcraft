import LightCraftCore
import SwiftUI

/// A tool screen's Cancel (take back everything done since it opened) and Done, in the navigation bar.
struct ScreenToolbar: ToolbarContent {
    let model: AppModel

    var body: some ToolbarContent {
        ToolbarItem(placement: .cancellationAction) {
            Button(role: .close) {
                Task {
                    if let base = model.editor.screenBase { await model.run("edit.undoTo", ["depth": .int(Int64(base))]) }
                    model.editor.tool = .edit
                }
            }
            .accessibilityLabel("Cancel")
        }
        ToolbarItem(placement: .confirmationAction) {
            Button(role: .confirm) {
                model.perform("develop.endInteraction")
                model.editor.tool = .edit
            }
            .accessibilityLabel("Done")
        }
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
    @Environment(\.toolColumn) private var inColumn
    @State private var dialStart: Double?

    /// The crop's composition guides (`crop.guides` kinds).
    static let guides: [(String, String)] = [
        ("thirds", "Rule of Thirds"), ("grid", "Grid"), ("golden", "Golden Ratio"), ("diagonal", "Diagonal"),
        ("triangle", "Golden Triangle"), ("spiral", "Golden Spiral"), ("none", "None"),
    ]

    static let ratios: [(String, String)] = [
        ("1x1", "1 × 1"), ("4x5", "4 × 5"), ("3x4", "3 × 4"), ("2x3", "2 × 3"), ("5x7", "5 × 7"),
        ("9x16", "9 × 16"), ("10x16", "10 × 16"), ("8.5x11", "8.5 × 11"), ("1x2", "1 × 2"),
    ]

    var body: some View {
        let editor = model.editor
        @Bindable var editorBindings = editor
        let tab = editor.cropTab
        let wide = inColumn && model.duo.unfolded
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
                // (closer together in a column: a phone held sideways, an iPad's inspector)
                HStack(spacing: inColumn ? 4 : 18) {
                    toolButton("wand.and.stars", "Auto") {
                        Task {
                            let r = await model.run("crop.autoStraighten")
                            if r?["changed"].bool == false { model.show(String(localized: "No horizon or vertical lines found")) } else if r != nil { model.buzz(.light) }
                        }
                    }
                    toolButton("line.diagonal", "Straighten") { editor.cropDraw = editor.cropDraw == "straighten" ? nil : "straighten" }
                    toolButton(locked ? "lock" : "lock.open", locked ? "Unlock Aspect" : "Lock Aspect") {
                        model.perform("crop.aspect", ["aspect": "toggle"])
                        model.buzz(.selection)
                    }
                    toolButton("rotate.left", "Rotate") { model.perform("photo.rotateLeft") }
                    Menu {
                        Button("Flip Horizontal", systemImage: "arrow.left.and.right.righttriangle.left.righttriangle.right") { model.perform("photo.flipHorizontal") }
                        Button("Flip Vertical", systemImage: "arrow.up.and.down.righttriangle.up.righttriangle.down") { model.perform("photo.flipVertical") }
                        Button("Reset Crop", systemImage: "arrow.counterclockwise") { model.perform("crop.reset"); model.buzz(.light) }
                        Divider()
                        Picker("Overlay", selection: Binding(get: { model.prefs.cropGuide }, set: { model.prefs.cropGuide = $0 })) {
                            ForEach(Self.guides, id: \.0) { Text(LocalizedStringKey($0.1)).tag($0.0) }
                        }
                        .pickerStyle(.menu)
                        if model.prefs.cropGuide == "triangle" || model.prefs.cropGuide == "spiral" {
                            Button("Cycle Overlay Orientation", systemImage: "arrow.triangle.2.circlepath") {
                                model.prefs.cropGuideOrientation = (model.prefs.cropGuideOrientation + 1) % 4
                            }
                        }
                    } label: {
                        Image(systemName: "ellipsis").frame(width: buttonSide, height: buttonSide)
                    }
                    .buttonStyle(.glass)
                    .buttonBorderShape(.circle)
                    .accessibilityLabel("More")
                }
            }
            if tab == "aspect", expand {
                ExpandControls()
                    .transition(.opacity.combined(with: .move(edge: .bottom)))
            }
            Picker("Tab", selection: $editorBindings.cropTab) {
                Text("Aspect").tag("aspect")
                Text("Geometry").tag("geometry")
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16)
            .onChange(of: tab) { _, t in
                editor.cropDraw = nil
                model.buzz(.selection)
                if t == "geometry" { editor.want(["geometry", "optics"], model) }
            }
            if tab == "aspect" {
                if wide {
                    // the room of an unfolded Duo: the tiles in a grid that fills the panel, not a row to scroll
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 72), spacing: 10)], spacing: 10) { aspectTiles }
                        .padding(.horizontal, 16)
                } else {
                    ScrollView(.horizontal) {
                        HStack(spacing: 10) { aspectTiles }
                            .padding(.horizontal, 16)
                    }
                    .scrollIndicators(.hidden)
                }
            } else {
                ScrollView {
                    GeometryControls(wide: wide)
                        .padding(.horizontal, 16)
                }
                .frame(maxHeight: wide ? .infinity : 236)
            }
        }
        .padding(.vertical, 8)
        .animation(.snappy, value: expand)
        .onDisappear { editor.cropDraw = nil }
    }

    /// Expand, Original, Free and the ratio menus.
    @ViewBuilder private var aspectTiles: some View {
        // Expand: the crop may reach past the photo, and Generate paints the new area
        Button {
            model.perform("crop.expand", ["on": .bool(!expand)])
            model.buzz(.selection)
        } label: {
            tileLabel("Expand", "arrow.up.left.and.arrow.down.right", on: expand)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(expand ? .isSelected : [])
        .accessibilityHint("Lets the crop reach past the photo's edges, to paint the new area")
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

    /// Expand is on: the crop may reach past the photo.
    private var expand: Bool { model.editor.setting("crop", "expand").bool == true }

    /// The round buttons' size: smaller in a column.
    private var buttonSide: CGFloat { inColumn ? 34 : 44 }

    private func toolButton(_ symbol: String, _ label: LocalizedStringKey, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol).frame(width: buttonSide, height: buttonSide)
        }
        .buttonStyle(.glass)
        .buttonBorderShape(.circle)
        .accessibilityLabel(label)
    }

    private func aspectTile(_ label: LocalizedStringKey, _ symbol: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) { tileLabel(label, symbol) }
            .buttonStyle(.plain)
    }

    private func tileLabel(_ label: LocalizedStringKey, _ symbol: String, on: Bool = false) -> some View {
        VStack(spacing: 6) {
            Image(systemName: symbol).font(.title3)
            Text(label).font(.caption)
        }
        .foregroundStyle(on ? Color.white : Color.primary)
        .frame(width: 72, height: 66)
        .background(on ? Color.accentColor : Color.lcTertiaryFill, in: .rect(cornerRadius: 12))
    }
}

/// Crop's Geometry: Upright (Auto, Level, Vertical, Full, Guided and its guides), the distortion and
/// perspective sliders, Constrain Crop.
struct GeometryControls: View {
    @Environment(AppModel.self) private var model
    /// The buttons in a grid that wraps (an unfolded Duo's column), not a row that shortens their names.
    var wide = false

    @ViewBuilder private func row<C: View>(@ViewBuilder _ content: () -> C) -> some View {
        if wide {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 84), spacing: 8)], spacing: 8) { content() }
        } else {
            HStack { content() }
        }
    }

    var body: some View {
        let editor = model.editor
        VStack(alignment: .leading) {
            Text("Upright").font(.subheadline.weight(.semibold))
            row {
                ForEach([("auto", "Auto"), ("level", "Level"), ("vertical", "Vertical"), ("full", "Full"), ("guided", "Guided")], id: \.0) { mode, label in
                    let on = editor.setting("geometry", "upright").string == mode
                    Button(LocalizedStringKey(label)) {
                        model.perform("geometry.upright", ["mode": .string(on ? "off" : mode)])
                        // Guided: draw its guides on the photo
                        editor.cropDraw = mode == "guided" && !on ? "guides" : nil
                    }
                    .buttonStyle(.bordered)
                    .tint(on ? .accentColor : .secondary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
                }
            }
            if editor.setting("geometry", "upright").string == "guided" {
                let n = editor.setting("geometry", "guides").array?.count ?? 0
                Text("\(n) of 4 guides — drag along lines that should be vertical or horizontal.")
                    .font(.caption).foregroundStyle(.secondary)
                HStack {
                    Button(editor.cropDraw == "guides" ? "Done" : "Draw Guides") {
                        editor.cropDraw = editor.cropDraw == "guides" ? nil : "guides"
                    }
                    if n > 0 {
                        Button("Clear Guides", role: .destructive) { model.perform("geometry.guides", ["guides": []]) }
                    }
                }
                .buttonStyle(.bordered)
            }
            Sliders(section: "optics") { $0.id == "optics.distortion" }
            Sliders(section: "geometry") { $0.id != "crop.angle" }
            Toggle("Constrain Crop", isOn: Binding(get: { editor.setting("geometry", "constrain_crop").bool == true }, set: {
                model.perform("develop.merge", ["settings": ["geometry": ["constrain_crop": .bool($0)]], "label": "Constrain Crop"])
            }))
        }
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
            .haptic(.selection, trigger: tick)
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
    @Environment(\.toolColumn) private var column

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
                    Toggle("Visualize Spots", systemImage: "eye", isOn: $editor.visualizeSpots)
                    Divider()
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
            if editor.removeMode == "remove" {
                Picker("Select with", selection: $editor.removeTap) {
                    Label("Brush", systemImage: "paintbrush.pointed").tag(false)
                    Label("Tap", systemImage: "hand.tap").tag(true)
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 16)
                .transition(.opacity.combined(with: .move(edge: .bottom)))
            }
            VStack(spacing: 0) {
                if editor.removeMode == "remove" && editor.removeTap {
                    // how far past the tapped object's edge the spot reaches (the selected tapped spot's own, as it is)
                    ControlSlider(spec: ControlSpec(id: "remove.reach", label: "Size", section: "remove", min: 1, max: 30, default: 4, step: 1, decimals: 0),
                                  value: (model.tappedSpot?.spot["size"].double ?? editor.removeTapSize) * 1000, set: { model.tapSizeChanged($0 / 1000) },
                                  editing: { model.tapSizeEditing($0) }, reset: { model.tapSizeChanged(0.004) })
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                } else {
                    ControlSlider(spec: ControlSpec(id: "remove.size", label: "Size", section: "remove", min: 1, max: 250, default: 20, step: 1, decimals: 0),
                                  value: editor.removeSize * 1000, set: { editor.removeSize = $0 / 1000 }, editing: { _ in }, reset: { editor.removeSize = 0.02 })
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
                if editor.removeMode != "remove" {
                    ControlSlider(spec: ControlSpec(id: "remove.feather", label: "Feather", section: "remove", min: 0, max: 100, default: 50, step: 1, decimals: 0),
                                  value: editor.removeFeather, set: { editor.removeFeather = $0 }, editing: { _ in }, reset: { editor.removeFeather = 50 })
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                    ControlSlider(spec: ControlSpec(id: "remove.opacity", label: "Opacity", section: "remove", min: 0, max: 100, default: 100, step: 1, decimals: 0),
                                  value: editor.removeOpacity, set: { editor.removeOpacity = $0 }, editing: { _ in }, reset: { editor.removeOpacity = 100 })
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
                if editor.removeMode == "remove" {
                    GenerativeAISwitch()
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
                if editor.visualizeSpots {
                    ControlSlider(spec: ControlSpec(id: "remove.spotsThreshold", label: "Threshold", section: "remove", min: 0, max: 100, default: 50, step: 1, decimals: 0),
                                  value: editor.spotsThreshold, set: { editor.spotsThreshold = $0 }, editing: { _ in }, reset: { editor.spotsThreshold = 50 })
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
            }
            .padding(.horizontal, 16)
            Group {
                if spots.isEmpty {
                    if editor.removeMode == "remove" && editor.removeTap {
                        Text("Tap an object to take it away.").transition(.opacity)
                    } else {
                        Text("Paint over what to take away.").transition(.opacity)
                    }
                } else {
                    Text("^[\(spots.count) spot](inflect: true) · tap a pin for more")
                }
            }
            .font(.footnote).foregroundStyle(.secondary)
        }
        .padding(.vertical, 12)
        // what appears makes room smoothly, and settles in, instead of popping in and pushing the rest at once
        .animation(.snappy, value: editor.removeMode)
        .animation(.snappy, value: editor.removeTap)
        .animation(.snappy, value: editor.visualizeSpots)
        .animation(.snappy, value: spots.isEmpty)
        // under the photo it's a sheet like the other tools' panels
        .toolCard(!column)
    }
}

/// Remove's Generative AI switch (saved with the app's settings) and, when it's on, the MI-GAN
/// model's state: the offer to download it (size, licence, credit first), its download, or nothing
/// once it's here (`models.list`, `models.download`, `models.cancel`).
struct GenerativeAISwitch: View {
    @Environment(AppModel.self) private var model
    @State private var info: JSONValue = .null

    private static let id = "migan"

    var body: some View {
        @Bindable var prefs = model.prefs
        VStack(alignment: .leading, spacing: 6) {
            Toggle("Generative AI", isOn: $prefs.removeAI)
            if prefs.removeAI, !model.editor.eraseReady, !info.isNull {
                Group {
                    if info["available"].bool == false {
                        Text("AI Remove isn't available in this build.")
                    } else if info["enabled"].bool == false {
                        Text("AI Remove is turned off in Settings → AI Models.")
                    } else if info["download"]["running"].bool == true {
                        let d = info["download"], total = max(d["total"].double ?? 0, 1)
                        HStack {
                            ProgressView(value: (d["done"].double ?? 0) / total) { Text("Downloading MI-GAN…") }
                            Button("Cancel Download") { model.perform("models.cancel", ["id": .string(Self.id)]) }
                        }
                    } else {
                        Text("Uses the MI-GAN model, a \(Int((info["downloadBytes"].double ?? 0) / 1e6)) MB download (\(info["licence"].string ?? "")). \(info["credit"].string ?? "").")
                        if let e = info["download"]["error"].string { Text(e).foregroundStyle(.red) }
                        Button("Download") { model.perform("models.download", ["id": .string(Self.id), "acknowledged": true]) }
                            .buttonStyle(.bordered)
                    }
                }
                .font(.footnote)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            }
        }
        .task(id: prefs.removeAI) {
            // the model's state, until it's here (often while it downloads)
            while !Task.isCancelled {
                if let engine = model.engine, let list = (try? await engine.execute("models.list"))?["models"].array,
                   let m = list.first(where: { $0["id"].string == Self.id })
                {
                    info = m
                    model.editor.eraseReady = m["installed"].bool == true && m["enabled"].bool != false && m["available"].bool != false
                }
                guard prefs.removeAI, !model.editor.eraseReady else { return }
                try? await Task.sleep(for: .milliseconds(info["download"]["running"].bool == true ? 400 : 1500))
            }
        }
    }
}
