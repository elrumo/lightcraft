#if os(macOS)
import LightCraftCore
import SwiftUI

/// Remove as Lightroom lays it out: Remove, Heal or Clone, how the spot is chosen, the brush's size
/// (and its feather and opacity when it heals or clones), the Generative AI switch, and what the
/// photo's spots are (Find Dust, Visualize, delete all) in the header's menu.
struct MacRemovePanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var editor = model.editor
        let spots = editor.setting("spots").array ?? []
        MacPanel(title: "Remove") {
            MacIconMenu(symbol: "ellipsis", title: "More") {
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
            }
        } content: {
            MacGroup {
                MacSegmented(selection: $editor.removeMode, options: [("remove", "Remove"), ("heal", "Heal"), ("clone", "Clone")])
                    .padding(.bottom, 4)
                if editor.removeMode == "remove" {
                    MacRow(label: "Select with") {
                        MacSegmented(selection: $editor.removeTap, options: [(false, "Brush"), (true, "Tap")])
                    }
                }
            }
            MacGroup(title: "Brush") {
                if editor.removeMode == "remove" && editor.removeTap {
                    // how far past the tapped object's edge the spot reaches (the selected tapped spot's own, as it is)
                    MacSliderRow(spec: ControlSpec(id: "remove.reach", label: "Size", section: "remove", min: 1, max: 30, default: 4, step: 1, decimals: 0),
                                 value: (model.tappedSpot?.spot["size"].double ?? editor.removeTapSize) * 1000, set: { model.tapSizeChanged($0 / 1000) },
                                 editing: { model.tapSizeEditing($0) }, reset: { model.tapSizeChanged(0.004) })
                } else {
                    MacSliderRow(spec: ControlSpec(id: "remove.size", label: "Size", section: "remove", min: 1, max: 250, default: 20, step: 1, decimals: 0),
                                 value: editor.removeSize * 1000, set: { editor.removeSize = $0 / 1000 }, editing: { _ in }, reset: { editor.removeSize = 0.02 })
                }
                if editor.removeMode != "remove" {
                    MacSliderRow(spec: ControlSpec(id: "remove.feather", label: "Feather", section: "remove", min: 0, max: 100, default: 50, step: 1, decimals: 0),
                                 value: editor.removeFeather, set: { editor.removeFeather = $0 }, editing: { _ in }, reset: { editor.removeFeather = 50 })
                    MacSliderRow(spec: ControlSpec(id: "remove.opacity", label: "Opacity", section: "remove", min: 0, max: 100, default: 100, step: 1, decimals: 0),
                                 value: editor.removeOpacity, set: { editor.removeOpacity = $0 }, editing: { _ in }, reset: { editor.removeOpacity = 100 })
                }
                if editor.visualizeSpots {
                    MacSliderRow(spec: ControlSpec(id: "remove.spotsThreshold", label: "Threshold", section: "remove", min: 0, max: 100, default: 50, step: 1, decimals: 0),
                                 value: editor.spotsThreshold, set: { editor.spotsThreshold = $0 }, editing: { _ in }, reset: { editor.spotsThreshold = 50 })
                }
            }
            if editor.removeMode == "remove" {
                MacGroup { GenerativeAISwitch() }
            }
            MacGroup {
                if spots.isEmpty {
                    if editor.removeMode == "remove" && editor.removeTap {
                        MacHint(text: Text("Tap an object to take it away."))
                    } else {
                        MacHint(text: Text("Paint over what to take away."))
                    }
                } else {
                    MacHint(text: Text("^[\(spots.count) spot](inflect: true) · tap a pin for more"))
                }
            }
        }
        // what appears makes room smoothly, and settles in
        .animation(.snappy, value: editor.removeMode)
        .animation(.snappy, value: editor.removeTap)
        .animation(.snappy, value: editor.visualizeSpots)
        .animation(.snappy, value: spots.isEmpty)
    }
}

/// Red Eye and Pet Eye: which, then the chosen eye's controls (or what to do to make one).
struct MacRedEyePanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var editor = model.editor
        let eyes = editor.setting("red_eye").array ?? []
        MacPanel(title: "Red Eye") {
            MacGroup {
                MacSegmented(selection: $editor.eyePet, options: [(false, "Red Eye"), (true, "Pet Eye")])
                    .padding(.bottom, 4)
                if eyes.indices.contains(editor.eye) {
                    Sliders(section: "redEye") { $0.id.hasPrefix("redEye.\(editor.eye).") }
                    if eyes[editor.eye]["pet"].bool == true {
                        MacCheck(title: "Catchlight", isOn: Binding(get: { !eyes[editor.eye]["catchlight"].isNull }, set: { on in
                            model.perform("redeye.catchlight", ["index": .int(Int64(editor.eye)), "on": .bool(on)])
                        }))
                    }
                    MacButton(title: "Delete", symbol: "trash", wide: false) {
                        model.perform("redeye.delete", ["index": .int(Int64(editor.eye))])
                        editor.eye = max(editor.eye - 1, 0)
                    }
                    .padding(.top, 4)
                } else {
                    MacHint(text: Text("Drag a box around an eye."))
                }
            }
        }
        .onAppear { editor.want(["redEye"], model) }
    }
}
#endif
