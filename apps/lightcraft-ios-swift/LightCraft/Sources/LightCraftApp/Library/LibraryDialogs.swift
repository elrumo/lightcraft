import LightCraftCore
import SwiftUI
import UniformTypeIdentifiers

/// Rename Photos: a file-name template and its tags, the new names as they would be, then Rename
/// (`photo.renameTokens`, `photo.renamePreview`, `photo.rename`).
struct RenamePhotosSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let ids: [UInt64]
    @State private var template = "{date}_{name}"
    @State private var start = 1
    @State private var plans: [JSONValue] = []
    @State private var tokens: [JSONValue] = []

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Template", text: $template)
                        .noAutocap()
                        .autocorrectionDisabled()
                        .font(.body.monospaced())
                    Stepper(value: $start, in: 1...1_000_000) {
                        LabeledContent("Start Number", value: start.formatted())
                    }
                }
                Section("Preview") {
                    ForEach(Array(plans.prefix(20).enumerated()), id: \.offset) { _, p in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(p["to"].string ?? "").font(.body.monospaced())
                            Text(p["from"].string ?? "").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    if plans.count > 20 {
                        Text("and \(plans.count - 20) more").foregroundStyle(.secondary)
                    }
                }
                Section("Tags") {
                    ForEach(Array(tokens.enumerated()), id: \.offset) { _, t in
                        Button {
                            template += t["tag"].string ?? ""
                        } label: {
                            LabeledContent {
                                Text(t["example"].string ?? "").foregroundStyle(.secondary)
                            } label: {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(t["tag"].string ?? "").font(.body.monospaced())
                                    Text(LocalizedStringKey(t["meaning"].string ?? "")).font(.caption).foregroundStyle(.secondary)
                                }
                            }
                        }
                        .foregroundStyle(.primary)
                    }
                }
            }
            .navigationTitle("Rename Photos")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Rename") {
                        Task { if await model.run("photo.rename", params) != nil { dismiss() } }
                    }
                    .disabled(template.trimmingCharacters(in: .whitespaces).isEmpty || plans.isEmpty)
                }
            }
            .task(id: "\(template)|\(start)") {
                guard let engine = model.engine else { return }
                plans = (try? await engine.execute("photo.renamePreview", params))?.array ?? []
            }
            .task {
                guard let engine = model.engine else { return }
                tokens = (try? await engine.execute("photo.renameTokens"))?["tokens"].array ?? []
            }
        }
    }

    private var params: JSONValue {
        ["template": .string(template), "start": .int(Int64(start)), "ids": AppModel.ids(ids)]
    }
}

/// Edit Capture Time: the photo's date and time (the others move by as much), every photo to one
/// time, or a time-zone shift in hours (`photo.setCaptureTime`).
struct CaptureTimeSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let ids: [UInt64]
    @State private var mode = Mode.adjust
    @State private var date = Date()
    @State private var hours = 0

    enum Mode: Hashable { case adjust, each, zone }

    /// Capture times are local and carry no zone: read and written as if in UTC, they don't move.
    private static let utc = TimeZone(secondsFromGMT: 0) ?? .current

    var body: some View {
        NavigationStack {
            Form {
                Picker("Change", selection: $mode) {
                    Text("Adjust to a Time").tag(Mode.adjust)
                    Text("Set Each Photo").tag(Mode.each)
                    Text("Shift by Time Zone").tag(Mode.zone)
                }
                .pickerStyle(.inline)
                .labelsHidden()
                Section {
                    switch mode {
                    case .adjust, .each:
                        DatePicker("Date and Time", selection: $date)
                            .environment(\.timeZone, Self.utc)
                    case .zone:
                        Stepper(value: $hours, in: -26...26) {
                            LabeledContent("Hours", value: hours > 0 ? "+\(hours)" : "\(hours)")
                        }
                    }
                } footer: {
                    switch mode {
                    case .adjust: Text("The photo shown gets this time; the others move by as much, keeping their spacing.")
                    case .each: Text("Every chosen photo gets this time.")
                    case .zone: Text("Every chosen photo moves by this many hours.")
                    }
                }
            }
            .navigationTitle("Edit Capture Time")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Change") {
                        Task { if await model.run("photo.setCaptureTime", params) != nil { dismiss() } }
                    }
                    .disabled(mode == .zone && hours == 0)
                }
            }
            .task {
                guard let engine = model.engine, let id = model.state.active ?? ids.first,
                      let info = try? await engine.execute("photo.inspect", ["id": .int(Int64(clamping: id))]),
                      let captured = info["captured"].string, let d = Self.format.date(from: captured) else { return }
                date = d
            }
        }
    }

    private var params: JSONValue {
        switch mode {
        case .adjust: ["ids": AppModel.ids(ids), "time": .string(Self.format.string(from: date))]
        case .each: ["ids": AppModel.ids(ids), "time": .string(Self.format.string(from: date)), "each": true]
        case .zone: ["ids": AppModel.ids(ids), "hours": .int(Int64(hours))]
        }
    }

    private static let format: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = utc
        f.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
        return f
    }()
}

/// Color Label Names: what each colour means here (Red: "Client picks"…), and the sets of names
/// to switch between (`label.names`, `label.setNames`, `label.sets`, `label.applySet`, …).
struct LabelNamesSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var names: [String: String] = [:]
    @State private var defaults: [String: String] = [:]
    @State private var sets: [JSONValue] = []
    @State private var current: String?
    @State private var savingSet = false
    @State private var setName = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    ForEach(LabelColor.all, id: \.self) { label in
                        HStack {
                            Circle().fill(LabelColor.color(label)).frame(width: 14, height: 14)
                            TextField(defaults[label] ?? label, text: Binding(get: { names[label] ?? "" }, set: { names[label] = $0 }))
                        }
                    }
                } footer: {
                    Text("An empty name gives the colour its own name back.")
                }
                Section("Label Sets") {
                    ForEach(Array(sets.enumerated()), id: \.offset) { _, set in
                        let name = set["name"].string ?? ""
                        Button {
                            Task {
                                await model.run("label.applySet", ["name": .string(name)])
                                await load()
                            }
                        } label: {
                            LabeledContent(LocalizedStringKey(name)) {
                                if name == current { Image(systemName: "checkmark").foregroundStyle(Color.accentColor) }
                            }
                        }
                        .foregroundStyle(.primary)
                        .swipeActions {
                            if set["builtin"].bool != true {
                                Button("Delete", role: .destructive) {
                                    Task {
                                        await model.run("label.deleteSet", ["name": .string(name)])
                                        await load()
                                    }
                                }
                            }
                        }
                    }
                    Button("Save as Set…", systemImage: "plus") {
                        setName = ""
                        savingSet = true
                    }
                }
            }
            .navigationTitle("Color Label Names")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        Task { if await save() { dismiss() } }
                    }
                }
            }
            .alert("Save Label Set", isPresented: $savingSet) {
                TextField("Set Name", text: $setName)
                Button("Save") {
                    Task {
                        guard await save() else { return }
                        await model.run("label.saveSet", ["name": .string(setName.trimmingCharacters(in: .whitespaces))])
                        await load()
                    }
                }
                Button("Cancel", role: .cancel) {}
            }
            .task { await load() }
        }
    }

    private func save() async -> Bool {
        var out: [String: JSONValue] = [:]
        for label in LabelColor.all {
            let n = (names[label] ?? "").trimmingCharacters(in: .whitespaces)
            out[label] = n.isEmpty || n == defaults[label] ? .null : .string(n)
        }
        return await model.run("label.setNames", ["names": .object(out)]) != nil
    }

    private func load() async {
        guard let engine = model.engine else { return }
        for row in (try? await engine.execute("label.names"))?.array ?? [] {
            guard let label = row["label"].string else { continue }
            let custom = row["custom"].bool == true
            names[label] = custom ? row["name"].string ?? "" : ""
            // the colour's own name: the engine's when none is set, else the colour, translated
            defaults[label] = custom ? String(localized: String.LocalizationValue(label.capitalized)) : row["name"].string ?? label.capitalized
        }
        let s = (try? await engine.execute("label.sets")) ?? .null
        sets = s["sets"].array ?? []
        current = s["current"].string
    }
}

/// Auto-Stack by Capture Time: photos taken within the gap of each other become a stack; how many
/// stacks it would make shows as the gap changes (`stack.auto`, `preview`).
struct AutoStackSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var gap: Double = 60
    @State private var preview: JSONValue = .null

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    VStack(alignment: .leading) {
                        LabeledContent("Time Between Stacks", value: Duration.seconds(gap).formatted(.units(allowed: [.hours, .minutes, .seconds], width: .abbreviated)))
                        Slider(value: $gap, in: 1...3600, step: 1)
                    }
                } footer: {
                    if let stacks = preview["stacks"].int {
                        Text("\(stacks) stacks of \(preview["photos"].int ?? 0) photos")
                    }
                }
            }
            .navigationTitle("Auto-Stack by Capture Time")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Stack") {
                        Task { if await model.run("stack.auto", ["gap": .double(gap)]) != nil { dismiss() } }
                    }
                    .disabled((preview["stacks"].int ?? 0) == 0)
                }
            }
            .task(id: gap) {
                guard let engine = model.engine else { return }
                preview = (try? await engine.execute("stack.auto", ["gap": .double(gap), "preview": true])) ?? .null
            }
        }
        .presentationDetents([.medium])
    }
}

/// Keyword Sets: nine keywords a tap puts on the chosen photos (or takes off), the sets to switch
/// between, and editing the current one (`keyword.sets`, `keyword.useSet`, `keyword.saveSet`, …).
struct KeywordSetsSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var sets: [JSONValue] = []
    @State private var current: String?
    @State private var nine: [String] = Array(repeating: "", count: 9)
    @State private var newSet = false
    @State private var newName = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 3)) {
                        ForEach(0..<9, id: \.self) { i in
                            Button {
                                model.perform("keyword.toggleFromSet", ["index": .int(Int64(i + 1))])
                            } label: {
                                Text(nine[safe: i].flatMap { $0.isEmpty ? nil : $0 } ?? "–")
                                    .lineLimit(1)
                                    .frame(maxWidth: .infinity, minHeight: 36)
                            }
                            .buttonStyle(.bordered)
                            .disabled(nine[safe: i]?.isEmpty != false)
                        }
                    }
                } header: {
                    Text(current.map { LocalizedStringKey($0) } ?? "Keyword Set")
                } footer: {
                    Text("A tap puts the keyword on the chosen photos, or takes it off when they all have it.")
                }
                Section("Keywords in This Set") {
                    ForEach(0..<9, id: \.self) { i in
                        TextField("Keyword \(i + 1)", text: Binding(get: { nine[safe: i] ?? "" }, set: { if nine.indices.contains(i) { nine[i] = $0 } }))
                            .noAutocap()
                    }
                    Button("Save Set") {
                        Task {
                            await model.run("keyword.saveSet", ["name": .string(current ?? "Keyword Set"), "keywords": .array(nine.map { .string($0) })])
                            await load()
                        }
                    }
                }
                Section("Sets") {
                    ForEach(Array(sets.enumerated()), id: \.offset) { _, set in
                        let name = set["name"].string ?? ""
                        Button {
                            Task {
                                await model.run("keyword.useSet", ["name": .string(name)])
                                await load()
                            }
                        } label: {
                            LabeledContent(LocalizedStringKey(name)) {
                                if name == current { Image(systemName: "checkmark").foregroundStyle(Color.accentColor) }
                            }
                        }
                        .foregroundStyle(.primary)
                        .swipeActions {
                            Button("Delete", role: .destructive) {
                                Task {
                                    await model.run("keyword.deleteSet", ["name": .string(name)])
                                    await load()
                                }
                            }
                        }
                    }
                    Button("New Set…", systemImage: "plus") {
                        newName = ""
                        newSet = true
                    }
                }
            }
            .navigationTitle("Keyword Sets")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button(role: .confirm) { dismiss() } }
            }
            .alert("New Keyword Set", isPresented: $newSet) {
                TextField("Set Name", text: $newName)
                Button("Create") {
                    Task {
                        await model.run("keyword.saveSet", ["name": .string(newName.trimmingCharacters(in: .whitespaces)), "keywords": .array(Array(repeating: .string(""), count: 9)), "new": true])
                        await load()
                    }
                }
                Button("Cancel", role: .cancel) {}
            }
            .task { await load() }
        }
    }

    private func load() async {
        guard let engine = model.engine else { return }
        let s = (try? await engine.execute("keyword.sets")) ?? .null
        sets = s["sets"].array ?? []
        current = s["current"].string
        let kw = (s["keywords"].array ?? []).map { $0.string ?? "" }
        nine = (0..<9).map { kw[safe: $0] ?? "" }
    }
}

/// Metadata Presets: copyright, creator, place and keywords saved by name, put on the chosen
/// photos with a tap (`metadata.presets`, `metadata.applyPreset`, `metadata.savePreset`, …).
struct MetadataPresetsSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let ids: [UInt64]
    @State private var presets: [JSONValue] = []
    @State private var saving = false
    @State private var name = ""

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(Array(presets.enumerated()), id: \.offset) { _, p in
                        let n = p["name"].string ?? ""
                        Button {
                            Task {
                                if await model.run("metadata.applyPreset", ["name": .string(n), "ids": AppModel.ids(ids)]) != nil { dismiss() }
                            }
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(n)
                                Text(Self.summary(p["fields"])).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                            }
                        }
                        .foregroundStyle(.primary)
                        .swipeActions {
                            Button("Delete", role: .destructive) {
                                Task {
                                    await model.run("metadata.deletePreset", ["name": .string(n)])
                                    await load()
                                }
                            }
                        }
                    }
                    if presets.isEmpty { Text("No metadata presets yet").foregroundStyle(.secondary) }
                } footer: {
                    Text("A tap puts the preset on the chosen photos: its text replaces theirs, its keywords are added.")
                }
                Section {
                    Button("Save This Photo's as a Preset…", systemImage: "plus") {
                        name = ""
                        saving = true
                    }
                    .disabled(model.state.active == nil)
                } footer: {
                    Text("Its copyright, creator and place.")
                }
            }
            .navigationTitle("Metadata Presets")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
            }
            .alert("Save Metadata Preset", isPresented: $saving) {
                TextField("Preset Name", text: $name)
                Button("Save") {
                    Task {
                        await model.run("metadata.savePreset", ["name": .string(name.trimmingCharacters(in: .whitespaces))])
                        await load()
                    }
                }
                Button("Cancel", role: .cancel) {}
            }
            .task { await load() }
        }
    }

    private func load() async {
        guard let engine = model.engine else { return }
        presets = (try? await engine.execute("metadata.presets"))?.array ?? []
    }

    /// The preset's fields, as a line: "© Ana · Ana Ruiz · Lisbon".
    static func summary(_ fields: JSONValue) -> String {
        guard case .object(let o) = fields else { return "" }
        return o.keys.sorted().compactMap { k in
            if let s = o[k]?.string, !s.isEmpty { return s }
            if let a = o[k]?.array, !a.isEmpty { return a.compactMap(\.string).joined(separator: ", ") }
            return nil
        }
        .joined(separator: " · ")
    }
}

/// Contact Sheet: the chosen photos laid out on PDF pages, with or without their names, to share
/// or print (`export.contactSheet`).
struct ContactSheetSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let ids: [UInt64]
    @State private var paper = "a4"
    @State private var landscape = false
    @State private var columns = 4
    @State private var rows = 5
    @State private var captions = true
    @State private var working = false
    @State private var made: SharedFile?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Picker("Paper", selection: $paper) {
                        Text("A4").tag("a4")
                        Text("Letter").tag("letter")
                    }
                    Toggle("Landscape", isOn: $landscape)
                }
                Section {
                    Stepper(value: $columns, in: 1...8) { LabeledContent("Columns", value: columns.formatted()) }
                    Stepper(value: $rows, in: 1...10) { LabeledContent("Rows", value: rows.formatted()) }
                    Toggle("File Names", isOn: $captions)
                } footer: {
                    let per = columns * rows
                    Text("\(ids.count) photos on \(max(1, (ids.count + per - 1) / per)) pages")
                }
            }
            .navigationTitle("Contact Sheet")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Make PDF") { Task { await make() } }
                        .disabled(ids.isEmpty || working)
                }
            }
            .sheet(item: $made) { file in
                ActivityView(urls: [file.url]) { dismiss() }
            }
        }
    }

    private func make() async {
        working = true
        defer { working = false }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(String(localized: "Contact Sheet") + ".pdf")
        let p: JSONValue = ["path": .string(url.path), "ids": AppModel.ids(ids), "paper": .string(paper), "landscape": .bool(landscape),
                            "columns": .int(Int64(columns)), "rows": .int(Int64(rows)), "captions": .bool(captions)]
        if await model.run("export.contactSheet", p) != nil { made = SharedFile(url: url) }
    }
}

/// Assisted Culling: the chosen photos scored for focus, look-alike bursts grouped; optionally the
/// blurry ones rejected and the sharpest of each burst picked (`photo.analyze`).
struct CullSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let ids: [UInt64]
    @State private var rejectBelow: Double = 0
    @State private var pickBest = true
    @State private var working = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    VStack(alignment: .leading) {
                        LabeledContent("Reject below focus", value: Int(rejectBelow).formatted())
                        Slider(value: $rejectBelow, in: 0...80, step: 1)
                    }
                } footer: {
                    Text(rejectBelow > 0 ? "Blurry photos below the score are flagged as rejects." : "0: nothing is rejected.")
                }
                Section {
                    Toggle("Pick the sharpest photo of each burst", isOn: $pickBest)
                } footer: {
                    Text(ids.isEmpty ? "The photos in view are scored." : "\(ids.count) photos are scored.")
                }
            }
            .navigationTitle("Assisted Culling")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Cull") {
                        working = true
                        var p: [String: JSONValue] = ["pickBest": .bool(pickBest)]
                        if !ids.isEmpty { p["ids"] = AppModel.ids(ids) }
                        if rejectBelow > 0 { p["rejectBelow"] = .double(rejectBelow) }
                        Task {
                            if await model.run("photo.analyze", .object(p)) != nil { dismiss() }
                            working = false
                        }
                    }
                    .disabled(working)
                }
            }
            .overlay { if working { ProgressView() } }
        }
        .presentationDetents([.medium])
    }
}

extension AppModel {
    /// A file or folder picked for `pick`: read while the system lets the app (it's another app's).
    func picked(_ url: URL, for pick: FilePick) async {
        // (the catalog's sheet keeps the folder open for as long as its import runs)
        if case .lightroom = pick {
            sheet = .lightroom(url)
            return
        }
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        switch pick {
        case .tracklog(let ids):
            var p: [String: JSONValue] = ["path": .string(url.path)]
            if !ids.isEmpty { p["ids"] = AppModel.ids(ids) }
            if let r = await run("photo.autoTagTracklog", .object(p)) {
                show(String(localized: "\(r["tagged"].int ?? 0) photos tagged with a location"))
            }
        case .findMissing:
            if let r = await run("library.findMissing", ["folder": .string(url.path)]) {
                show(String(localized: "\(r["relinked"].int ?? r["found"].array.map { Int64($0.count) } ?? 0) photos found"))
            }
        case .locate(let id):
            if await run("photo.relink", ["id": .int(Int64(clamping: id)), "path": .string(url.path)]) != nil {
                show(String(localized: "Photo found"))
            }
        case .lightroom: break
        }
    }

    /// Export with Previous: the chosen photos with the last export's options, then the share sheet.
    func exportPrevious() async {
        guard let r = await request("host.exportPrevious") else { return }
        let paths = (r["files"].array ?? []).compactMap { $0["path"].string }
        if !paths.isEmpty { sheet = .shareFiles(paths) }
    }
}

/// A file to hand to the share sheet.
struct SharedFile: Identifiable {
    let id = UUID()
    let url: URL
}

/// Pick a keyword (or the top level) to move or merge a keyword into.
struct KeywordPickerSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let title: LocalizedStringKey
    /// The keyword being moved or merged: it and what is inside it can't be the target.
    let keyword: String
    let allowTopLevel: Bool
    let pick: (String?) -> Void
    @State private var paths: [String] = []
    @State private var search = ""

    var body: some View {
        NavigationStack {
            List {
                if allowTopLevel && search.isEmpty {
                    Button("Top Level") {
                        pick(nil)
                        dismiss()
                    }
                }
                ForEach(paths.filter { search.isEmpty || $0.localizedCaseInsensitiveContains(search) }, id: \.self) { p in
                    Button(p.replacingOccurrences(of: "|", with: " ▸ ")) {
                        pick(p)
                        dismiss()
                    }
                    .foregroundStyle(.primary)
                }
            }
            .searchable(text: $search)
            .navigationTitle(title)
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
            }
            .task {
                guard let engine = model.engine else { return }
                let tree = (try? await engine.execute("keyword.list", [:], as: [KeywordNode].self)) ?? []
                var out: [String] = []
                func walk(_ nodes: [KeywordNode], _ depth: Int) {
                    guard depth < 32 else { return }
                    for n in nodes where n.path != keyword && !n.path.hasPrefix(keyword + "|") {
                        out.append(n.path)
                        walk(n.children, depth + 1)
                    }
                }
                walk(tree, 0)
                paths = out
            }
        }
    }
}
