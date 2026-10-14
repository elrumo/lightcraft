#if os(iOS)
import LightCraftCore
import SwiftUI

/// File ▸ Import Lightroom Catalog…: a Lightroom Classic catalog's folder (picked in Files, with
/// its photos inside or beside it) is read in the background (`library.inspectLightroom`), what it
/// holds is shown, and Import adds its photos, edits, ratings, keywords and collections to the
/// library in the background (`library.importLightroom`, a row in the activity stack). The folder
/// stays open to the app until the import ends.
struct LightroomSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let folder: URL
    @State private var report: JSONValue?
    @State private var error: String?
    @State private var update = false
    @State private var importing = false
    @State private var scoped = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledContent("Folder", value: folder.lastPathComponent)
                    if let catalog = report?["path"].string {
                        LabeledContent("Catalog", value: (catalog as NSString).lastPathComponent)
                    }
                }
                if let report {
                    Section {
                        LabeledContent("Photos", value: (report["photos"].int ?? 0).formatted())
                        LabeledContent("Collections", value: (report["collections"].int ?? 0).formatted())
                        let missing = report["missing"].array?.count ?? 0
                        if missing > 0 {
                            LabeledContent("Originals Not Found", value: missing.formatted())
                        }
                    } footer: {
                        if (report["missing"].array?.count ?? 0) > 0 {
                            Text("Photos whose originals aren't in this folder are skipped. Pick the folder that holds both the catalog and its photos.")
                        }
                    }
                    Section {
                        Toggle("Update Photos Already in the Library", isOn: $update)
                    } footer: {
                        Text("Their edits, ratings and keywords are replaced with the catalog's.")
                    }
                    if let warnings = report["warnings"].array, !warnings.isEmpty {
                        Section {
                            DisclosureGroup {
                                ForEach(Array(warnings.enumerated()), id: \.offset) { _, w in
                                    Text(w.string ?? "").font(.footnote).foregroundStyle(.secondary)
                                }
                            } label: {
                                LabeledContent("Notes", value: warnings.count.formatted())
                            }
                        }
                    }
                } else if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
                    }
                } else {
                    Section {
                        HStack {
                            ProgressView()
                            Text("Reading the catalog…").foregroundStyle(.secondary)
                        }
                    }
                }
            }
            .navigationTitle("Lightroom Classic Catalog")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Import") { start() }
                        .disabled(report == nil || (report?["photos"].int ?? 0) == 0)
                }
            }
        }
        .task {
            scoped = folder.startAccessingSecurityScopedResource()
            guard let engine = model.engine else { return }
            do {
                report = try await engine.execute("library.inspectLightroom", ["path": .string(folder.path)])
            } catch let e as ErrorBody where e.code == "cancelled" {
                _ = e
            } catch {
                self.error = error.localizedDescription
            }
        }
        .onDisappear {
            // closed while reading: stop it; without an import the folder is let go
            if report == nil, error == nil { model.perform("library.inspectLightroom", ["status": true, "cancel": true]) }
            if !importing, scoped { folder.stopAccessingSecurityScopedResource() }
        }
    }

    private func start() {
        importing = true
        let (folder, update, scoped) = (folder, update, scoped)
        Task {
            await model.importLightroom(folder: folder, update: update)
            if scoped { folder.stopAccessingSecurityScopedResource() }
        }
        dismiss()
    }
}

extension AppModel {
    /// Import the catalog in `folder` (in the background, its row in the activity stack); say how it went.
    func importLightroom(folder: URL, update: Bool) async {
        guard let r = await run("library.importLightroom", ["path": .string(folder.path), "updateExisting": .bool(update)]) else { return }
        let report = r["report"].isNull ? r : r["report"]
        let added = report["imported"].int ?? 0
        let failed = report["failed"].array?.count ?? 0
        show(failed == 0 ? String(localized: "Lightroom import complete: \(added) photos added")
                         : String(localized: "Lightroom import complete: \(added) photos added, \(failed) failed"))
    }
}
#endif
