#if os(iOS)
import LightCraftCore
import SwiftUI

/// The navigation root: the library, the photo and other screens pushed over it, the pages that
/// slide up (`model.sheet`), and the messages over everything.
struct RootView: View {
    @Environment(AppModel.self) private var model
    @Namespace private var photos

    var body: some View {
        @Bindable var model = model
        NavigationStack(path: $model.path) {
            Group {
                if let error = model.startError {
                    ContentUnavailableView("LightCraft couldn't start", systemImage: "exclamationmark.triangle", description: Text(error))
                } else if model.engine == nil {
                    ProgressView("Opening your library…")
                } else {
                    LibraryScreen(namespace: photos)
                }
            }
            .navigationDestination(for: Route.self) { route in
                switch route {
                case .photo(let id):
                    PhotoScreen(start: id)
                        .navigationTransition(.zoom(sourceID: id, in: photos))
                case .commands: AllCommandsView()
                case .people: PeopleView()
                case .person(let name): PersonView(name: name)
                case .map: MapScreen()
                }
            }
        }
        .sheet(item: $model.sheet) { sheet in
            SheetContent(sheet: sheet)
                .environment(model)
        }
        .alert(deleteTitle, isPresented: Binding(get: { model.confirmDelete != nil }, set: { if !$0 { model.confirmDelete = nil } })) {
            Button("Delete", role: .destructive) {
                if let ids = model.confirmDelete {
                    model.perform("photo.delete", ["ids": AppModel.ids(ids)])
                    if model.selectMode { model.perform("library.select", ["ids": []]) }
                }
                model.confirmDelete = nil
            }
            Button("Cancel", role: .cancel) { model.confirmDelete = nil }
        } message: {
            Text("They can be restored from Recently Deleted until it is emptied.")
        }
        .overlay(alignment: .top) {
            ToastView()
        }
        .overlay(alignment: .bottom) {
            ActivityHUD()
        }
        .preferredColorScheme(model.prefs.appearance.scheme)
        .haptic(trigger: model.haptic) { _, h in h.seq == 0 ? nil : h.feedback }
    }

    private var deleteTitle: String {
        let n = model.confirmDelete?.count ?? 0
        return n == 1 ? String(localized: "Move this photo to Recently Deleted?") : String(localized: "Move these \(n) photos to Recently Deleted?")
    }
}

/// The page a `Sheet` names.
struct SheetContent: View {
    @Environment(AppModel.self) private var model
    let sheet: Sheet

    var body: some View {
        switch sheet {
        case .albums: AlbumsSheet()
        case .filter: FilterSheet()
        case .settings(let section): SettingsView(section: section)
        case .export(let ids): ExportSheet(ids: ids)
        case .syncStatus: SyncStatusSheet()
        case .syncChoice: SyncChoiceSheet()
        case .newAlbum(let parent, let folder, let addSelected):
            AlbumNameSheet(title: folder ? "New Folder" : "New Album", action: "Create", name: "") { name in
                var p: [String: JSONValue] = ["name": .string(name), "folder": .bool(folder), "addSelected": .bool(addSelected)]
                if let parent { p["parent"] = .int(Int64(clamping: parent)) }
                await model.run("album.create", .object(p))
            }
        case .renameAlbum(let id, let name):
            AlbumNameSheet(title: "Rename", action: "Rename", name: name) { new in
                await model.run("album.rename", ["id": .int(Int64(clamping: id)), "name": .string(new)])
            }
        case .shareLink(let id, let name): ShareLinkSheet(album: id, name: name)
        case .about: AboutView()
        case .keywords: KeywordsSheet()
        case .importReview: ImportReviewSheet()
        case .history(let versions): HistorySheet(versions: versions)
        case .copySettings: CopySettingsSheet(pasteOnto: nil)
        case .pasteSettings(let ids): CopySettingsSheet(pasteOnto: ids)
        case .superRes: SuperResSheet()
        case .createPreset: CreatePresetSheet()
        case .smartAlbum(let id, let parent): SmartAlbumSheet(album: id, parent: parent)
        case .allMetadata: AllMetadataSheet()
        case .activity: ActivityPage()
        case .newSmartAlbumFromView:
            // the grid as it is shown (source and filter) becomes a smart album
            AlbumNameSheet(title: "New Smart Album", action: "Create", name: "") { name in
                await model.run("album.createSmart", ["name": .string(name)])
            }
        }
    }
}

/// The last message, for a few seconds, as a glass capsule.
struct ToastView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let toast = model.toast {
                Text(toast.text)
                    .font(.callout)
                    .multilineTextAlignment(.center)
                    .foregroundStyle(toast.error ? Color.red : Color.primary)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 10)
                    .glassEffect(.regular, in: .capsule)
                    .padding(.horizontal, 24)
                    .padding(.top, 54)
                    .onTapGesture { model.toast = nil }
                    .task(id: toast.id) {
                        try? await Task.sleep(for: .seconds(toast.error ? 6 : 3))
                        if model.toast?.id == toast.id {
                            withAnimation { model.toast = nil }
                        }
                    }
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .accessibilityAddTraits(.isStaticText)
            }
        }
        .animation(.snappy, value: model.toast)
    }
}

/// What runs in the background (an import, a scan, an export when its page is closed) as a glass
/// capsule over the bottom: how far it is, and ✕.
struct ActivityHUD: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let rows = model.activity.filter { ["import", "scan", "previews", "download", "superRes"].contains($0.kind) || ($0.kind == "export" && model.sheet?.id != "export") }
        VStack(spacing: 6) {
            ForEach(rows) { row in
                HStack(spacing: 10) {
                    if row.total > 0 {
                        ProgressView(value: Double(row.done), total: Double(row.total)).frame(width: 70)
                    } else {
                        ProgressView()
                    }
                    Group {
                        if row.total > 0 {
                            Text("\(Text(LocalizedStringKey(row.label))) \(row.done)/\(row.total)")
                        } else {
                            Text(LocalizedStringKey(row.label))
                        }
                    }
                    .font(.footnote).lineLimit(1)
                    if row.cancellable {
                        Button("Cancel", systemImage: "xmark.circle.fill") {
                            model.perform("activity.cancel", ["id": .int(Int64(clamping: row.id))])
                        }
                        .labelStyle(.iconOnly)
                        .disabled(row.cancelling)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .glassEffect(.regular, in: .capsule)
            }
        }
        .padding(.bottom, model.path.isEmpty ? 84 : 120)
        .animation(.snappy, value: rows.map(\.id))
        // a tap shows everything that is running
        .onTapGesture { model.sheet = .activity }
        .accessibilityHint("Shows everything that is running")
    }
}

/// Everything running in the background (the activity stack): each task with what it is on, its
/// progress and Cancel.
struct ActivityPage: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                ForEach(model.activity) { row in
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Text(LocalizedStringKey(row.label)).font(.headline)
                            Spacer()
                            if row.cancellable {
                                Button(row.cancelling ? "Stopping…" : "Cancel", role: .destructive) {
                                    model.perform("activity.cancel", ["id": .int(Int64(clamping: row.id))])
                                }
                                .buttonStyle(.bordered)
                                .disabled(row.cancelling)
                            }
                        }
                        if row.total > 0 {
                            ProgressView(value: Double(row.done), total: Double(row.total)) {
                                Text(progress(row)).font(.caption).monospacedDigit()
                            }
                        } else {
                            ProgressView().frame(maxWidth: .infinity, alignment: .leading)
                        }
                        if !row.detail.isEmpty {
                            Text(row.detail).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                        }
                    }
                    .padding(.vertical, 4)
                }
                if model.activity.isEmpty {
                    ContentUnavailableView("Nothing Running", systemImage: "checkmark.circle", description: Text("Imports, exports, syncing and other long work show here while they run."))
                }
            }
            .navigationTitle("Activity")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done", systemImage: "checkmark") { dismiss() } } }
        }
        .presentationDetents([.medium, .large])
    }

    private func progress(_ row: ActivityTask) -> String {
        if row.unit == "bytes" {
            let f = ByteCountFormatter()
            return "\(f.string(fromByteCount: Int64(clamping: row.done))) / \(f.string(fromByteCount: Int64(clamping: row.total)))"
        }
        return "\(row.done) / \(row.total)"
    }
}

/// Where syncing with the server stands: tinted while it works or on a problem; a tap shows more.
struct CloudButton: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let sync = model.state.sync
        Button {
            model.sheet = .syncStatus
        } label: {
            Image(systemName: symbol(sync?.state))
                .symbolEffect(.pulse, isActive: sync?.state == "syncing")
                .foregroundStyle(tint(sync?.state))
        }
        .accessibilityLabel("Sync Status")
        .accessibilityValue(sync?.state ?? "")
    }

    private func symbol(_ state: String?) -> String {
        switch state {
        case "syncing": "arrow.triangle.2.circlepath.icloud"
        case "error", "conflict": "exclamationmark.icloud"
        case "paused": "pause.circle"
        case "signedOut": "icloud.slash"
        default: model.state.unsaved > 0 ? "exclamationmark.icloud" : "checkmark.icloud"
        }
    }

    private func tint(_ state: String?) -> Color {
        switch state {
        case "syncing": .accentColor
        case "error", "conflict": .orange
        default: model.state.unsaved > 0 ? .orange : .primary
        }
    }
}
#endif
