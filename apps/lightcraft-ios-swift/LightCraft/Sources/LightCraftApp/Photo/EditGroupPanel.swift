#if os(iOS)
import LightCraftCore
import SwiftUI

/// The Edit tool's open group: its sliders (from the engine's control specs, so a new control
/// appears without a change here) and the group's own rows (profile, white balance, B&W) and
/// flyouts (Tone Curve, Color Mixer, Color Grading).
struct EditGroupPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let group = EditGroup(rawValue: model.prefs.editGroup) ?? .light
        VStack(alignment: .leading, spacing: 4) {
            switch group {
            case .profile: ProfileGroup()
            case .light: LightGroup()
            case .color: ColorGroup()
            case .effects: EffectsGroup()
            default: Sliders(section: group.sections.first ?? "")
            }
        }
        .task(id: group) { model.editor.want(group.sections, model) }
    }
}

/// Every slider of an engine section, optionally only some.
struct Sliders: View {
    @Environment(AppModel.self) private var model
    let section: String
    var only: ((ControlSpec) -> Bool)? = nil

    var body: some View {
        let editor = model.editor
        ForEach(editor.specs(section).filter { only?($0) ?? true }) { spec in
            ControlSlider(
                spec: spec,
                value: editor.value(spec),
                set: { editor.set(spec, $0, model) },
                editing: { editor.editing(spec, $0, model) },
                reset: { editor.reset(spec, model) }
            )
        }
    }
}

/// A heading that opens more controls under it (Lightroom's flyouts).
struct Flyout<Content: View>: View {
    let title: LocalizedStringKey
    var symbol: String? = nil
    @Binding var open: Bool
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                withAnimation(.snappy) { open.toggle() }
            } label: {
                HStack {
                    if let symbol { Image(systemName: symbol).foregroundStyle(.secondary) }
                    Text(title).font(.subheadline.weight(.semibold))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .padding(.vertical, 10)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(.isHeader)
            .accessibilityValue(open ? "Expanded" : "Collapsed")
            if open {
                content()
                    .transition(.opacity.combined(with: .move(edge: .top)))
            }
            Divider()
        }
    }
}

/// Profile: the profiles of a group as live thumbnails of the photo (favourites and recent first),
/// a tap applies one; and how strongly the applied one works.
private struct ProfileGroup: View {
    @Environment(AppModel.self) private var model
    @State private var menu: ProfilesMenu?
    @State private var group = ""

    var body: some View {
        let current = model.editor.setting("profile", "id").string ?? "lc.color"
        let name = menu?.name(of: current) ?? current
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(name).font(.headline)
                Spacer()
                Picker("Group", selection: $group) {
                    if menu?.favorites.isEmpty == false { Text("Favorites").tag("★") }
                    if menu?.recent.isEmpty == false { Text("Recent").tag("recent") }
                    ForEach(menu?.groups ?? [], id: \.name) { Text($0.name).tag($0.name) }
                }
                .pickerStyle(.menu)
            }
            if current != "lc.color" {
                Sliders(section: "profile")
            }
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 84), spacing: 8)], spacing: 10) {
                ForEach(shown, id: \.id) { p in
                    Button {
                        model.perform("develop.profile", ["id": .string(p.id)])
                    } label: {
                        VStack(spacing: 4) {
                            VariantPreview(which: ["profile": .string(p.id)])
                                .aspectRatio(1, contentMode: .fit)
                                .clipShape(.rect(cornerRadius: 10))
                                .overlay {
                                    if p.id == current { RoundedRectangle(cornerRadius: 10).strokeBorder(Color.accentColor, lineWidth: 2) }
                                }
                                .overlay(alignment: .topTrailing) {
                                    if menu?.isFavorite(p.id) == true { Image(systemName: "star.fill").font(.caption2).padding(5).foregroundStyle(.yellow) }
                                }
                            Text(p.name).font(.caption2.weight(.medium)).lineLimit(1)
                        }
                    }
                    .buttonStyle(.plain)
                    .contextMenu {
                        let fav = menu?.isFavorite(p.id) == true
                        Button(fav ? "Remove from Favorites" : "Add to Favorites", systemImage: fav ? "star.slash" : "star") {
                            model.perform("profile.favorite", ["id": .string(p.id), "favorite": .bool(!fav)])
                            Task { await load() }
                        }
                    }
                    .accessibilityAddTraits(p.id == current ? .isSelected : [])
                }
            }
        }
        .task(id: model.state.catalogRevision) { await load() }
    }

    private var shown: [ProfilesMenu.Item] {
        guard let menu else { return [] }
        switch group {
        case "★": return menu.favorites
        case "recent": return menu.recent
        default: return menu.groups.first { $0.name == group }?.profiles ?? menu.groups.first?.profiles ?? []
        }
    }

    private func load() async {
        guard let engine = model.engine else { return }
        menu = try? await engine.execute("profiles.menu", [:], as: ProfilesMenu.self)
        if group.isEmpty, let menu {
            let current = model.editor.setting("profile", "id").string ?? ""
            group = menu.groups.first { g in g.profiles.contains { $0.id == current } }?.name ?? menu.groups.first?.name ?? ""
        }
    }
}

/// `profiles.menu`.
nonisolated struct ProfilesMenu: Decodable {
    struct Item: Decodable, Hashable {
        var id: String
        var name: String
        var favorite: Bool?
    }
    struct Group: Decodable {
        var name: String
        var profiles: [Item]
    }
    var favorites: [Item]
    var recent: [Item]
    var groups: [Group]

    func name(of id: String) -> String? {
        groups.lazy.flatMap(\.profiles).first { $0.id == id }?.name
    }

    func isFavorite(_ id: String) -> Bool {
        favorites.contains { $0.id == id }
    }

    enum CodingKeys: String, CodingKey { case favorites, recent, groups }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        favorites = (try? c.decode([Item].self, forKey: .favorites)) ?? []
        recent = (try? c.decode([Item].self, forKey: .recent)) ?? []
        groups = (try? c.decode([Group].self, forKey: .groups)) ?? []
    }
}

/// Light: HDR, exposure to blacks, and the Tone Curve.
private struct LightGroup: View {
    @Environment(AppModel.self) private var model
    @State private var curve = false
    @State private var sdr = false

    var body: some View {
        let hdr = model.editor.hdr
        Toggle(isOn: Binding(get: { hdr }, set: { model.perform("develop.hdr", ["enabled": .bool($0)]) })) {
            Label("HDR", systemImage: "sun.max.trianglebadge.exclamationmark")
        }
        .font(.subheadline)
        if hdr {
            Sliders(section: "light") { $0.id == "hdr.maxEv" }
            Toggle("Visualize HDR", isOn: Binding(get: { model.editor.visualizeHdr }, set: { model.editor.visualizeHdr = $0 }))
                .font(.subheadline)
        }
        Sliders(section: "light") { !$0.id.hasPrefix("hdr.") }
        if hdr {
            Flyout(title: "SDR Rendition", open: $sdr) {
                Sliders(section: "light") { $0.id.hasPrefix("hdr.sdr") }
            }
        }
        Flyout(title: "Curve", symbol: "point.topleft.down.to.point.bottomright.curvepath", open: $curve) {
            CurveEditor()
        }
        .onChange(of: curve) { _, open in if open { model.editor.want(["curve"], model) } }
    }
}

/// Color: white balance (preset, picker), B&W, Temp and Tint, Vibrance and Saturation, then the Color
/// Mixer, Point Color and Color Grading.
private struct ColorGroup: View {
    @Environment(AppModel.self) private var model
    @State private var mixer = false
    @State private var grading = false
    @State private var point = false

    private static let modes: [(String, LocalizedStringKey)] = [
        ("asShot", "As Shot"), ("auto", "Auto"), ("daylight", "Daylight"), ("cloudy", "Cloudy"), ("shade", "Shade"),
        ("tungsten", "Tungsten"), ("fluorescent", "Fluorescent"), ("flash", "Flash"),
    ]

    var body: some View {
        let editor = model.editor
        let mode = editor.setting("wb", "mode").string ?? "asShot"
        HStack {
            Text("White Balance").font(.subheadline).foregroundStyle(.secondary)
            Spacer()
            Menu {
                ForEach(Self.modes, id: \.0) { id, label in
                    Button {
                        model.perform("develop.wb", ["mode": .string(id)])
                    } label: {
                        if id == mode { Label(label, systemImage: "checkmark") } else { Text(label) }
                    }
                }
            } label: {
                HStack(spacing: 4) {
                    Text(Self.modes.first { $0.0 == mode }?.1 ?? "Custom").font(.subheadline.weight(.semibold))
                    Image(systemName: "chevron.up.chevron.down").font(.caption)
                }
            }
            Button {
                model.editor.picking = .whiteBalance
                model.show(String(localized: "Tap something white or grey in the photo"))
            } label: {
                Image(systemName: "eyedropper")
            }
            .accessibilityLabel("White Balance Selector")
            Toggle("B&W", isOn: Binding(get: { editor.isBW }, set: { _ in model.perform("develop.treatment") }))
                .toggleStyle(.button)
                .font(.subheadline.weight(.semibold))
        }
        .padding(.vertical, 4)
        Sliders(section: "color")
        Flyout(title: editor.isBW ? "B&W Mixer" : "Color Mixer", symbol: "paintpalette", open: $mixer) {
            MixerView()
        }
        .onChange(of: mixer) { _, open in if open { editor.want(["mixer", "bwMix"], model) } }
        if !editor.isBW {
            Flyout(title: "Point Color", symbol: "eyedropper.halffull", open: $point) {
                PointColorView()
            }
            .onChange(of: point) { _, open in if open { editor.want(["pointColor"], model) } }
        }
        Flyout(title: "Color Grading", symbol: "circle.hexagongrid", open: $grading) {
            GradingView()
        }
        .onChange(of: grading) { _, open in if open { editor.want(["grading"], model) } }
    }
}

/// Effects: Texture, Clarity, Dehaze, then the vignette (its style) and grain.
private struct EffectsGroup: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Sliders(section: "effects")
        Text("Vignette").font(.subheadline.weight(.semibold)).padding(.top, 8)
        Picker("Style", selection: Binding(get: { model.editor.setting("vignette", "style").string ?? "highlight" }, set: { style in
            model.perform("develop.merge", ["settings": ["vignette": ["style": .string(style)]], "label": "Vignette Style"])
        })) {
            Text("Highlight").tag("highlight")
            Text("Color").tag("color")
            Text("Paint").tag("paint")
        }
        .pickerStyle(.segmented)
        Sliders(section: "vignette")
        Text("Grain").font(.subheadline.weight(.semibold)).padding(.top, 8)
        Sliders(section: "grain")
    }
}

/// The eight colour bands' Hue, Saturation and Luminance (or, in black and white, how light each
/// band turns out), one band at a time.
struct MixerView: View {
    @Environment(AppModel.self) private var model
    @State private var band = 0
    @State private var all = false

    static let names = ["red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"]

    var body: some View {
        let editor = model.editor
        if editor.isBW {
            Button("Auto") { model.perform("develop.autoBwMix") }
                .buttonStyle(.bordered)
            Sliders(section: "bwMix")
        } else {
            HStack(spacing: 0) {
                ForEach(0..<8, id: \.self) { i in
                    Button {
                        band = i
                    } label: {
                        Circle()
                            .fill(ControlSlider.bands[i])
                            .frame(width: 28, height: 28)
                            .overlay { if band == i { Circle().strokeBorder(.primary, lineWidth: 2).padding(-4) } }
                            .frame(maxWidth: .infinity, minHeight: 44)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(LocalizedStringKey(Self.names[i].capitalized))
                    .accessibilityAddTraits(band == i ? .isSelected : [])
                }
            }
            .haptic(.selection, trigger: band)
            Sliders(section: "mixer") { $0.id.hasPrefix("mixer.\(Self.names[band]).") }
        }
    }
}

/// Color Grading: one wheel at a time (Shadows, Midtones, Highlights, Global) — drag the puck for
/// hue and saturation — then its luminance, Blending and Balance.
struct GradingView: View {
    @Environment(AppModel.self) private var model
    @State private var region = "midtones"

    var body: some View {
        Picker("Region", selection: $region) {
            Text("Shadows").tag("shadows")
            Text("Midtones").tag("midtones")
            Text("Highlights").tag("highlights")
            Text("Global").tag("global")
        }
        .pickerStyle(.segmented)
        GradingWheel(region: region)
            .frame(maxWidth: 210)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
        Sliders(section: "grading") { $0.id == "grading.\(region).lum" }
        Sliders(section: "grading") { $0.id == "grading.blending" || $0.id == "grading.balance" }
    }
}

/// A colour wheel: the puck's angle is the hue, its distance from the centre the saturation.
/// Dragging it is one undo step; a double tap puts it back in the middle.
struct GradingWheel: View {
    @Environment(AppModel.self) private var model
    let region: String
    @State private var dragging = false

    var body: some View {
        let editor = model.editor
        let hueSpec = editor.spec("grading.\(region).hue")
        let satSpec = editor.spec("grading.\(region).sat")
        let hue = hueSpec.map { editor.value($0) } ?? 0
        let sat = satSpec.map { editor.value($0) } ?? 0
        GeometryReader { geo in
            let r = min(geo.size.width, geo.size.height) / 2
            let c = CGPoint(x: geo.size.width / 2, y: geo.size.height / 2)
            let angle = hue * .pi / 180
            let d = CGFloat(sat / 100) * r
            ZStack {
                Circle()
                    .fill(AngularGradient(colors: stride(from: 0.0, through: 1.0, by: 1.0 / 12).map { Color(hue: $0, saturation: 0.85, brightness: 0.95) }, center: .center, angle: .degrees(0)))
                    .overlay(Circle().fill(RadialGradient(colors: [.gray, .gray.opacity(0)], center: .center, startRadius: 0, endRadius: r)))
                Circle()
                    .fill(.white)
                    .frame(width: 22, height: 22)
                    .shadow(radius: 2)
                    .position(x: c.x + cos(angle) * d, y: c.y - sin(angle) * d)
            }
            .contentShape(Circle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { g in
                        let dx = g.location.x - c.x
                        let dy = c.y - g.location.y
                        var h = atan2(dy, dx) * 180 / .pi
                        if h < 0 { h += 360 }
                        let s = min(Double(hypot(dx, dy) / r), 1) * 100
                        editor.setMany(["grading.\(region).hue": h.rounded(), "grading.\(region).sat": s.rounded()], label: "Color Grading", began: !dragging, ended: false, model)
                        dragging = true
                    }
                    .onEnded { _ in
                        editor.setMany(["grading.\(region).hue": editor.spec("grading.\(region).hue").map { editor.value($0) } ?? 0, "grading.\(region).sat": editor.spec("grading.\(region).sat").map { editor.value($0) } ?? 0], label: "Color Grading", began: false, ended: true, model)
                        dragging = false
                    }
            )
            .onTapGesture(count: 2) {
                editor.setMany(["grading.\(region).hue": 0, "grading.\(region).sat": 0], label: "Color Grading", began: true, ended: true, model)
            }
        }
        .aspectRatio(1, contentMode: .fit)
        .accessibilityElement()
        .accessibilityLabel("\(region.capitalized) wheel")
        .accessibilityValue("Hue \(Int(hue)), saturation \(Int(sat))")
    }
}
#endif
