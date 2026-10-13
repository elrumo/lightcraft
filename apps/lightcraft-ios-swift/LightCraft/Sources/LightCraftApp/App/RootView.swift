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
            Text("They stay in Recently Deleted for 30 days.")
        }
        .overlay(alignment: .top) {
            ToastView()
        }
        .preferredColorScheme(model.prefs.appearance.scheme)
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
        case .importReview: EmptyView()
        case .history(let versions): HistorySheet(versions: versions)
        case .copySettings: CopySettingsSheet(pasteOnto: nil)
        case .pasteSettings(let ids): CopySettingsSheet(pasteOnto: ids)
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
