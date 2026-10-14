#if os(iOS)
import LightCraftCore
import SwiftUI

/// My Photos: All Photos, Recently Added, People, Picks, Recently Deleted, then the albums (folders
/// fold), each with its cover and count. Choosing one shows it in the grid and closes the page.
struct AlbumsSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var expanded: Set<UInt64> = []

    var body: some View {
        NavigationStack {
            List {
                Section {
                    sourceRow("All Photos", "photo.on.rectangle", kind: "all", count: model.library.stats?.photos)
                    sourceRow("Recently Added", "clock", kind: "recentlyAdded", count: nil)
                    Button {
                        dismiss()
                        model.path.append(.people)
                    } label: {
                        AlbumRowLabel(title: String(localized: "People"), symbol: "person.2", cover: nil, count: nil, chosen: false)
                    }
                    sourceRow("Picks", "flag", kind: "picks", count: model.library.stats?.picks)
                    if (model.library.stats?.deleted ?? 0) > 0 {
                        sourceRow("Recently Deleted", "trash", kind: "recentlyDeleted", count: model.library.stats?.deleted)
                    }
                } footer: {
                    if let library = model.opened?.library {
                        Text((library as NSString).lastPathComponent)
                    }
                }
                Section("Albums") {
                    ForEach(model.albums.roots) { node in
                        AlbumTreeRow(node: node, depth: 0, expanded: $expanded) { choose($0) }
                    }
                    if model.albums.roots.isEmpty {
                        Text("No albums yet").foregroundStyle(.secondary)
                    }
                }
                Section {
                    NavigationLink {
                        KeywordListView()
                    } label: {
                        Label("Keywords", systemImage: "tag")
                    }
                }
            }
            .listStyle(.insetGrouped)
            .foregroundStyle(.primary)
            .navigationTitle("Albums")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        Section("Create New") {
                            Button("Album", systemImage: "rectangle.stack") { model.sheet = .newAlbum(parent: nil, folder: false, addSelected: false) }
                            Button("Smart Album…", systemImage: "gearshape.2") { model.sheet = .smartAlbum(nil, parent: nil) }
                            Button("Smart Album from Filter", systemImage: "line.3.horizontal.decrease.circle") { model.sheet = .newSmartAlbumFromView }
                            Button("Folder", systemImage: "folder") { model.sheet = .newAlbum(parent: nil, folder: true, addSelected: false) }
                        }
                        Button("Sort Albums A–Z", systemImage: "arrow.up.arrow.down") { model.perform("album.sort") }
                    } label: {
                        Label("Create New", systemImage: "plus")
                    }
                }
            }
            .task { if let engine = model.engine { await model.albums.reload(engine) } }
        }
    }

    private func sourceRow(_ title: String.LocalizationValue, _ symbol: String, kind: String, count: Int?) -> some View {
        let chosen = model.library.state?.source.kind == kind
        return Button {
            model.perform("library.source", ["kind": .string(kind)])
            dismiss()
        } label: {
            AlbumRowLabel(title: String(localized: title), symbol: symbol, cover: nil, count: model.prefs.showCounts ? count : nil, chosen: chosen)
        }
    }

    private func choose(_ node: AlbumNode) {
        model.perform("library.source", ["kind": "album", "id": .int(Int64(clamping: node.id))])
        dismiss()
    }
}

/// An album in the tree: a folder folds open, an album is chosen; its menu has what can be done to it.
private struct AlbumTreeRow: View {
    @Environment(AppModel.self) private var model
    let node: AlbumNode
    let depth: Int
    @Binding var expanded: Set<UInt64>
    let choose: (AlbumNode) -> Void

    var body: some View {
        let open = expanded.contains(node.id)
        let chosen = model.library.state?.source.kind == "album" && model.library.state?.source.id == node.id
        Button {
            if node.folder {
                withAnimation(.snappy) {
                    if open { expanded.remove(node.id) } else { expanded.insert(node.id) }
                }
            } else {
                choose(node)
            }
        } label: {
            AlbumRowLabel(
                title: node.name,
                symbol: node.folder ? (open ? "folder.fill" : "folder") : (node.smart == true ? "gearshape.2" : "rectangle.stack"),
                cover: node.folder ? nil : node.cover,
                count: model.prefs.showCounts ? (node.folder ? node.children.count : node.count) : nil,
                countLabel: node.folder ? "items" : nil,
                chosen: chosen,
                badge: node.problems?.isEmpty == false ? "exclamationmark.triangle.fill" : (model.albums.isOffline(node.id) ? "checkmark.icloud" : nil)
            )
            .padding(.leading, CGFloat(depth) * 20)
        }
        .contextMenu { AlbumMenu(node: node) }
        if node.folder && open {
            ForEach(node.children) { child in
                AlbumTreeRow(node: child, depth: depth + 1, expanded: $expanded, choose: choose)
            }
        }
    }
}

/// What can be done to an album or folder (its row's long press).
struct AlbumMenu: View {
    @Environment(AppModel.self) private var model
    let node: AlbumNode

    var body: some View {
        let id = JSONValue.int(Int64(clamping: node.id))
        if node.folder {
            Section("New") {
                Button("Album", systemImage: "rectangle.stack") { model.sheet = .newAlbum(parent: node.id, folder: false, addSelected: false) }
                Button("Smart Album…", systemImage: "gearshape.2") { model.sheet = .smartAlbum(nil, parent: node.id) }
                Button("Folder", systemImage: "folder") { model.sheet = .newAlbum(parent: node.id, folder: true, addSelected: false) }
            }
            Button("Sort Contents A–Z", systemImage: "arrow.up.arrow.down") { model.perform("album.sort", ["parent": id]) }
        } else {
            if node.isPlain {
                Button("Add Selected Photos", systemImage: "plus") { model.perform("album.addPhotos", ["id": id]) }
                    .disabled(model.library.selected.isEmpty)
            }
            if node.smart == true {
                Button("Edit Smart Album…", systemImage: "slider.horizontal.3") { model.sheet = .smartAlbum(node.id, parent: nil) }
            }
            if model.albums.sync?.signedIn == true {
                let offline = model.albums.isOffline(node.id)
                Button(offline ? "Remove Offline Copy" : "Make Available Offline", systemImage: offline ? "icloud.slash" : "icloud.and.arrow.down") {
                    model.perform("album.makeAvailableOffline", ["id": id, "on": .bool(!offline)])
                }
                if node.isPlain {
                    Button("Share Link…", systemImage: "link") { model.sheet = .shareLink(node.id, node.name) }
                }
            }
            Button("Export Album…", systemImage: "square.and.arrow.up") {
                Task {
                    await model.run("library.source", ["kind": "album", "id": id])
                    await model.run("library.selectAll")
                    model.sheet = .export(Array(model.library.selected))
                }
            }
        }
        if !model.albums.folders.filter({ $0.id != node.id }).isEmpty {
            Menu("Move to", systemImage: "folder") {
                Button("Top Level") { model.perform("album.move", ["id": id, "parent": .null]) }
                ForEach(model.albums.folders.filter { $0.id != node.id }) { folder in
                    Button(folder.name) { model.perform("album.move", ["id": id, "parent": .int(Int64(clamping: folder.id))]) }
                }
            }
        }
        Button("Rename…", systemImage: "pencil") { model.sheet = .renameAlbum(node.id, node.name) }
        Button("Delete", systemImage: "trash", role: .destructive) { model.perform("album.delete", ["id": id]) }
    }
}

/// An albums-list row: a tile (the cover, or a symbol), the name and the count under it.
struct AlbumRowLabel: View {
    @Environment(AppModel.self) private var model
    let title: String
    let symbol: String
    let cover: UInt64?
    let count: Int?
    var countLabel: String? = nil
    let chosen: Bool
    var badge: String? = nil

    var body: some View {
        HStack(spacing: 12) {
            RoundedRectangle(cornerRadius: 8)
                .fill(Color(uiColor: .tertiarySystemFill))
                .frame(width: 44, height: 44)
                .overlay {
                    if let cover, let picture = model.images.thumb(cover) {
                        Image(decorative: picture.image, scale: 1).resizable().scaledToFill()
                    } else {
                        Image(systemName: symbol).font(.title3).foregroundStyle(.secondary)
                    }
                }
                .clipShape(.rect(cornerRadius: 8))
                .task(id: cover) { if let cover { model.wantThumb(cover, edge: 256) } }
                .onDisappear { if let cover { model.thumbDisappeared(cover) } }
            VStack(alignment: .leading, spacing: 1) {
                Text(title).foregroundStyle(.primary)
                if let count {
                    Text(countLabel == nil ? "\(count)" : "^[\(count) item](inflect: true)").font(.subheadline).foregroundStyle(.secondary)
                }
            }
            Spacer()
            if let badge {
                Image(systemName: badge).foregroundStyle(.secondary)
            }
            if chosen {
                Image(systemName: "checkmark").foregroundStyle(Color.accentColor)
            }
        }
        .contentShape(Rectangle())
    }
}

/// A new album or folder, or a new name for one.
struct AlbumNameSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let title: LocalizedStringKey
    let action: LocalizedStringKey
    @State var name: String
    let done: (String) async -> Void

    var body: some View {
        NavigationStack {
            Form {
                TextField("Name", text: $name)
                    .submitLabel(.done)
                    .onSubmit(confirm)
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(action, action: confirm)
                        .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        .presentationDetents([.medium])
    }

    private func confirm() {
        let n = name.trimmingCharacters(in: .whitespaces)
        guard !n.isEmpty else { return }
        Task {
            await done(n)
            dismiss()
        }
    }
}
#endif
