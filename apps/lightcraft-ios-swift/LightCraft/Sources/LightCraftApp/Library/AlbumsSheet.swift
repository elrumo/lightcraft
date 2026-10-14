#if os(iOS)
import LightCraftCore
import SwiftUI

/// My Photos: All Photos, Recently Added, People, Picks, Recently Deleted, then the albums (folders
/// fold), each with its cover and count. Choosing one shows it in the grid and closes the page.
struct AlbumsSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var expanded: Set<UInt64> = []
    /// The iPad's sidebar column rather than a page over the grid (nothing to close).
    var sidebar = false

    var body: some View {
        NavigationStack {
            List {
                Section {
                    sourceRow("All Photos", "photo.on.rectangle", kind: "all", count: model.library.stats?.photos)
                    sourceRow("Recently Added", "clock", kind: "recentlyAdded", count: nil)
                    Button {
                        leave()
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
                // the sections below, as arranged (Edit Sidebar, View ▸ Sidebar Sections)
                ForEach(model.sidebarSections.filter { !$0.hidden && SidebarSectionState.here.contains($0.id) }) { section in
                    sidebarSection(section)
                }
            }
            .listStyle(.insetGrouped)
            .foregroundStyle(.primary)
            .navigationTitle("Albums")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if !sidebar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Close", systemImage: "xmark") { dismiss() }
                    }
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
                        Button("Edit Sidebar…", systemImage: "sidebar.left") { model.sheet = .editSidebar }
                    } label: {
                        Label("Create New", systemImage: "plus")
                    }
                }
            }
            .task {
                await model.arrangeSidebar()
                if let engine = model.engine { await model.albums.reload(engine) }
            }
            .alert(model.albumToDelete.map { String(localized: "Delete “\($0.name)”?") } ?? "",
                   isPresented: Binding(get: { model.albumToDelete != nil }, set: { if !$0 { model.albumToDelete = nil } })) {
                Button("Delete", role: .destructive) {
                    if let a = model.albumToDelete { model.perform("album.delete", ["id": .int(Int64(clamping: a.id))]) }
                    model.albumToDelete = nil
                }
                Button("Cancel", role: .cancel) { model.albumToDelete = nil }
            } message: {
                Text("Photos stay in All Photos. Deleting a folder also deletes its albums.")
            }
        }
    }

    @ViewBuilder private func sidebarSection(_ section: SidebarSectionState) -> some View {
        switch section.id {
        case "albums":
            Section(isExpanded: Binding(get: { !section.collapsed }, set: { open in
                model.arrange("view.sidebarSection", ["section": "albums", "collapsed": .bool(!open)])
            })) {
                ForEach(model.albums.roots) { node in
                    AlbumTreeRow(node: node, depth: 0, expanded: $expanded) { choose($0) }
                }
                if model.albums.roots.isEmpty {
                    Text("No albums yet").foregroundStyle(.secondary)
                }
            } header: {
                // a tap folds the section; an album carried here goes to the top level
                Button {
                    withAnimation(.snappy) { model.arrange("view.sidebarSection", ["section": "albums", "collapsed": .bool(!section.collapsed)]) }
                } label: {
                    HStack {
                        Text("Albums")
                        Spacer()
                        Image(systemName: "chevron.down").rotationEffect(.degrees(section.collapsed ? -90 : 0))
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityValue(section.collapsed ? Text("Collapsed") : Text("Expanded"))
                .dropDestination(for: String.self) { items, _ in AlbumDrag.move(items, into: nil, model) }
                .contextMenu { SidebarSectionActions(id: section.id) }
            }
        default:
            Section {
                NavigationLink {
                    switch section.id {
                    case "byDate": ByDateView()
                    case "folders": LibraryFoldersView()
                    default: KeywordListView()
                    }
                } label: {
                    Label(LocalizedStringKey(section.title), systemImage: section.symbol)
                }
                .contextMenu { SidebarSectionActions(id: section.id) }
            }
        }
    }

    private func sourceRow(_ title: String.LocalizationValue, _ symbol: String, kind: String, count: Int?) -> some View {
        let chosen = model.library.state?.source.kind == kind
        return Button {
            model.perform("library.source", ["kind": .string(kind)])
            leave()
        } label: {
            AlbumRowLabel(title: String(localized: title), symbol: symbol, cover: nil, count: model.prefs.showCounts ? count : nil, chosen: chosen)
        }
    }

    /// After a choice: close the page, or (the sidebar) bring the grid to the front.
    private func leave() {
        if sidebar { model.path = [] } else { dismiss() }
    }

    private func choose(_ node: AlbumNode) {
        model.perform("library.source", ["kind": "album", "id": .int(Int64(clamping: node.id))])
        leave()
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
        // lift an album and carry it into a folder (or onto the Albums heading: the top level)
        .draggable(AlbumDrag.payload(node.id)) {
            // (a drag's preview is drawn apart from the screen, without its environment)
            AlbumRowLabel(title: node.name, symbol: node.folder ? "folder" : "rectangle.stack", cover: node.folder ? nil : node.cover, count: nil, chosen: false)
                .padding(8)
                .environment(model)
        }
        .dropDestination(for: String.self) { items, _ in
            // photos carried here are added to the album; an album, moved into the folder
            let photos = PhotoDrag.ids(items)
            if !node.folder, !photos.isEmpty {
                model.perform("album.addPhotos", ["id": .int(Int64(clamping: node.id)), "ids": AppModel.ids(photos)])
                model.buzz(.medium)
                return true
            }
            return node.folder && AlbumDrag.move(items, into: node.id, model)
        }
        if node.folder && open {
            ForEach(node.children) { child in
                AlbumTreeRow(node: child, depth: depth + 1, expanded: $expanded, choose: choose)
            }
        }
    }
}

/// An album being carried, as the text a drag holds (`lightcraft-album:<id>`).
enum AlbumDrag {
    static func payload(_ id: UInt64) -> String { "lightcraft-album:\(id)" }

    /// Move the albums dropped into `folder` (`nil`: the top level); the engine refuses a folder
    /// into itself or what is inside it.
    static func move(_ items: [String], into folder: UInt64?, _ model: AppModel) -> Bool {
        let ids = items.compactMap { $0.hasPrefix("lightcraft-album:") ? UInt64($0.dropFirst("lightcraft-album:".count)) : nil }
            .filter { $0 != folder }
        for id in ids {
            model.perform("album.move", ["id": .int(Int64(clamping: id)), "parent": folder.map { .int(Int64(clamping: $0)) } ?? .null])
        }
        if !ids.isEmpty { model.buzz(.medium) }
        return !ids.isEmpty
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
        // (asked first: a finger sliding over a long press's menu can choose it)
        Button("Delete", systemImage: "trash", role: .destructive) { model.albumToDelete = node }
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
/// A section of the albums page below My Photos, as `host.sidebar` lists it.
nonisolated struct SidebarSectionState: Decodable, Hashable, Identifiable {
    let id: String
    let hidden: Bool
    let collapsed: Bool

    // ponytail: egui's Local section browses a computer's folders; this app has none (Files imports them)
    /// The sections this app has, in their usual order.
    static let here = ["albums", "byDate", "folders", "keywords"]
    static let usual = here.map { SidebarSectionState(id: $0, hidden: false, collapsed: false) }

    var title: String { ["albums": "Albums", "byDate": "By Date", "folders": "Folders", "keywords": "Keywords"][id] ?? id }
    var symbol: String { ["albums": "rectangle.stack", "byDate": "calendar", "folders": "folder", "keywords": "tag"][id] ?? "square" }
}

extension AppModel {
    /// Show, fold, move or reset the sidebar's sections with one of the shared `view.sidebar*`
    /// commands (`host.sidebar`; none: just read the arrangement); the arrangement is kept in Prefs.
    func arrangeSidebar(_ command: String? = nil, _ params: JSONValue = [:]) async {
        var p: JSONValue = ["layout": prefs.sidebarLayout]
        if let command {
            p["command"] = .string(command)
            p["params"] = params
        }
        guard let r = await request("host.sidebar", p), let sections = try? r["layout"].decode([SidebarSectionState].self) else { return }
        prefs.sidebarLayout = r["layout"]
        withAnimation(.snappy) { sidebarSections = sections }
    }

    func arrange(_ command: String, _ params: JSONValue = [:]) {
        Task { await arrangeSidebar(command, params) }
    }

    /// The sections this app has, hidden ones too, in their order.
    var sidebarHere: [SidebarSectionState] { sidebarSections.filter { SidebarSectionState.here.contains($0.id) } }
}

/// A section's menu: hide it, move it up or down among the shown ones.
private struct SidebarSectionActions: View {
    @Environment(AppModel.self) private var model
    let id: String

    var body: some View {
        let shown = model.sidebarHere.filter { !$0.hidden }.map(\.id)
        let at = shown.firstIndex(of: id)
        Button("Move Up", systemImage: "arrow.up") { model.arrange("view.sidebarMoveSection", ["section": .string(id), "direction": "up"]) }
            .disabled(at == nil || at == 0)
        Button("Move Down", systemImage: "arrow.down") { model.arrange("view.sidebarMoveSection", ["section": .string(id), "direction": "down"]) }
            .disabled(at.map { $0 + 1 >= shown.count } ?? true)
        Button("Hide Section", systemImage: "eye.slash") { model.arrange("view.sidebarSection", ["section": .string(id), "show": false]) }
        Button("Edit Sidebar…", systemImage: "sidebar.left") { model.sheet = .editSidebar }
    }
}

/// Edit Sidebar: which sections show, in what order (drag), and back to the usual arrangement.
struct EditSidebarSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let sections = model.sidebarHere
        NavigationStack {
            List {
                Section {
                    ForEach(sections) { s in
                        Toggle(isOn: Binding(get: { !s.hidden }, set: { model.arrange("view.sidebarSection", ["section": .string(s.id), "show": .bool($0)]) })) {
                            Label(LocalizedStringKey(s.title), systemImage: s.symbol)
                        }
                    }
                    .onMove { from, to in
                        guard let i = from.first, let moved = sections[safe: i] else { return }
                        // just above the one now at `to`, or last
                        let before: JSONValue = sections[safe: to].map { .string($0.id) } ?? .null
                        model.arrange("view.sidebarMoveSection", ["section": .string(moved.id), "before": before])
                    }
                } footer: {
                    Text("Drag a section to move it; hidden ones stay where they are.")
                }
                Section {
                    Button("Reset Sidebar Sections", systemImage: "arrow.counterclockwise") { model.arrange("view.sidebarReset") }
                        .disabled(sections == SidebarSectionState.usual)
                }
            }
            .environment(\.editMode, .constant(.active))
            .navigationTitle("Edit Sidebar")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .task { await model.arrangeSidebar() }
    }
}

/// View ▸ Sidebar Sections on the iPad's menu bar: each section shown or hidden, and Reset.
struct SidebarSectionsMenu: View {
    let model: AppModel

    var body: some View {
        Menu("Sidebar Sections") {
            ForEach(model.sidebarHere) { s in
                Toggle(LocalizedStringKey(s.title), isOn: Binding(get: { !s.hidden }, set: { model.arrange("view.sidebarSection", ["section": .string(s.id), "show": .bool($0)]) }))
            }
            Divider()
            Button("Reset Sidebar Sections") { model.arrange("view.sidebarReset") }
                .disabled(model.sidebarHere == SidebarSectionState.usual)
        }
    }
}
#endif
