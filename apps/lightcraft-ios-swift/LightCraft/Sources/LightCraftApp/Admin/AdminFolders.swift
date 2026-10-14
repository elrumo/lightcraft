#if os(iOS)
import LightCraftCore
import SwiftUI

/// A user's library folders: photo folders already on the server (a NAS share, a backup disk) that
/// show in their library; the names the scan skips; and the scan, with its progress.
struct AdminFolders: View {
    let name: String
    let admin: AdminModel
    @State private var folders: JSONValue = .null
    @State private var loaded = false
    @State private var tick = 0
    @State private var adding = false
    @State private var editing: FolderRef?
    @State private var skipping = false

    var body: some View {
        let list = folders["folders"].array ?? []
        let scan = folders["scan"]
        let info = Self.scanInfo(scan)
        List {
            Section {
                if loaded && list.isEmpty {
                    Text("No library folders yet. Add a folder to show photos that are already on the server.").foregroundStyle(.secondary)
                }
                ForEach(list, id: \.self) { f in
                    row(f)
                        .swipeActions {
                            Button("Remove") { admin.offer = remove(f) }.tint(.red)
                            Button("Writing") { editing = FolderRef(folder: f) }.tint(.blue)
                        }
                        .contextMenu {
                            Button("Writing…", systemImage: "key") { editing = FolderRef(folder: f) }
                            Button("Remove", systemImage: "trash", role: .destructive) { admin.offer = remove(f) }
                        }
                }
                Button("Add Folder…", systemImage: "folder.badge.plus") { adding = true }
            } header: {
                Text("Library Folders")
            } footer: {
                Text("Photo folders already on the server, like a NAS share or a backup disk, shown in this user's library on every device. Photos are read where they are: never copied, moved or changed. If you allow it, the server also writes XMP sidecars beside them and can file uploaded photos here.")
            }
            if !list.isEmpty {
                Section {
                    let skipped = (folders["ignore"].array ?? []).compactMap(\.string)
                    if skipped.isEmpty {
                        Text("Nothing is left out. Name what to skip, like *.fcpbundle for Final Cut Pro bundles.").foregroundStyle(.secondary)
                    } else {
                        Text(verbatim: skipped.joined(separator: ", ")).font(.footnote.monospaced())
                    }
                    Button("Edit Skipped Names…", systemImage: "pencil") { skipping = true }
                } header: {
                    Text("Skipped Names")
                } footer: {
                    Text("Left out of the scan, with everything inside.")
                }
                Section {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(info.title).font(.headline)
                        Text(info.text).font(.subheadline).foregroundStyle(.secondary)
                        switch info.progress {
                        case .none: EmptyView()
                        case .indeterminate: ProgressView()
                        case .fraction(let f): ProgressView(value: min(max(f, 0), 1))
                        }
                    }
                    Button("Scan Now", systemImage: "arrow.clockwise") { scanNow() }
                        .disabled(scan["scanning"].bool == true)
                    let errors = (scan["errors"].array ?? []).compactMap(\.string)
                        + (scan["previewErrors"].array ?? []).compactMap(\.string).map { String(localized: "Preview: \($0)") }
                    if !errors.isEmpty {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Some files couldn't be read").font(.subheadline.weight(.semibold))
                            ForEach(errors, id: \.self) { Text(verbatim: $0).font(.caption) }
                        }
                        .foregroundStyle(.red)
                    }
                } header: {
                    Text("Scan")
                }
            }
        }
        .navigationTitle("Library Folders")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $adding, onDismiss: { tick += 1 }) { AdminAddFolder(name: name, admin: admin) }
        .sheet(item: $editing, onDismiss: { tick += 1 }) { AdminFolderMode(name: name, folder: $0.folder, admin: admin) }
        .sheet(isPresented: $skipping, onDismiss: { tick += 1 }) { AdminSkipped(name: name, names: (folders["ignore"].array ?? []).compactMap(\.string), admin: admin) }
        .adminPage(admin)
        .adminTask(admin, tick) { await load() }
        .refreshable { await load() }
    }

    /// A folder: its name and path, whether the server can see it, and what it may do there.
    private func row(_ f: JSONValue) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Label(f["name"].string ?? "", systemImage: "folder").font(.headline)
            Text(verbatim: f["path"].string ?? "").font(.caption.monospaced()).foregroundStyle(.secondary)
            HStack(spacing: 10) {
                if f["there"].bool == true {
                    Text("Available").foregroundStyle(.green)
                } else {
                    Text("Not found").foregroundStyle(.red)
                }
                if let why = f["writeError"].string {
                    Text("Can't write").foregroundStyle(.red)
                    Text(why).foregroundStyle(.secondary)
                } else if f["imports"].bool == true {
                    Text("Takes uploads").foregroundStyle(.blue)
                } else if f["writable"].bool == true {
                    Text("Writable").foregroundStyle(.blue)
                } else {
                    Text("Read-only").foregroundStyle(.secondary)
                }
            }
            .font(.caption)
        }
    }

    /// The folders, and again while a scan or the previews are going on (every 2 s).
    private func load() async {
        while !Task.isCancelled {
            guard await admin.attempt({ folders = try await admin.get("users", name, "folders") }) else { return }
            loaded = true
            let scan = folders["scan"]
            guard scan["scanning"].bool == true || (scan["previews"].int ?? 0) > 0 else { return }
            try? await Task.sleep(for: .seconds(2))
        }
    }

    private func scanNow() {
        Task {
            await admin.attempt { folders = try await admin.call("POST", ["users", name, "scan"]) }
            tick += 1
        }
    }

    private func remove(_ f: JSONValue) -> AdminOffer {
        let folder = f["name"].string ?? ""
        return AdminOffer(
            title: String(localized: "Stop reading \(folder)?"),
            text: String(localized: "Its photos stay in \(name)'s library with their edits, but their originals can no longer be downloaded. Nothing in \(f["path"].string ?? "") is touched."),
            ok: String(localized: "Remove Folder"),
            danger: true
        ) {
            await admin.attempt { try await admin.call("DELETE", ["users", name, "folders", folder]) }
            tick += 1
        }
    }

    /// A folder whose writing is being changed (what `sheet(item:)` needs).
    struct FolderRef: Identifiable {
        let id = UUID()
        let folder: JSONValue
    }

    enum ScanProgress { case none, indeterminate, fraction(Double) }

    /// What the scan is doing, in words, and how far it is.
    static func scanInfo(_ s: JSONValue) -> (title: String, text: String, progress: ScanProgress) {
        let n = AdminFormat.number
        let eta = (s["etaSecs"].int ?? 0) > 0 ? AdminFormat.duration(s["etaSecs"].int ?? 0) : nil
        if s["scanning"].bool == true {
            if s["phase"].string == "reading", let todo = s["todo"].int, todo > 0 {
                let text = eta.map { String(localized: "Reading \(n(s["done"])) of \(n(s["todo"])) new or changed files, about \($0) left.") }
                    ?? String(localized: "Reading \(n(s["done"])) of \(n(s["todo"])) new or changed files.")
                return (String(localized: "Scanning…"), text, .fraction(Double(s["done"].int ?? 0) / Double(todo)))
            }
            if s["phase"].string == "listing" {
                return (String(localized: "Scanning…"), String(localized: "Looking for photos: \(n(s["files"])) found so far."), .indeterminate)
            }
            return (String(localized: "Scanning…"), String(localized: "Finishing up."), .indeterminate)
        }
        if let previews = s["previews"].int, previews > 0 {
            let total = s["previewsTotal"].int.flatMap { $0 > 0 ? $0 : nil } ?? previews
            let done = n(s["previewsDone"])
            let eta = (s["previewsEtaSecs"].int ?? 0) > 0 ? AdminFormat.duration(s["previewsEtaSecs"].int ?? 0) : nil
            let text = eta.map { String(localized: "\(done) of \(total) done, about \($0) left. Photos show on devices as their previews are ready.") }
                ?? String(localized: "\(done) of \(total) done. Photos show on devices as their previews are ready.")
            return (String(localized: "Building previews"), text, .fraction(Double(s["previewsDone"].int ?? 0) / Double(max(total, 1))))
        }
        guard let last = s["lastScan"].int, last > 0 else {
            return (String(localized: "Not scanned yet"), String(localized: "Scan to add the photos in these folders to the library."), .none)
        }
        var changes: [String] = []
        if let v = s["added"].int, v > 0 { changes.append(String(localized: "\(v) new")) }
        if let v = s["moved"].int, v > 0 { changes.append(String(localized: "\(v) moved")) }
        if let v = s["changed"].int, v > 0 { changes.append(String(localized: "\(v) changed")) }
        if let v = s["linked"].int, v > 0 { changes.append(String(localized: "\(v) uploaded before")) }
        if let v = s["missing"].int, v > 0 { changes.append(String(localized: "\(v) missing")) }
        if let v = s["failed"].int, v > 0 { changes.append(String(localized: "\(v) not read")) }
        let files = s["files"].int ?? 0
        let count = files == 1 ? String(localized: "1 photo file.") : String(localized: "\(files) photo files.")
        let what = changes.isEmpty ? String(localized: "No changes.") : String(localized: "Changes: \(changes.joined(separator: ", ")).")
        return (String(localized: "Last scanned \(AdminFormat.ago(last))"), "\(count) \(what)", .none)
    }
}

/// Add a library folder: a path the server can read, and what it may do there.
struct AdminAddFolder: View {
    let name: String
    let admin: AdminModel
    @Environment(\.dismiss) private var dismiss
    @State private var path = ""
    @State private var label = ""
    @State private var writable = false
    @State private var imports = false
    @State private var working = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Folder on the server", text: $path, prompt: Text(verbatim: "/photos/\(name)"))
                        .font(.body.monospaced())
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } footer: {
                    Text("A path the server can read. With Docker, mount the folder into the container read-only first (-v /srv/photos:/photos:ro) and enter /photos/…")
                }
                Section {
                    TextField("Name on devices (optional)", text: $label)
                } footer: {
                    Text("The folder's own name if left empty.")
                }
                Section {
                    Toggle("Let the server write XMP sidecars here", isOn: $writable)
                } footer: {
                    Text("Off: the folder is only read (and may be a read-only mount). On: edits made on devices are written beside the photos as .xmp files, for Lightroom and other programs. The photos themselves are never changed.")
                }
                Section {
                    Toggle("File uploaded photos here", isOn: $imports)
                } footer: {
                    Text("Photos devices upload are filed in this folder as year/date/name instead of kept in the server's own store. One folder per user; needs writing.")
                }
            }
            .navigationTitle("Add a Library Folder")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") { add() }.disabled(working || path.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
            .adminPage(admin)
        }
    }

    private func add() {
        working = true
        Task {
            let named = label.trimmingCharacters(in: .whitespaces)
            let body: JSONValue = [
                "path": .string(path.trimmingCharacters(in: .whitespaces)),
                "name": named.isEmpty ? .null : .string(named),
                "writable": .bool(writable || imports),
                "imports": .bool(imports),
            ]
            if await admin.attempt({ try await admin.call("POST", ["users", name, "folders"], body) }) { dismiss() }
            working = false
        }
    }
}

/// What the server may do in a folder: write XMP sidecars, file uploads.
struct AdminFolderMode: View {
    let name: String
    let folder: JSONValue
    let admin: AdminModel
    @Environment(\.dismiss) private var dismiss
    @State private var writable = false
    @State private var imports = false
    @State private var working = false

    var body: some View {
        let label = folder["name"].string ?? ""
        NavigationStack {
            Form {
                Section {
                    Toggle("Write XMP sidecars here", isOn: $writable)
                } footer: {
                    if let why = folder["writeError"].string {
                        Text("The server never changes, moves or deletes the photos. With writing on, it adds and updates .xmp sidecar files beside them — but it cannot write there now: \(why)")
                    } else {
                        Text("The server never changes, moves or deletes the photos. With writing on, it adds and updates .xmp sidecar files beside them. Mount the folder read-write, with the server user allowed to write, before turning this on.")
                    }
                }
                Section {
                    Toggle("File uploaded photos here", isOn: $imports)
                } footer: {
                    Text("One folder per user; turns writing on.")
                }
            }
            .navigationTitle(label)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Save") { save(label) }.disabled(working) }
            }
            .onAppear {
                writable = folder["writable"].bool == true
                imports = folder["imports"].bool == true
            }
            .adminPage(admin)
        }
    }

    private func save(_ label: String) {
        working = true
        Task {
            let body: JSONValue = ["writable": .bool(writable || imports), "imports": .bool(imports)]
            if await admin.attempt({ try await admin.call("PUT", ["users", name, "folders", label], body) }) { dismiss() }
            working = false
        }
    }
}

/// The names the scan skips, one per line.
struct AdminSkipped: View {
    let name: String
    let names: [String]
    let admin: AdminModel
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var working = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextEditor(text: $text)
                        .font(.body.monospaced())
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .frame(minHeight: 160)
                } header: {
                    Text("One Name per Line")
                } footer: {
                    Text("The scan skips any file or folder with one of these names, wherever it is in \(name)'s library folders, and everything inside it. * stands for any text and ? for one character; capitals do not matter. Photos already in the library stay there, but their files show as missing.")
                }
            }
            .navigationTitle("Names to Leave Out")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Save") { save() }.disabled(working) }
            }
            .onAppear { text = names.joined(separator: "\n") }
            .adminPage(admin)
        }
    }

    private func save() {
        working = true
        Task {
            let lines = text.split(separator: "\n", omittingEmptySubsequences: false).map { JSONValue.string(String($0)) }
            if await admin.attempt({ try await admin.call("PUT", ["users", name, "ignore"], ["ignore": .array(lines)]) }) { dismiss() }
            working = false
        }
    }
}
#endif
