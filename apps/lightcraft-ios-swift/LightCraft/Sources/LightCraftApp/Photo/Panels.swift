#if os(iOS)
import LightCraftCore
import SwiftUI

/// Presets: the groups (favourites first), each preset one tap; the last one applied gets an
/// Amount slider (0–200 %). Long press for favourite, rename, delete.
struct PresetsPanel: View {
    @Environment(AppModel.self) private var model
    @State private var presets: [PresetInfo] = []
    @State private var groups: [String] = []
    @State private var applied: String?
    @State private var amount: Double = 100
    @State private var group: String = ""
    @State private var importing = false
    @State private var review: JSONValue?
    @State private var exported: [URL]?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Picker("Group", selection: $group) {
                    if presets.contains(where: \.favorite) { Text("Favorites").tag("★") }
                    ForEach(groups, id: \.self) { Text($0).tag($0) }
                    if presets.contains(where: { !$0.builtin }) { Text("User Presets").tag("user") }
                }
                .pickerStyle(.menu)
                Spacer()
                Menu {
                    Button("Create Preset", systemImage: "plus") { model.sheet = .createPreset }
                    Button("Import Presets…", systemImage: "square.and.arrow.down") { importing = true }
                    Button("Export Presets…", systemImage: "square.and.arrow.up") { exportPresets() }
                        .disabled(!presets.contains { !$0.builtin })
                } label: {
                    Label("More", systemImage: "plus.circle")
                }
                .labelStyle(.iconOnly)
            }
            if let applied, presets.contains(where: { $0.id == applied }) {
                ControlSlider(
                    spec: ControlSpec.amount,
                    value: amount,
                    label: String(localized: "Amount"),
                    set: { amount = $0 },
                    editing: { began in if !began { apply(applied, amount: amount, replacing: true) } },
                    reset: {
                        amount = 100
                        apply(applied, amount: 100, replacing: true)
                    }
                )
            }
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 84), spacing: 8)], spacing: 10) {
                ForEach(shown) { preset in
                    Button {
                        applied = preset.id
                        amount = 100
                        apply(preset.id, amount: 100, replacing: false)
                    } label: {
                        VStack(spacing: 4) {
                            VariantPreview(which: ["preset": .string(preset.id)])
                                .aspectRatio(1, contentMode: .fit)
                                .clipShape(.rect(cornerRadius: 10))
                                .overlay {
                                    if applied == preset.id { RoundedRectangle(cornerRadius: 10).strokeBorder(Color.accentColor, lineWidth: 2) }
                                }
                                .overlay(alignment: .topTrailing) {
                                    if preset.favorite { Image(systemName: "star.fill").font(.caption2).padding(5).foregroundStyle(.yellow) }
                                }
                            Text(preset.name)
                                .font(.caption2.weight(.medium))
                                .lineLimit(1)
                        }
                    }
                    .buttonStyle(.plain)
                    .contextMenu {
                        Button(preset.favorite ? "Remove from Favorites" : "Add to Favorites", systemImage: preset.favorite ? "star.slash" : "star") {
                            model.perform("preset.favorite", ["id": .string(preset.id), "favorite": .bool(!preset.favorite)])
                            Task { await load() }
                        }
                        if !preset.builtin {
                            Button("Update with Current Settings", systemImage: "arrow.triangle.2.circlepath") { model.perform("preset.update", ["id": .string(preset.id)]) }
                            Button("Delete", systemImage: "trash", role: .destructive) {
                                model.perform("preset.delete", ["id": .string(preset.id)])
                                Task { await load() }
                            }
                        }
                    }
                }
            }
        }
        .task { await load() }
        .onChange(of: model.sheet == nil) { _, closed in
            // a preset made in Create Preset
            if closed { Task { await load() } }
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: PresetFiles.types, allowsMultipleSelection: true) { result in
            guard case .success(let urls) = result, let engine = model.engine else { return }
            let paths = PresetFiles.stage(urls)
            Task {
                do {
                    review = try await engine.execute("preset.importReview", ["paths": .array(paths.map { .string($0) })])
                } catch {
                    model.show(error.localizedDescription, error: true)
                }
            }
        }
        .sheet(item: Binding(get: { review.map(ReviewItem.init) }, set: { if $0 == nil { review = nil } }), onDismiss: { Task { await load() } }) {
            PresetImportSheet(review: $0.review)
        }
        .sheet(item: Binding(get: { exported.map { ExportedFiles(urls: $0) } }, set: { if $0 == nil { exported = nil } })) {
            ActivityView(urls: $0.urls) { exported = nil }
                .ignoresSafeArea()
        }
    }

    private struct ReviewItem: Identifiable {
        let review: JSONValue
        let id = UUID()
    }

    private struct ExportedFiles: Identifiable {
        let urls: [URL]
        var id: String { urls.map(\.path).joined() }
    }

    /// Your presets (the group shown, if it is one of yours) as a `.lcpreset` file, to share or save.
    private func exportPresets() {
        let mine = presets.filter { !$0.builtin }
        let one = group != "★" && group != "user" && mine.contains { $0.group == group }
        let name = one ? group : String(localized: "LightCraft Presets")
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("Exports", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let file = dir.appendingPathComponent(name.replacingOccurrences(of: "/", with: "-")).appendingPathExtension("lcpreset")
        Task {
            var p: [String: JSONValue] = ["path": .string(file.path)]
            if one { p["group"] = .string(group) }
            if await model.run("preset.export", .object(p)) != nil { exported = [file] }
        }
    }

    private var shown: [PresetInfo] {
        switch group {
        case "★": presets.filter(\.favorite)
        case "user": presets.filter { !$0.builtin }
        default: presets.filter { $0.group == group }
        }
    }

    private func load() async {
        guard let engine = model.engine else { return }
        presets = (try? await engine.execute("presets.list", [:], as: [PresetInfo].self)) ?? []
        groups = (try? await engine.execute("presets.groups", [:], as: [String].self)) ?? []
        if group.isEmpty { group = groups.first ?? "" }
    }

    /// Apply a preset; changing its amount replaces the last application (one undo step each).
    private func apply(_ id: String, amount: Double, replacing: Bool) {
        Task {
            if replacing { await model.run("edit.undo") }
            await model.run("preset.apply", ["id": .string(id), "amount": .double(amount)])
        }
    }
}

/// A preview of the active photo with a preset, a profile or a version (asked for again when the
/// photo or its edits change).
struct VariantPreview: View {
    @Environment(AppModel.self) private var model
    let which: JSONValue
    @State private var slot: String?

    var body: some View {
        Color(uiColor: .tertiarySystemFill)
            .overlay {
                if let slot, let p = model.images.pictures[slot] {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFill()
                }
            }
            .clipped()
            .task(id: Key(which: which, photo: model.state.active, revision: model.state.catalogRevision)) {
                if model.state.interaction == nil, let s = await model.requestVariant(which) { slot = s }
            }
    }

    private nonisolated struct Key: Hashable {
        let which: JSONValue
        let photo: UInt64?
        let revision: UInt64
    }
}

extension ControlSpec {
    /// A preset's amount (0–200 %).
    static let amount = ControlSpec(id: "preset.amount", label: "Amount", section: "presets", min: 0, max: 200, default: 100, step: 1, decimals: 0)
}

/// Info: the photo's file, camera and exposure, where it was taken, its title, caption and
/// keywords (editable), and its rating, flag and label.
struct InfoPanel: View {
    @Environment(AppModel.self) private var model
    @State private var detail: PhotoDetail?
    @State private var title = ""
    @State private var caption = ""
    @FocusState private var focus: Field?

    private enum Field { case title, caption }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let d = detail {
                VStack(alignment: .leading, spacing: 4) {
                    Text(d.fileName).font(.headline)
                    if let c = d.captured { Text(Self.date(c)).font(.subheadline).foregroundStyle(.secondary) }
                    Text("\(d.width) × \(d.height)\(d.fileSize.map { " · " + ByteCountFormatter.string(fromByteCount: Int64($0), countStyle: .file) } ?? "")\(d.format.map { " · " + $0 } ?? "")")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                if let m = d.meta {
                    VStack(alignment: .leading, spacing: 2) {
                        if let cam = m.camera, !cam.isEmpty { Label(cam, systemImage: "camera").font(.subheadline) }
                        if let lens = m.lens, !lens.isEmpty { Label(lens, systemImage: "camera.aperture").font(.subheadline) }
                        let exposure = [m.shutter, m.aperture.map { String(format: "ƒ/%.1f", $0) }, m.iso.map { "ISO \(Int($0))" }, m.focalMm.map { "\(Int($0)) mm" }].compactMap { $0 }
                        if !exposure.isEmpty { Text(exposure.joined(separator: "  ·  ")).font(.footnote.monospacedDigit()).foregroundStyle(.secondary) }
                    }
                    let place = [m.location, m.city, m.state, m.country].compactMap { $0 }.filter { !$0.isEmpty }
                    if !place.isEmpty || m.gps != nil {
                        Button {
                            model.path.append(.map)
                        } label: {
                            Label(place.isEmpty ? String(localized: "Show on Map") : place.joined(separator: ", "), systemImage: "mappin.and.ellipse")
                                .font(.subheadline)
                        }
                    }
                }
                ratingRow(d)
                TextField("Title", text: $title)
                    .focused($focus, equals: .title)
                    .onSubmit { save("title", title) }
                    .textFieldStyle(.roundedBorder)
                TextField("Caption", text: $caption, axis: .vertical)
                    .focused($focus, equals: .caption)
                    .lineLimit(1...4)
                    .onSubmit { save("caption", caption) }
                    .textFieldStyle(.roundedBorder)
                Button {
                    model.sheet = .keywords
                } label: {
                    LabeledContent("Keywords") {
                        Text(d.meta?.keywords?.joined(separator: ", ") ?? "").lineLimit(2)
                    }
                }
                .buttonStyle(.plain)
                Button("All Metadata…", systemImage: "list.bullet.rectangle") { model.sheet = .allMetadata }
                    .font(.subheadline)
            } else {
                ProgressView()
            }
        }
        .onChange(of: focus) { old, _ in
            if old == .title { save("title", title) }
            if old == .caption { save("caption", caption) }
        }
        .task(id: Key(photo: model.state.active, revision: model.state.catalogRevision)) { await load() }
    }

    private nonisolated struct Key: Hashable {
        let photo: UInt64?
        let revision: UInt64
    }

    private func ratingRow(_ d: PhotoDetail) -> some View {
        HStack(spacing: 4) {
            ForEach(1...5, id: \.self) { star in
                Button {
                    model.perform("photo.rate", ["ids": [.int(Int64(clamping: d.id))], "rating": .int(Int64(star == d.rating ? 0 : star))])
                } label: {
                    Image(systemName: star <= (d.rating ?? 0) ? "star.fill" : "star")
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("\(star) stars")
            }
            Spacer()
            Menu {
                FlagItems(ids: [d.id])
            } label: {
                Image(systemName: d.flag == "pick" ? "flag.fill" : d.flag == "reject" ? "flag.slash.fill" : "flag")
            }
            .accessibilityLabel("Flag")
            Menu {
                LabelItems(ids: [d.id])
            } label: {
                Image(systemName: "circle.fill").foregroundStyle(d.label.map(LabelColor.color) ?? .secondary)
            }
            .accessibilityLabel("Color Label")
        }
        .font(.title3)
    }

    private func load() async {
        guard let engine = model.engine, let r = try? await engine.execute("photo.inspect") else { return }
        detail = try? r.decode(PhotoDetail.self)
        if focus == nil {
            title = detail?.meta?.title ?? ""
            caption = detail?.meta?.caption ?? ""
        }
    }

    private func save(_ field: String, _ value: String) {
        guard let d = detail else { return }
        let old = field == "title" ? d.meta?.title ?? "" : d.meta?.caption ?? ""
        guard old != value else { return }
        model.perform("photo.setMeta", .object(["ids": [.int(Int64(clamping: d.id))], field: .string(value)]))
    }

    static func date(_ iso: String) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = iso.count > 10 ? "yyyy-MM-dd'T'HH:mm:ss" : "yyyy-MM-dd"
        guard let d = f.date(from: String(iso.prefix(19))) else { return iso }
        return d.formatted(date: .complete, time: iso.count > 10 ? .shortened : .omitted)
    }
}

/// All Metadata: every EXIF, TIFF and GPS tag of the photo's file by group, then its XMP
/// properties (`photo.allMetadata`), searchable.
struct AllMetadataSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var rows: JSONValue = .null
    @State private var search = ""

    var body: some View {
        NavigationStack {
            List {
                if let note = rows["note"].string {
                    Text(note).foregroundStyle(.secondary)
                }
                ForEach(groups, id: \.0) { group, items in
                    Section(group) {
                        ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                            LabeledContent(item.0) {
                                Text(item.1).multilineTextAlignment(.trailing).textSelection(.enabled)
                            }
                        }
                    }
                }
            }
            .searchable(text: $search, prompt: "Filter fields")
            .navigationTitle("All Metadata")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done", systemImage: "checkmark") { dismiss() } } }
            .overlay { if rows.isNull { ProgressView() } }
        }
        .task {
            guard let engine = model.engine else { return }
            rows = (try? await engine.execute("photo.allMetadata")) ?? ["exif": [], "xmp": []]
        }
    }

    /// The fields that match the search, by group (EXIF groups in order, XMP last).
    private var groups: [(String, [(String, String)])] {
        let q = search.trimmingCharacters(in: .whitespaces).lowercased()
        let keep = { (n: String, v: String) in q.isEmpty || n.lowercased().contains(q) || v.lowercased().contains(q) }
        var out: [(String, [(String, String)])] = []
        for row in rows["exif"].array ?? [] {
            let (g, n, v) = (row["group"].string ?? "", row["name"].string ?? "", row["value"].string ?? "")
            guard keep(n, v) else { continue }
            if let i = out.firstIndex(where: { $0.0 == g }) { out[i].1.append((n, v)) } else { out.append((g, [(n, v)])) }
        }
        let xmp = (rows["xmp"].array ?? []).map { ($0["name"].string ?? "", $0["value"].string ?? "") }.filter { keep($0.0, $0.1) }
        if !xmp.isEmpty { out.append(("XMP", xmp)) }
        return out
    }
}
#endif
