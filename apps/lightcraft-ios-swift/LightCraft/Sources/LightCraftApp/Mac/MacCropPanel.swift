#if os(macOS)
import LightCraftCore
import SwiftUI

/// Crop & Rotate as Lightroom lays it out: Aspect (a menu and its lock), Angle (a slider), the
/// buttons that straighten, turn and flip, the overlay; and Geometry (Upright and the perspective
/// sliders) as a group of its own. Not the phone's ruler and row of tiles.
struct MacCropPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                MacSection(id: "crop", title: "Crop", open: true) {
                    aspect
                    angle
                    tools
                    overlay
                    expand
                }
                MacSection(id: "geometry", title: "Geometry") { GeometryControls() }
            }
        }
        .task { model.editor.want(["geometry", "optics"], model) }
        .onDisappear { model.editor.cropDraw = nil }
    }

    // MARK: aspect

    private var aspect: some View {
        let current = model.editor.setting("crop", "aspect")
        let locked = current != .null
        return HStack(spacing: 8) {
            Text("Aspect").font(.callout).foregroundStyle(.secondary)
            Menu(Self.name(of: current)) {
                Button("Original") { model.perform("crop.aspect", ["aspect": "original"]) }
                Button("Free") { model.perform("crop.aspect", ["aspect": "free"]) }
                Divider()
                ForEach(CropTools.ratios, id: \.0) { key, label in
                    Button(label) { model.perform("crop.aspect", ["aspect": .string(key)]) }
                }
                Button("Swap Orientation", systemImage: "rectangle.portrait.rotate") { model.perform("crop.rotateAspect") }
                Divider()
                Menu("Social") {
                    Button("Square 1 × 1") { model.perform("crop.aspect", ["aspect": "1x1"]) }
                    Button("Portrait 4 × 5") { model.perform("crop.aspect", ["aspect": "4x5"]) }
                    Button("Story or Reel 9 × 16") { model.perform("crop.aspect", ["aspect": "9x16"]) }
                    Button("Landscape 1.91 × 1") { model.perform("crop.aspect", ["aspect": "100x191"]) }
                }
            }
            .menuStyle(.button)
            .buttonStyle(.borderless)
            .fixedSize()
            Spacer()
            Button {
                model.perform("crop.aspect", ["aspect": "toggle"])
            } label: {
                Image(systemName: locked ? "lock" : "lock.open")
            }
            .buttonStyle(.borderless)
            .help(Text(locked ? "Unlock Aspect" : "Lock Aspect"))
        }
        .padding(.bottom, 4)
    }

    /// "3 × 2" for a locked ratio, "Free" for none.
    private static func name(of aspect: JSONValue) -> String {
        guard let r = aspect.array, r.count == 2, let w = r[0].int, let h = r[1].int, w > 0, h > 0 else { return String(localized: "Free") }
        var (a, b) = (w, h)
        while b != 0 { (a, b) = (b, a % b) }
        return "\(w / a) × \(h / a)"
    }

    // MARK: angle and tools

    /// Straightening: the same engine calls as the phone's dial, one undo step per drag.
    private var angle: some View {
        let editor = model.editor
        return MacSliderRow(
            spec: ControlSpec(id: "crop.angle", label: "Angle", section: "crop", min: -45, max: 45, default: 0, step: 0.1, decimals: 1),
            value: editor.setting("crop", "geometry", "angle").double ?? 0,
            set: { v in
                model.engine?.send("engine.execute", ["command": "crop.straighten", "params": ["angle": .double(v)]])
                model.requestDraft()
            },
            editing: { began in
                model.engine?.send("engine.execute", ["command": began ? "develop.beginInteraction" : "develop.endInteraction", "params": began ? ["label": "Straighten"] : [:]])
            },
            reset: {
                model.engine?.send("engine.execute", ["command": "develop.beginInteraction", "params": ["label": "Straighten"]])
                model.engine?.send("engine.execute", ["command": "crop.straighten", "params": ["angle": 0]])
                model.engine?.send("engine.execute", ["command": "develop.endInteraction"])
            }
        )
    }

    /// Auto-straighten, the line tool, turn and flip, start over.
    private var tools: some View {
        let editor = model.editor
        return HStack(spacing: 6) {
            toolButton("wand.and.stars", "Auto") {
                Task {
                    let r = await model.run("crop.autoStraighten")
                    if r?["changed"].bool == false { model.show(String(localized: "No horizon or vertical lines found")) }
                }
            }
            toolButton("line.diagonal", "Straighten", on: editor.cropDraw == "straighten") {
                editor.cropDraw = editor.cropDraw == "straighten" ? nil : "straighten"
            }
            toolButton("rotate.left", "Rotate Left") { model.perform("photo.rotateLeft") }
            toolButton("rotate.right", "Rotate Right") { model.perform("photo.rotateRight") }
            toolButton("arrow.left.and.right.righttriangle.left.righttriangle.right", "Flip Horizontal") { model.perform("photo.flipHorizontal") }
            toolButton("arrow.up.and.down.righttriangle.up.righttriangle.down", "Flip Vertical") { model.perform("photo.flipVertical") }
            Spacer()
            toolButton("arrow.counterclockwise", "Reset Crop") { model.perform("crop.reset") }
        }
        .padding(.vertical, 6)
    }

    private func toolButton(_ symbol: String, _ title: LocalizedStringKey, on: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 13))
                .frame(width: 28, height: 26)
                .background(on ? Color.accentColor.opacity(0.3) : Color.primary.opacity(0.08), in: .rect(cornerRadius: 6))
        }
        .buttonStyle(.plain)
        .help(Text(title))
        .accessibilityLabel(Text(title))
    }

    // MARK: overlay and expand

    private var overlay: some View {
        @Bindable var prefs = model.prefs
        return HStack(spacing: 8) {
            Text("Overlay").font(.callout).foregroundStyle(.secondary)
            Picker("Overlay", selection: $prefs.cropGuide) {
                ForEach(CropTools.guides, id: \.0) { Text(LocalizedStringKey($0.1)).tag($0.0) }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .fixedSize()
            if prefs.cropGuide == "triangle" || prefs.cropGuide == "spiral" {
                Button {
                    prefs.cropGuideOrientation = (prefs.cropGuideOrientation + 1) % 4
                } label: {
                    Image(systemName: "arrow.triangle.2.circlepath")
                }
                .buttonStyle(.borderless)
                .help(Text("Cycle Overlay Orientation"))
            }
            Spacer()
        }
        .padding(.bottom, 4)
    }

    /// Expand: the crop may reach past the photo, and Generate paints the new area.
    @ViewBuilder private var expand: some View {
        let on = model.editor.setting("crop", "expand").bool == true
        Toggle("Expand", isOn: Binding(get: { on }, set: { model.perform("crop.expand", ["on": .bool($0)]) }))
            .help(Text("Lets the crop reach past the photo's edges, to paint the new area"))
        if on { ExpandControls().padding(.top, 6) }
    }
}
#endif
