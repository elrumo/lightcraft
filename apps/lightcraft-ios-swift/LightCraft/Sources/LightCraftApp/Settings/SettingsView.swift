import LightCraftCore
import SwiftUI

/// Settings, as an iOS settings list: the library (and where it syncs), then General, Import,
/// Performance, Interface, Sync, Faces and AI Models.
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let section: String?
    @State private var path: [String] = []

    var body: some View {
        NavigationStack(path: $path) {
            List {
                Section {
                    NavigationLink(value: "sync") {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(libraryName).font(.headline)
                            Text(librarySubtitle).font(.subheadline).foregroundStyle(.secondary)
                        }
                        .padding(.vertical, 4)
                    }
                }
                Section {
                    row("general", "General", "gear", .gray)
                    row("import", "Import", "square.and.arrow.down", .green)
                    row("performance", "Performance", "gauge.with.dots.needle.67percent", .orange)
                    row("interface", "Interface", "paintbrush", .indigo)
                }
                Section {
                    row("sync", "Sync", "arrow.triangle.2.circlepath.icloud", .blue)
                    row("faces", "Faces", "person.crop.square", .teal)
                    row("models", "AI Models", "cpu", .purple)
                    row("admin", "Server Administration", "server.rack", .red)
                }
                #if os(iOS)
                Section {
                    // the first launch's welcome again (the sheet closes first: see `AppModel.welcomeAsked`)
                    Button {
                        model.welcomeAsked = true
                        dismiss()
                    } label: {
                        rowLabel("Show Welcome", "sparkles", Brand.amber)
                    }
                    .foregroundStyle(.primary)
                }
                #endif
            }
            .navigationTitle("Settings")
            .inlineTitle()
            .navigationDestination(for: String.self) { Self.page($0) }
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { dismiss() }
                }
            }
            .onAppear { if let section, path.isEmpty { path = [section] } }
            .onChange(of: section) { _, s in path = s.map { [$0] } ?? [] }
        }
    }

    /// A page by its name (the list's rows, `model.sheet = .settings(name)`, a Mac's tabs).
    @ViewBuilder static func page(_ name: String) -> some View {
        switch name {
        case "general": GeneralSettings()
        case "import": ImportSettings()
        case "performance": PerformanceSettings()
        case "interface": InterfaceSettings()
        case "sync": SyncSettings()
        case "faces": FacesSettings()
        case "storage": SyncStorageView()
        case "systemInfo": SystemInfoView()
        case "models": ModelsSettings()
        case "admin": AdminView()
        default: EmptyView()
        }
    }

    private var libraryName: String {
        if model.opened?.demo == true { return String(localized: "Demo Library") }
        return model.opened?.library.map { ($0 as NSString).lastPathComponent } ?? String(localized: "Library")
    }

    private var librarySubtitle: String {
        let n = model.library.stats?.photos ?? model.library.count
        if model.opened?.demo == true { return String(localized: "\(n) photos, not saved") }
        if let s = model.albums.sync, s.signedIn, let server = s.server {
            return String(localized: "\(n) photos · synced with \(server.replacingOccurrences(of: "https://", with: ""))")
        }
        return String(localized: "\(n) photos on this device")
    }

    private func row(_ id: String, _ title: LocalizedStringKey, _ symbol: String, _ color: Color) -> some View {
        NavigationLink(value: id) { rowLabel(title, symbol, color) }
    }

    private func rowLabel(_ title: LocalizedStringKey, _ symbol: String, _ color: Color) -> some View {
        Label {
            Text(title)
        } icon: {
            Image(systemName: symbol)
                .font(.footnote.weight(.semibold))
                .foregroundStyle(.white)
                .frame(width: 28, height: 28)
                .background(color, in: .rect(cornerRadius: 7))
        }
    }
}

struct GeneralSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var prefs = model.prefs
        Form {
            Section {
                Toggle("Auto Advance", isOn: $prefs.autoAdvance)
            } header: {
                Text("Culling")
            } footer: {
                Text("After rating or flagging a photo, go on to the next.")
            }
            #if os(iOS)
            Section {
                Toggle("Haptics", isOn: $prefs.haptics)
            }
            #endif
        }
        .navigationTitle("General")
        .inlineTitle()
    }
}

/// Import defaults: presets for raws and other files, copyright and creator (`library.preferences`).
struct ImportSettings: View {
    @Environment(AppModel.self) private var model
    @State private var prefs: JSONValue = .null
    @State private var presets: [PresetInfo] = []
    @State private var copyright = ""
    @State private var creator = ""

    var body: some View {
        Form {
            Section("Develop Defaults") {
                presetPicker("Raw Photos", key: "rawPreset")
                presetPicker("Other Photos", key: "otherPreset")
                Toggle("Camera-Specific Defaults", isOn: Binding(get: { prefs["import"]["perCamera"].bool == true }, set: { set(["perCamera": .bool($0)]) }))
            }
            Section("Cameras and cards") {
                Toggle("Show the import when a camera or memory card is connected", isOn: Bindable(model.prefs).importOnConnect)
            }
            Section("Metadata") {
                TextField("Copyright", text: $copyright).onSubmit { set(["copyright": .string(copyright)]) }
                TextField("Creator", text: $creator).onSubmit { set(["creator": .string(creator)]) }
            }
        }
        .navigationTitle("Import")
        .inlineTitle()
        .task { await load() }
    }

    private func presetPicker(_ title: LocalizedStringKey, key: String) -> some View {
        Picker(title, selection: Binding(get: { prefs["import"][key].string ?? "" }, set: { set([key: $0.isEmpty ? .null : .string($0)]) })) {
            Text("None").tag("")
            ForEach(presets) { Text($0.name).tag($0.id) }
        }
    }

    private func set(_ values: [String: JSONValue]) {
        Task {
            if let r = await model.run("library.preferences", ["import": .object(values)]) { prefs = r }
        }
    }

    private func load() async {
        guard let engine = model.engine else { return }
        prefs = (try? await engine.execute("library.preferences", [:])) ?? .null
        presets = (try? await engine.execute("presets.list", [:], as: [PresetInfo].self)) ?? []
        copyright = prefs["import"]["copyright"].string ?? ""
        creator = prefs["import"]["creator"].string ?? ""
    }
}

/// The GPU, memory for caches, the thumbnail cache.
struct PerformanceSettings: View {
    @Environment(AppModel.self) private var model
    @State private var gpu: JSONValue = .null
    @State private var prefs: JSONValue = .null

    var body: some View {
        Form {
            Section {
                Toggle("Use the GPU", isOn: Binding(get: { gpu["enabled"].bool == true }, set: { on in
                    Task { gpu = await model.run("app.gpu", ["enabled": .bool(on)]) ?? gpu }
                }))
            } footer: {
                if let adapter = gpu["adapter"].string { Text(adapter) } else if let why = gpu["reason"].string { Text(why) }
            }
            Section {
                Picker("Thumbnail Cache", selection: Binding(get: { Int(prefs["cacheMb"].int ?? 1024) }, set: { mb in
                    Task { prefs = await model.run("library.preferences", ["cacheMb": .int(Int64(mb))]) ?? prefs }
                })) {
                    ForEach([256, 512, 1024, 2048, 4096], id: \.self) { Text(ByteCountFormatter.string(fromByteCount: Int64($0) << 20, countStyle: .file)).tag($0) }
                }
                Button("Clear Cache", role: .destructive) {
                    Task { if await model.run("library.clearPreviews") != nil { model.show(String(localized: "✓ Cache cleared")) } }
                }
            }
        }
        .navigationTitle("Performance")
        .inlineTitle()
        .task {
            guard let engine = model.engine else { return }
            gpu = (try? await engine.execute("app.gpu", [:])) ?? .null
            prefs = (try? await engine.execute("library.preferences", [:])) ?? .null
        }
    }
}

struct InterfaceSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var prefs = model.prefs
        Form {
            Section {
                Picker("Appearance", selection: $prefs.appearance) {
                    ForEach(Prefs.Appearance.allCases) { Text($0.label).tag($0) }
                }
                // iOS keeps each app's language: Settings ▸ LightCraft ▸ Language
                Button {
                    #if os(iOS)
                    if let url = URL(string: UIApplication.openSettingsURLString) { Platform.open(url) }
                    #else
                    // (System Settings ▸ General ▸ Language & Region ▸ Applications)
                    if let url = URL(string: "x-apple.systempreferences:com.apple.Localization-Settings.extension") { Platform.open(url) }
                    #endif
                } label: {
                    LabeledContent("Language", value: Locale.current.localizedString(forIdentifier: Bundle.main.preferredLocalizations.first ?? "en") ?? "")
                }
                .foregroundStyle(.primary)
            } footer: {
                #if os(iOS)
                Text("Opens LightCraft in the Settings app, where its language is chosen.")
                #else
                Text("Opens System Settings, where each app's language is chosen.")
                #endif
            }
            Section("Grid") {
                Picker("Ratings & Flags", selection: $prefs.badges) {
                    ForEach(Prefs.Badges.allCases) { Text($0.label).tag($0) }
                }
                Toggle("Show Counts in Albums", isOn: $prefs.showCounts)
            }
            // (a Mac always has sliders: dials are for a finger)
            #if os(iOS)
            Section {
                Picker("Adjustments", selection: $prefs.editStyle) {
                    ForEach(Prefs.EditStyle.allCases) { Text($0.label).tag($0) }
                }
            } header: {
                Text("Editing")
            } footer: {
                Text("Dials set one control at a time with a ruler, as in Photos; sliders show a group's controls together. Automatic uses dials under the photo and sliders beside it.")
            }
            #endif
            Section("Photo") {
                Picker("Info Overlay", selection: $prefs.infoOverlay) {
                    ForEach(Prefs.InfoOverlay.allCases) { Text($0.label).tag($0) }
                }
                Picker("Double Tap Zooms To", selection: $prefs.clickZoom) {
                    Text("200 %").tag(2.0)
                    Text("300 %").tag(3.0)
                    Text("400 %").tag(4.0)
                }
            }
            Section("Map") {
                Toggle("Load Map Tiles from the Internet", isOn: $prefs.mapTiles)
            }
        }
        .navigationTitle("Interface")
        .inlineTitle()
    }
}

/// Sync with your own LightCraft server: sign in, the account, what stays on this device, shared
/// links, sign out. Without a server the library stays on this device and everything works.
struct SyncSettings: View {
    @Environment(AppModel.self) private var model
    @State private var status: SyncStatus?
    @State private var server = ""
    @State private var user = ""
    @State private var password = ""
    @State private var merge = false
    @State private var signingIn = false
    @State private var shares: [JSONValue] = []

    var body: some View {
        Form {
            if model.opened?.demo == true {
                Section { Text("Sync needs a saved library: this is the demo library.").foregroundStyle(.secondary) }
            } else if let s = status, s.signedIn {
                Section("Account") {
                    LabeledContent("Server", value: s.server ?? "")
                    LabeledContent("User", value: s.user ?? "")
                    LabeledContent("Status", value: SyncStatusSheet.describe(s))
                }
                Section {
                    if s.state == "conflict" {
                        Button("Choose What to Do…") { model.sheet = .syncChoice }
                    } else {
                        Button("Sync Now", systemImage: "arrow.triangle.2.circlepath") { run("sync.now") }
                        Button(s.paused == true ? "Resume Syncing" : "Pause Syncing", systemImage: s.paused == true ? "play" : "pause") {
                            run("sync.pause", ["on": .bool(s.paused != true)])
                        }
                    }
                }
                Section {
                    Toggle("Store All Originals on This Device", isOn: Binding(get: { s.storeOriginals == true }, set: { run("sync.storeOriginalsLocally", ["on": .bool($0)]) }))
                    Toggle("Download All Small Previews", isOn: Binding(get: { s.allPreviews == true }, set: { run("sync.allPreviews", ["on": .bool($0)]) }))
                    Picker("Originals Limit", selection: Binding(get: { Int(s.originalsBudgetMb ?? 0) }, set: { mb in run("sync.originalsBudget", ["mb": mb == 0 ? .null : .int(Int64(mb))]) })) {
                        Text("No Limit").tag(0)
                        ForEach([5, 10, 25, 50, 100, 250], id: \.self) { Text("\($0) GB").tag($0 * 1000) }
                    }
                    Toggle("Let the Server Build Previews", isOn: Binding(get: { s.serverPreviews == true }, set: { run("sync.serverPreviews", ["on": .bool($0)]) }))
                } header: {
                    Text("On This Device")
                } footer: {
                    Text("\(s.offlineAlbums?.count ?? 0) albums available offline.")
                }
                Section {
                    NavigationLink("Storage") { SyncStorageView() }
                }
                if !shares.isEmpty {
                    Section("Shared Links") {
                        ForEach(Array(shares.enumerated()), id: \.offset) { _, share in
                            VStack(alignment: .leading) {
                                Text(share["name"].string ?? "").font(.headline)
                                Text(share["url"].string ?? "").font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                            }
                            .swipeActions {
                                Button("Revoke", role: .destructive) { run("album.unshare", ["id": share["album"]]) }
                                Button("Copy") { if let url = share["url"].string { Platform.copy(url) } }.tint(.blue)
                            }
                        }
                    }
                }
                Section {
                    Button("Sign Out", role: .destructive) { run("sync.signOut") }
                }
            } else if status == nil {
                Section { ProgressView().frame(maxWidth: .infinity) }
            } else {
                Section {
                    if let e = status?.error { Text(e).foregroundStyle(.red) }
                    TextField("Server", text: $server, prompt: Text(verbatim: "https://photos.example.com"))
                        .lcKeyboard(.url)
                        .noAutocap()
                        .autocorrectionDisabled()
                    TextField("User", text: $user)
                        .textContentType(.username)
                        .noAutocap()
                        .autocorrectionDisabled()
                    SecureField("Password", text: $password)
                        .textContentType(.password)
                    Toggle("Combine with the Server's Library", isOn: $merge)
                } header: {
                    Text("Your LightCraft Server")
                } footer: {
                    Text("Sync this library with a LightCraft server you run (docs/self-hosting.md). Without one, the library stays on this device and everything works offline.")
                }
                Section {
                    Button {
                        Task { await signIn() }
                    } label: {
                        HStack {
                            Text("Sign In")
                            if signingIn { Spacer(); ProgressView() }
                        }
                    }
                    .disabled(server.isEmpty || user.isEmpty || password.isEmpty || signingIn)
                }
            }
        }
        .navigationTitle("Sync")
        .inlineTitle()
        .task(id: model.state.sync) { await load() }
    }

    private func run(_ command: String, _ params: JSONValue = [:]) {
        Task {
            await model.run(command, params)
            await load()
        }
    }

    private func signIn() async {
        signingIn = true
        defer { signingIn = false }
        let p: JSONValue = ["server": .string(server), "user": .string(user), "password": .string(password), "merge": .bool(merge)]
        password = ""
        _ = await model.run("sync.signIn", p)
        await load()
    }

    private func load() async {
        guard let engine = model.engine else { return }
        do {
            status = try await engine.execute("sync.status", [:], as: SyncStatus.self)
        } catch {
            // (a reply this app can't read: say so rather than offer to sign in again)
            model.show(error.localizedDescription, error: true)
        }
        if status?.signedIn == true, let r = try? await engine.execute("shares.list") {
            shares = r["shares"].array ?? []
        }
        if let engineModel = model.engine { await model.albums.reload(engineModel) }
    }
}

/// The AI models: what each is for, its size and licence; download, turn on or off, delete.
struct ModelsSettings: View {
    @Environment(AppModel.self) private var model
    @State private var models: [JSONValue] = []
    @State private var confirming: JSONValue?

    var body: some View {
        Form {
            AppleModelsSection()
            ForEach(Array(models.enumerated()), id: \.offset) { _, m in
                Section {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(m["label"].string ?? "").font(.headline)
                        Text(m["purpose"].string ?? "").font(.subheadline).foregroundStyle(.secondary)
                        if let bytes = m["downloadBytes"].int {
                            Text(ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)).font(.caption).foregroundStyle(.secondary)
                        }
                        if let licence = m["licence"].string { Text(licence).font(.caption).foregroundStyle(.secondary) }
                    }
                    if m["installed"].bool == true {
                        Toggle("Use", isOn: Binding(get: { m["enabled"].bool == true }, set: { on in switchModel(m["id"], on) }))
                        Button("Delete", role: .destructive) { run("models.delete", ["id": m["id"], "confirm": true]) }
                    } else if m["download"]["running"].bool == true {
                        HStack {
                            ProgressView(value: Double(m["download"]["done"].int ?? 0), total: Double(max(m["download"]["total"].int ?? 1, 1)))
                            Button("Cancel") { run("models.cancel", ["id": m["id"]]) }
                        }
                    } else if m["available"].bool != false {
                        Button("Download…") { confirming = m }
                    }
                }
            }
        }
        .navigationTitle("AI Models")
        .inlineTitle()
        .task { await load() }
        .task(id: model.activity.count) { await load() }
        .alert("Download \(confirming?["label"].string ?? "")?", isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } })) {
            Button("Download") {
                if let m = confirming { run("models.download", ["id": m["id"], "acknowledged": true]) }
                confirming = nil
            }
            Button("Cancel", role: .cancel) { confirming = nil }
        } message: {
            Text("By downloading it you accept its licence: \(confirming?["licence"].string ?? "").")
        }
    }

    private func run(_ command: String, _ params: JSONValue) {
        Task {
            await model.run(command, params)
            await load()
        }
    }

    /// "Use" on a model: the engine answers with every model that is off now, which the app keeps for the next launch.
    private func switchModel(_ id: JSONValue, _ on: Bool) {
        Task {
            if let r = await model.run("models.setEnabled", ["id": id, "enabled": .bool(on)]) {
                model.prefs.modelsDisabled = (r["disabled"].array ?? []).compactMap(\.string)
            }
            await load()
        }
    }

    private func load() async {
        guard let engine = model.engine, let r = try? await engine.execute("models.list") else { return }
        models = r["models"].array ?? []
    }
}

/// Where syncing stands (the grid's cloud): state, what's left to send and receive, Pause, Sync Now.
struct SyncStatusSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var status: SyncStatus?

    var body: some View {
        NavigationStack {
            List {
                if let s = status {
                    Section {
                        VStack(alignment: .leading, spacing: 6) {
                            Text(Self.title(s)).font(.headline)
                            Text(Self.describe(s)).font(.subheadline).foregroundStyle(.secondary)
                        }
                        .padding(.vertical, 4)
                    }
                    if s.signedIn {
                        Section("This Device") {
                            if let r = s.reading, r.count == 2 {
                                LabeledContent("Loading the library", value: "\(r[0]) of \(r[1])")
                            }
                            LabeledContent("Changes to send", value: "\(s.pending ?? 0)")
                            LabeledContent("Uploads", value: "\(s.uploads ?? 0)")
                            LabeledContent("Downloads", value: "\(s.downloads ?? 0)")
                        }
                        Section {
                            if s.state == "conflict" {
                                Button("Choose What to Do…") { model.sheet = .syncChoice }
                            } else {
                                Button("Sync Now", systemImage: "arrow.triangle.2.circlepath") { model.perform("sync.now") }
                                Button(s.paused == true ? "Resume" : "Pause", systemImage: s.paused == true ? "play" : "pause") {
                                    model.perform("sync.pause", ["on": .bool(s.paused != true)])
                                }
                            }
                        }
                    } else {
                        Section {
                            Button("Set Up Sync…") { model.sheet = .settings("sync") }
                        }
                    }
                }
                if model.state.unsaved > 0 {
                    Section {
                        Label("\(model.state.unsaved) changes aren't saved on this device yet.", systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                    }
                }
            }
            .navigationTitle("Sync")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Close", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .primaryAction) {
                    Button("Settings", systemImage: "gear") { model.sheet = .settings("sync") }
                }
            }
            .task(id: model.state.sync) {
                guard let engine = model.engine else { return }
                status = try? await engine.execute("sync.status", [:], as: SyncStatus.self)
            }
        }
        .presentationDetents([.medium, .large])
    }

    static func title(_ s: SyncStatus) -> String {
        switch s.state {
        case "off": String(localized: "This library is only on this device")
        case "signedOut": String(localized: "Signed out")
        case "conflict": String(localized: "The server has a library already")
        case "paused": String(localized: "Syncing is paused")
        case "error": String(localized: "Can't sync")
        case "syncing": String(localized: "Syncing…")
        default: String(localized: "Up to date")
        }
    }

    static func describe(_ s: SyncStatus) -> String {
        let host = (s.server ?? "").replacingOccurrences(of: "https://", with: "").replacingOccurrences(of: "http://", with: "")
        switch s.state {
        case "off": return String(localized: "Set up sync to see it on your other devices.")
        case "signedOut": return String(localized: "Sign in again in Settings ▸ Sync.")
        case "conflict": return String(localized: "Choose what to do with this library's photos.")
        case "paused": return String(localized: "Changes wait on this device until you resume.")
        case "error": return (s.error ?? "") + "\n" + String(localized: "LightCraft keeps trying.")
        case "syncing": return String(localized: "\(s.pending ?? 0) changes to send, \(s.uploads ?? 0) uploads, \(s.downloads ?? 0) downloads")
        default: return String(localized: "Synced with \(host)")
        }
    }
}

/// Signing in met a server that has a library already: add this library's photos to it, use the
/// server's instead, or don't sync this library.
struct SyncChoiceSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var status: SyncStatus?

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Text("This library and the server's both have photos. Choose what to do with this library's photos; nothing syncs until you do.")
                        .font(.subheadline)
                }
                if let c = status?.conflict, c != .null {
                    Section {
                        LabeledContent("This library", value: "\(c["local"]["photos"].int ?? 0) photos")
                        LabeledContent("The server's", value: "\(c["server"]["photos"].int ?? 0) photos")
                    }
                }
                Section {
                    choice("Add to Server's Library", "Upload this library's photos and edits to the server, beside its own.", "upload", prominent: true)
                    choice("Use Server's Library", "Replace this library with the server's. Photos only here are kept in Recently Deleted.", "useServer")
                    choice("Don't Sync This Library", "Sign out and keep this library on this device only.", "cancel")
                }
            }
            .navigationTitle("Sync")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Decide Later") { dismiss() } }
            }
            .task {
                guard let engine = model.engine else { return }
                status = try? await engine.execute("sync.status", [:], as: SyncStatus.self)
            }
        }
    }

    private func choice(_ title: LocalizedStringKey, _ note: LocalizedStringKey, _ id: String, prominent: Bool = false) -> some View {
        Button {
            Task {
                if await model.run("sync.resolveConflict", ["choice": .string(id)]) != nil { dismiss() }
            }
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.headline).foregroundStyle(prominent ? Color.accentColor : .primary)
                Text(note).font(.footnote).foregroundStyle(.secondary)
            }
            .padding(.vertical, 4)
        }
    }
}

/// A link to an album on your server: how long it works, originals or pictures, whether people may
/// add photos, a password. The link is copied when the server has made it.
struct ShareLinkSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let album: UInt64
    let name: String
    @State private var days = 30
    @State private var originals = false
    @State private var upload = false
    @State private var password = ""

    var body: some View {
        NavigationStack {
            Form {
                Picker("Link Works For", selection: $days) {
                    Text("1 Day").tag(1)
                    Text("1 Week").tag(7)
                    Text("30 Days").tag(30)
                    Text("Until Revoked").tag(0)
                }
                Toggle("Originals Too", isOn: $originals)
                Toggle("People Can Add Photos", isOn: $upload)
                SecureField("Password (optional)", text: $password)
            }
            .navigationTitle(name)
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Create Link") {
                        Task {
                            var p: [String: JSONValue] = ["id": .int(Int64(clamping: album)), "expiresDays": .int(Int64(days)), "originals": .bool(originals), "upload": .bool(upload)]
                            if !password.isEmpty { p["password"] = .string(password) }
                            if await model.run("album.share", .object(p)) != nil {
                                model.show(String(localized: "Making the link: it's copied when it's ready"))
                                dismiss()
                            }
                        }
                    }
                    .disabled(!password.isEmpty && password.count < 6)
                }
            }
        }
        .presentationDetents([.medium])
    }
}

/// About LightCraft.
struct AboutView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var links: [JSONValue] = []

    private static func symbol(_ id: String?) -> String {
        switch id {
        case "app.help": "questionmark.circle"
        case "app.discord": "bubble.left.and.bubble.right"
        case "app.github": "chevron.left.forwardslash.chevron.right"
        case "app.feedback": "envelope"
        default: "safari"
        }
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    VStack(spacing: 8) {
                        Image(systemName: "camera.aperture").font(.system(size: 56)).foregroundStyle(Color.accentColor)
                        Text("LightCraft").font(.title.bold())
                        Text("Version \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "")")
                            .foregroundStyle(.secondary)
                        Text("A photo library and raw developer in pure Rust, open source (MIT OR Apache-2.0).")
                            .font(.footnote).multilineTextAlignment(.center).foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 12)
                }
                Section {
                    NavigationLink("What's New") { WhatsNewView() }
                    NavigationLink("System Info") { SystemInfoView() }
                }
                Section {
                    // the project's links, as the Help menu has them (`about.links`)
                    ForEach(links, id: \.self) { link in
                        if let url = link["url"].string.flatMap(URL.init(string:)) {
                            Link(destination: url) {
                                Label(LocalizedStringKey(link["label"].string ?? ""), systemImage: Self.symbol(link["id"].string))
                            }
                        }
                    }
                }
            }
            .task {
                guard let engine = model.engine else { return }
                links = (try? await engine.execute("about.links"))?.array ?? []
            }
            .navigationTitle("About")
            .inlineTitle()
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done", systemImage: "checkmark") { dismiss() } } }
        }
    }
}

/// What's New in this version (`about.whatsNew`): its headings and items.
struct WhatsNewView: View {
    @Environment(AppModel.self) private var model
    @State private var text = ""

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(Array(text.split(separator: "\n", omittingEmptySubsequences: false).enumerated()), id: \.offset) { _, line in
                    row(String(line))
                }
            }
            .padding()
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .navigationTitle("What's New")
        .inlineTitle()
        .task {
            guard let engine = model.engine else { return }
            text = (try? await engine.execute("about.whatsNew"))?["text"].string ?? ""
        }
    }

    @ViewBuilder private func row(_ line: String) -> some View {
        let t = line.trimmingCharacters(in: .whitespaces)
        if t.hasPrefix("#") {
            Text(markdown(t.drop { $0 == "#" }.trimmingCharacters(in: .whitespaces)))
                .font(t.hasPrefix("##") ? .headline : .title2.bold())
                .padding(.top, 8)
        } else if t.hasPrefix("- ") || t.hasPrefix("* ") {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(verbatim: "•")
                Text(markdown(String(t.dropFirst(2))))
            }
            .font(.subheadline)
        } else if !t.isEmpty {
            Text(markdown(t)).font(.subheadline)
        }
    }

    private func markdown(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(s)
    }
}

/// System Info: the engine's facts (`about.systemInfo`), then this app's own.
struct SystemInfoView: View {
    @Environment(AppModel.self) private var model
    @State private var rows: [JSONValue] = []

    var body: some View {
        List {
            Section {
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    LabeledContent(LocalizedStringKey(row["label"].string ?? ""), value: row["value"].string ?? "")
                        .textSelection(.enabled)
                }
            }
            Section("This App") {
                LabeledContent("App Version", value: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "")
                LabeledContent("Device", value: Platform.deviceDescription)
                if let ms = model.lastDraftRoundTripMs {
                    LabeledContent("Slider to draft", value: String(format: "%.0f ms", ms))
                }
            }
        }
        .navigationTitle("System Info")
        .inlineTitle()
        .task {
            guard let engine = model.engine else { return }
            rows = (try? await engine.execute("about.systemInfo"))?.array ?? []
        }
    }
}
