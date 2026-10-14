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
        VStack(spacing: 0) {
            MacEditHeader()
            Divider()
            ScrollView {
                VStack(spacing: 0) {
                    MacProfileRow()
                    Divider()
                    MacSection(id: "light", title: "Light", open: true) { MacLight() }
                    MacSection(id: "color", title: "Color", open: true) { MacColor() }
                    MacSection(id: "effects", title: "Effects") { MacEffects() }
                    MacSection(id: "detail", title: "Detail") { Sliders(section: "detail") }
                    MacSection(id: "optics", title: "Optics") { Sliders(section: "optics") }
                    MacSection(id: "calibration", title: "Calibration") { Sliders(section: "calibration") }
                }
            }
        }
        .task { model.editor.want(Self.sections, model) }
    }
}

/// "Edit", and the three things Lightroom keeps at the top: Auto, B&W and HDR.
private struct MacEditHeader: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let editor = model.editor
        VStack(alignment: .leading, spacing: 10) {
            Text("Edit").font(.headline)
            HStack(spacing: 6) {
                Button {
                    Task { if await model.run("develop.auto") != nil { model.show(String(localized: "Auto settings applied")) } }
                } label: {
                    Text("Auto").frame(maxWidth: .infinity)
                }
                Toggle(isOn: Binding(get: { editor.isBW }, set: { _ in model.perform("develop.treatment") })) {
                    Text("B&W").frame(maxWidth: .infinity)
                }
                .toggleStyle(.button)
                Toggle(isOn: Binding(get: { editor.hdr }, set: { model.perform("develop.hdr", ["enabled": .bool($0)]) })) {
                    Text("HDR").frame(maxWidth: .infinity)
                }
                .toggleStyle(.button)
            }
            .controlSize(.small)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 14)
    }
}

/// "Profile  Camera Standard ▾" and a button that opens the profiles as live thumbnails.
private struct MacProfileRow: View {
    @Environment(AppModel.self) private var model
    @State private var menu: ProfilesMenu?
    @State private var browsing = false

    var body: some View {
        let current = model.editor.setting("profile", "id").string ?? "lc.color"
        HStack(spacing: 8) {
            Text("Profile").font(.callout).foregroundStyle(.secondary)
            Menu(menu?.name(of: current) ?? current) {
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
            .menuStyle(.button)
            .buttonStyle(.borderless)
            .fixedSize()
            Spacer()
            Button {
                browsing = true
            } label: {
                Image(systemName: "square.grid.2x2")
            }
            .buttonStyle(.borderless)
            .help(Text("Browse Profiles"))
            .popover(isPresented: $browsing, arrowEdge: .leading) {
                ScrollView { ProfileGroup().padding(16) }
                    .frame(width: 340, height: 460)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .task(id: model.settledRevision) {
            menu = try? await model.engine?.execute("profiles.menu", [:], as: ProfilesMenu.self)
        }
    }
}

/// A group of a Mac panel: its name with a chevron, open or closed (kept; `open` is how it starts), its
/// controls under it.
struct MacSection<Content: View>: View {
    @Environment(AppModel.self) private var model
    let id: String
    let title: LocalizedStringKey
    var open: Bool = false
    @ViewBuilder let content: () -> Content

    var body: some View {
        let open = model.prefs.isOpen(id, default: open)
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { model.prefs.toggleOpen(id, default: self.open) }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "chevron.down")
                        .font(.caption.weight(.semibold))
                        .rotationEffect(.degrees(open ? 0 : -90))
                        .frame(width: 12)
                    Text(title).font(.subheadline.weight(.semibold))
                    Spacer()
                }
                .padding(.vertical, 11)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(.isHeader)
            .accessibilityValue(open ? Text("Expanded") : Text("Collapsed"))
            if open {
                VStack(alignment: .leading, spacing: 2) { content() }
                    .padding(.bottom, 14)
            }
            Divider()
        }
        .padding(.horizontal, 16)
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
                        .font(.callout)
                        .frame(width: 26, height: 26)
                        .background(.quaternary, in: .rect(cornerRadius: 6))
                    Text(title).font(.callout)
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
        .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 10))
        .padding(.top, 8)
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
            Toggle("Visualize HDR", isOn: Binding(get: { model.editor.visualizeHdr }, set: { model.editor.visualizeHdr = $0 }))
                .font(.callout)
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
        HStack(spacing: 8) {
            Text("White Balance").font(.callout).foregroundStyle(.secondary)
            Menu(ColorGroup.modes.first { $0.0 == mode }?.1 ?? "Custom") {
                ForEach(ColorGroup.modes, id: \.0) { id, label in
                    Button {
                        model.perform("develop.wb", ["mode": .string(id)])
                    } label: {
                        if id == mode { Label(label, systemImage: "checkmark") } else { Text(label) }
                    }
                }
            }
            .menuStyle(.button)
            .buttonStyle(.borderless)
            .fixedSize()
            Spacer()
            Button {
                editor.arm(.whiteBalance, model)
            } label: {
                Image(systemName: "eyedropper")
                    .foregroundStyle(editor.picking == .whiteBalance ? Color.accentColor : .primary)
            }
            .buttonStyle(.borderless)
            .help(Text("White Balance Selector"))
        }
        .padding(.bottom, 4)
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
        Text("Vignette").font(.callout.weight(.semibold)).padding(.top, 8).accessibilityAddTraits(.isHeader)
        MacSegmented(selection: Binding(get: { model.editor.setting("vignette", "style").string ?? "highlight" }, set: { style in
            model.perform("develop.merge", ["settings": ["vignette": ["style": .string(style)]], "label": "Vignette Style"])
        }), options: EffectsGroup.styles)
        Sliders(section: "vignette")
        Text("Grain").font(.callout.weight(.semibold)).padding(.top, 8).accessibilityAddTraits(.isHeader)
        Sliders(section: "grain")
    }
}
#endif
