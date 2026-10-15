#if os(macOS)
import LightCraftCore
import SwiftUI

/// Crop & Rotate as Lightroom lays it out: Aspect (a menu and its lock), Angle (a slider), the
/// buttons that straighten, turn and flip, the overlay and Expand; and Geometry (Upright and the
/// perspective sliders) as a group of its own. Not the phone's ruler and row of tiles.
struct MacCropPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        MacPanel(title: "Crop & Rotate") {
            MacIconButton(symbol: "arrow.counterclockwise", title: "Reset Crop") { model.perform("crop.reset") }
        } content: {
            MacGroup {
                aspect
                angle
                tools
                overlay
                expand
            }
            MacGroup(title: "Geometry") { MacGeometryControls() }
        }
        .task { model.editor.want(["geometry", "optics"], model) }
        .onDisappear { model.editor.cropDraw = nil }
    }

    // MARK: aspect

    private var aspect: some View {
        let current = model.editor.setting("crop", "aspect")
        let locked = current != .null
        return MacRow(label: "Aspect") {
            HStack(spacing: 4) {
                MacMenuField(text: Text(Self.name(of: current))) {
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
                MacIconButton(symbol: locked ? "lock" : "lock.open", title: locked ? "Unlock Aspect" : "Lock Aspect", on: locked) {
                    model.perform("crop.aspect", ["aspect": "toggle"])
                }
            }
        }
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

    /// Auto-straighten, the line tool, turn and flip, side by side.
    private var tools: some View {
        let editor = model.editor
        return MacIconBar(items: [
            .init(symbol: "wand.and.stars", title: "Auto") {
                Task {
                    let r = await model.run("crop.autoStraighten")
                    if r?["changed"].bool == false { model.show(String(localized: "No horizon or vertical lines found")) }
                }
            },
            .init(symbol: "line.diagonal", title: "Straighten", on: editor.cropDraw == "straighten") {
                editor.cropDraw = editor.cropDraw == "straighten" ? nil : "straighten"
            },
            .init(symbol: "rotate.left", title: "Rotate Left") { model.perform("photo.rotateLeft") },
            .init(symbol: "rotate.right", title: "Rotate Right") { model.perform("photo.rotateRight") },
            .init(symbol: "arrow.left.and.right.righttriangle.left.righttriangle.right", title: "Flip Horizontal") { model.perform("photo.flipHorizontal") },
            .init(symbol: "arrow.up.and.down.righttriangle.up.righttriangle.down", title: "Flip Vertical") { model.perform("photo.flipVertical") },
        ])
        .padding(.vertical, 4)
    }

    // MARK: overlay and expand

    private var overlay: some View {
        @Bindable var prefs = model.prefs
        return MacRow(label: "Overlay") {
            HStack(spacing: 4) {
                MacMenuField(text: Text(LocalizedStringKey(CropTools.guides.first { $0.0 == prefs.cropGuide }?.1 ?? "Rule of Thirds"))) {
                    Picker("Overlay", selection: $prefs.cropGuide) {
                        ForEach(CropTools.guides, id: \.0) { Text(LocalizedStringKey($0.1)).tag($0.0) }
                    }
                    .pickerStyle(.inline)
                    .labelsHidden()
                }
                if prefs.cropGuide == "triangle" || prefs.cropGuide == "spiral" {
                    MacIconButton(symbol: "arrow.triangle.2.circlepath", title: "Cycle Overlay Orientation") {
                        prefs.cropGuideOrientation = (prefs.cropGuideOrientation + 1) % 4
                    }
                }
            }
        }
    }

    /// Expand: the crop may reach past the photo, and Generate paints the new area.
    @ViewBuilder private var expand: some View {
        let on = model.editor.setting("crop", "expand").bool == true
        MacCheck(title: "Expand", isOn: Binding(get: { on }, set: { model.perform("crop.expand", ["on": .bool($0)]) }))
            .help(Text("Lets the crop reach past the photo's edges, to paint the new area"))
        if on { ExpandControls().padding(.top, 2) }
    }
}

/// Crop's Geometry: Upright (Auto, Level, Vertical, Full, Guided and its guides), the distortion and
/// perspective sliders, Constrain Crop.
struct MacGeometryControls: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let editor = model.editor
        let upright = editor.setting("geometry", "upright").string
        MacCaption(text: "Upright")
        MacSegmented(
            options: [("auto", "Auto"), ("level", "Level"), ("vertical", "Vertical"), ("full", "Full"), ("guided", "Guided")],
            isOn: { upright == $0 },
            choose: { mode in
                let on = upright == mode
                model.perform("geometry.upright", ["mode": .string(on ? "off" : mode)])
                // Guided: draw its guides on the photo
                editor.cropDraw = mode == "guided" && !on ? "guides" : nil
            }
        )
        if upright == "guided" {
            let n = editor.setting("geometry", "guides").array?.count ?? 0
            MacHint(text: Text("\(n) of 4 guides: drag along lines that should be vertical or horizontal."))
            HStack(spacing: 4) {
                MacButton(title: editor.cropDraw == "guides" ? "Done" : "Draw Guides") {
                    editor.cropDraw = editor.cropDraw == "guides" ? nil : "guides"
                }
                if n > 0 { MacButton(title: "Clear Guides") { model.perform("geometry.guides", ["guides": []]) } }
            }
        }
        Sliders(section: "optics") { $0.id == "optics.distortion" }
        Sliders(section: "geometry") { $0.id != "crop.angle" }
        MacCheck(title: "Constrain Crop", isOn: Binding(get: { editor.setting("geometry", "constrain_crop").bool == true }, set: {
            model.perform("develop.merge", ["settings": ["geometry": ["constrain_crop": .bool($0)]], "label": "Constrain Crop"])
        }))
    }
}
#endif
