#if os(macOS)
import LightCraftCore
import SwiftUI

/// The Edit tool as Lightroom lays it out: a header (Edit, Auto, B&W, HDR), the profile, then the
/// groups one under another (Light, Color, Effects, Detail, Optics, Calibration), each open or
/// closed, with Curve, Color Mixer (and its point colours) and Color Grading as boxes inside them. The sliders
/// are the engine's control specs, as everywhere (`Sliders`): a new control appears without a change
/// here.
struct MacEditPanel: View {
    @Environment(AppModel.self) private var model

    /// Every engine section the panel's sliders come from.
    private static let sections = ["profile", "light", "color", "effects", "vignette", "grain", "detail", "optics", "calibration"]

    var body: some View {
        MacPanel(title: "Edit") {
            MacEditHeader()
            MacProfileRow()
            MacSection(id: "light", title: "Light", open: true) { MacLight() }
            MacSection(id: "color", title: "Color", open: true) { MacColor() }
            MacSection(id: "effects", title: "Effects") { MacEffects() }
            MacSection(id: "detail", title: "Detail") { Sliders(section: "detail") }
            MacSection(id: "optics", title: "Optics") { Sliders(section: "optics") }
            MacSection(id: "calibration", title: "Calibration") { Sliders(section: "calibration") }
        }
        .task { model.editor.want(Self.sections, model) }
    }
}

/// The three things Lightroom keeps at the top of Edit: Auto, B&W and HDR, as one bar of buttons.
private struct MacEditHeader: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let editor = model.editor
        HStack(spacing: 4) {
            MacButton(title: "Auto") {
                Task { if await model.run("develop.auto") != nil { model.show(String(localized: "Auto settings applied")) } }
            }
            MacButton(title: "B&W", on: editor.isBW) { model.perform("develop.treatment") }
            MacButton(title: "HDR", on: editor.hdr) { model.perform("develop.hdr", ["enabled": .bool(!editor.hdr)]) }
        }
        .padding(.horizontal, MacUI.inset)
        .padding(.vertical, 12)
        .overlay(alignment: .bottom) { Divider() }
    }
}

/// "Profile  Camera Standard ▾" and a button that opens the profiles as live thumbnails.
private struct MacProfileRow: View {
    @Environment(AppModel.self) private var model
    @State private var menu: ProfilesMenu?
    @State private var browsing = false

    var body: some View {
        let current = model.editor.setting("profile", "id").string ?? "lc.color"
        MacRow(label: "Profile") {
            HStack(spacing: 4) {
                MacMenuField(text: Text(menu?.name(of: current) ?? current)) {
                    ForEach(menu?.groups ?? [], id: \.name) { group in
                        Section(group.name) {
                            ForEach(group.profiles, id: \.id) { p in
                                Button {
                                    model.perform("develop.profile", ["id": .string(p.id)])
                                } label: {
                                    if p.id == current { Label(p.name, systemImage: "checkmark") } else { Text(p.name) }
                                }
                            }
                        }
                    }
                }
                MacIconButton(symbol: "square.grid.2x2", title: "Browse Profiles") { browsing = true }
                    .popover(isPresented: $browsing, arrowEdge: .leading) {
                        ScrollView { ProfileGroup().padding(16) }
                            .frame(width: 340, height: 460)
                    }
            }
        }
        .padding(.horizontal, MacUI.inset)
        .padding(.vertical, 8)
        .overlay(alignment: .bottom) { Divider() }
        .task(id: model.settledRevision) {
            menu = try? await model.engine?.execute("profiles.menu", [:], as: ProfilesMenu.self)
        }
    }
}

/// A group of a Mac panel that opens and closes (kept; `open` is how it starts): its name at the
/// left edge, as a plain group's, and a chevron at the right.
struct MacSection<Content: View>: View {
    @Environment(AppModel.self) private var model
    let id: String
    let title: LocalizedStringKey
    var open: Bool = false
    @ViewBuilder let content: () -> Content

    var body: some View {
        let open = model.prefs.isOpen(id, default: open)
        VStack(alignment: .leading, spacing: 4) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { model.prefs.toggleOpen(id, default: self.open) }
            } label: {
                HStack(spacing: 8) {
                    Text(title).font(.inspectorTitle)
                    Spacer()
                    Image(systemName: "chevron.down")
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(open ? 0 : -90))
                }
                .frame(minHeight: 20)
                .padding(.bottom, open ? 6 : 0)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(.isHeader)
            .accessibilityValue(open ? Text("Expanded") : Text("Collapsed"))
            if open {
                VStack(alignment: .leading, spacing: 4) { content() }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, MacUI.inset)
        .padding(.vertical, 12)
        .overlay(alignment: .bottom) { Divider() }
    }
}

/// A box inside a group (Lightroom's Curve, Color Mixer…): an icon, the name and a chevron; the
/// controls open under it, and the engine sections they need are read when it first opens.
struct MacBox<Content: View>: View {
    @Environment(AppModel.self) private var model
    let id: String
    let title: LocalizedStringKey
    let symbol: String
    var sections: [String] = []
    var open: Bool = false
    @ViewBuilder let content: () -> Content

    var body: some View {
        let open = model.prefs.isOpen(id, default: open)
        VStack(alignment: .leading, spacing: 8) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { model.prefs.toggleOpen(id, default: self.open) }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: symbol)
                        .font(.inspector)
                        .frame(width: 24, height: 24)
                        .background(MacUI.fill, in: .rect(cornerRadius: MacUI.radius))
                    Text(title).font(.inspector)
                    Spacer()
                    Image(systemName: "chevron.down")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(open ? 0 : -90))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(open ? Text("Expanded") : Text("Collapsed"))
            if open { content() }
        }
        .padding(8)
        .background(MacUI.fill, in: .rect(cornerRadius: 8))
        .padding(.top, 4)
        .task(id: open) { if open, !sections.isEmpty { model.editor.want(sections, model) } }
    }
}

/// Light: Exposure to Blacks (HDR's range when HDR is on), and the Curve.
private struct MacLight: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let hdr = model.editor.hdr
        if hdr {
            Sliders(section: "light") { $0.id == "hdr.maxEv" }
            MacCheck(title: "Visualize HDR", isOn: Binding(get: { model.editor.visualizeHdr }, set: { model.editor.visualizeHdr = $0 }))
        }
        Sliders(section: "light") { !$0.id.hasPrefix("hdr.") }
        if hdr {
            MacBox(id: "sdr", title: "SDR Rendition", symbol: "sun.min") {
                Sliders(section: "light") { $0.id.hasPrefix("hdr.sdr") }
            }
        }
        MacBox(id: "curve", title: "Curve", symbol: "point.topleft.down.to.point.bottomright.curvepath", sections: ["curve"]) {
            CurveEditor()
        }
    }
}

/// Color: the white balance preset and its picker, Temp, Tint, Vibrance, Saturation, then the
/// Color Mixer (with the point colours) and Color Grading boxes.
private struct MacColor: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let editor = model.editor
        let mode = editor.setting("wb", "mode").string ?? "asShot"
        MacRow(label: "White Balance") {
            HStack(spacing: 4) {
                MacMenuField(text: Text(ColorGroup.modes.first { $0.0 == mode }?.1 ?? "Custom")) {
                    ForEach(ColorGroup.modes, id: \.0) { id, label in
                        Button {
                            model.perform("develop.wb", ["mode": .string(id)])
                        } label: {
                            if id == mode { Label(label, systemImage: "checkmark") } else { Text(label) }
                        }
                    }
                }
                MacIconButton(symbol: "eyedropper", title: "White Balance Selector", on: editor.picking == .whiteBalance) {
                    editor.arm(.whiteBalance, model)
                }
            }
        }
        Sliders(section: "color")
        // (the Mixer has the point colours too: its eyedropper)
        MacBox(id: "mixer", title: editor.isBW ? "B&W Mixer" : "Color Mixer", symbol: "paintpalette", sections: ["mixer", "bwMix", "pointColor"]) {
            MixerView()
        }
        MacBox(id: "grading", title: "Color Grading", symbol: "circle.hexagongrid", sections: ["grading"]) {
            GradingView()
        }
    }
}

/// Effects: Texture, Clarity, Dehaze, then the vignette (its style) and the grain.
private struct MacEffects: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Sliders(section: "effects")
        MacCaption(text: "Vignette")
        MacSegmented(selection: Binding(get: { model.editor.setting("vignette", "style").string ?? "highlight" }, set: { style in
            model.perform("develop.merge", ["settings": ["vignette": ["style": .string(style)]], "label": "Vignette Style"])
        }), options: EffectsGroup.styles)
        Sliders(section: "vignette")
        MacCaption(text: "Grain")
        Sliders(section: "grain")
    }
}
#endif
