#if os(iOS)
import LightCraftCore
import PhotosUI
import SwiftUI

/// The library: the collection's name as the title (its menu switches collection or opens the
/// albums), Select, Filter and "…" above, the filters as chips under the title, the grid (All,
/// Months or Years) and, in the bottom bar, Search, the span and the + button. Choosing photos
/// turns the bars into Cancel / Select All and the selection's actions.
struct LibraryScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(\.horizontalSizeClass) private var horizontalSize
    let namespace: Namespace.ID
    @State private var jump: Int?
    @State private var importer = ImportModel()
    @State private var scope = SearchScope.text
    /// This device or the sync server can search by description (`vision.model.status`).
    @State private var describes = false
    @State private var painterText = ""
    @State private var dropping = false
    /// What narrows the grid (`library.filterChips`), and the counts.
    @State private var chips: JSONValue = .null

    var body: some View {
        @Bindable var model = model
        Group {
            switch model.prefs.gridSpan {
            case .all: GridView(namespace: namespace, jump: $jump)
            case .months, .years: DateBlocksView(span: model.prefs.gridSpan) { index in
                withAnimation(.snappy) { model.prefs.gridSpan = .all }
                jump = index
            }
            }
        }
        .onChange(of: model.gridJump) { _, index in
            if let index {
                jump = index
                model.gridJump = nil
            }
        }
        .safeAreaBar(edge: .top) {
            if !model.selectMode { FilterChips(chips: chips["chips"].array ?? []) }
        }
        // pictures dropped from another app (Files, Photos, Safari…) are imported
        .dropDestination(for: GridDrop.self) { items, _ in
            let files = items.compactMap { if case .file(let f) = $0 { f } else { nil } }
            importer.dropped(files, model)
            return !files.isEmpty
        } isTargeted: { dropping = $0 }
        .overlay {
            if dropping {
                RoundedRectangle(cornerRadius: 16).stroke(.tint, lineWidth: 3).padding(4).allowsHitTesting(false)
            }
        }
        .alert("Keyword Painter", isPresented: $model.askPainter) {
            TextField("Keyword", text: $painterText)
            Button("Paint") {
                let k = painterText.trimmingCharacters(in: .whitespaces)
                if !k.isEmpty { model.painting = k }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Tap photos to give them the keyword; tap again to take it away.")
        }
        .navigationTitle(model.selectMode ? selectTitle : model.painting.map { String(localized: "Painting “\($0)”") } ?? title)
        .navigationSubtitle(model.selectMode || model.painting != nil ? Text(verbatim: "") : Text(countText))
        // (inline: the title's chevron shows its menu, and the date headers pin under it)
        .navigationBarTitleDisplayMode(.inline)
        // (on a view of its own: switching it on and off mustn't rebuild the grid)
        .background { Color.clear.modifier(TitleMenu(on: !model.selectMode && model.painting == nil)) }
        .toolbar { toolbar }
        .animation(.snappy, value: model.selectMode)
        .animation(.snappy, value: model.painting)
        .searchable(text: $model.searchText, isPresented: $model.searching, prompt: Text(scope == .describe ? "Describe a photo…" : "Search Photos"))
        .searchToolbarBehavior(.minimize)
        .modifier(DescribeScopes(describes: describes, scope: $scope))
        .onSubmit(of: .search) {
            let q = model.searchText.trimmingCharacters(in: .whitespaces)
            if scope == .describe, !q.isEmpty { model.perform("library.search", ["q": .string(q)]) }
        }
        // text: the grid narrows as it is typed; a description is searched on Return
        .task(id: model.searchText) {
            let search = model.searchText
            guard scope == .text, search != (model.library.state?.filter["text"].string ?? "") else {
                if scope == .describe, search.isEmpty { leaveDescribed() }
                return
            }
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            model.perform("library.filter", ["text": .string(search)])
        }
        // the other kind of search starts afresh: its words meant something else
        .onChange(of: scope) {
            model.searchText = ""
            model.perform("library.filter", ["text": "", "semantic": .null, "only": []])
        }
        // closed: the grid as it was
        .onChange(of: model.searching) { _, open in
            if !open {
                model.searchText = ""
                leaveDescribed()
            }
        }
        .task(id: model.settledRevision) {
            guard let engine = model.engine else { return }
            describes = (try? await engine.execute("vision.model.status"))?["available"].bool ?? false
        }
        .task(id: ChipsKey(gen: model.state.visibleGen, count: model.state.visibleCount)) {
            guard let engine = model.engine else { return }
            chips = (try? await engine.execute("library.filterChips")) ?? .null
        }
        .photosPicker(isPresented: $importer.pickingPhotos, selection: $importer.picked, matching: .any(of: [.images, .not(.livePhotos)]), preferredItemEncoding: .current, photoLibrary: .shared())
        .fileImporter(isPresented: $importer.pickingFiles, allowedContentTypes: ImportModel.fileTypes, allowsMultipleSelection: true) { result in
            importer.files(result, model)
        }
        .fileImporter(isPresented: $importer.pickingFolder, allowedContentTypes: [.folder], allowsMultipleSelection: false) { result in
            importer.folder(result, model)
        }
        .onChange(of: importer.picked) { _, items in
            importer.photos(items, model)
        }
    }

    /// Out of a search by description: the grid as it was.
    private func leaveDescribed() {
        if model.library.state?.filter["semantic"].isNull == false {
            model.perform("library.filter", ["semantic": .null, "only": []])
        }
    }

    private nonisolated struct ChipsKey: Hashable {
        let gen: UInt64
        let count: Int
    }

    private var selectTitle: String {
        if let album = model.addingTo, let node = model.albums.node(album) {
            return String(localized: "Add to \(node.name)")
        }
        let n = model.library.selected.count
        return n == 0 ? String(localized: "Select Photos") : String(localized: "\(n) Selected")
    }

    /// The collection's name: a built-in one in the reader's language, an album or folder as named.
    private var title: String {
        guard let state = model.library.state else { return String(localized: "All Photos") }
        if ["album", "folder", "libraryFolder"].contains(state.source.kind) { return state.sourceLabel }
        return Bundle.main.localizedString(forKey: state.sourceLabel, value: state.sourceLabel, table: nil)
    }

    /// "14 of 120 photos" when a filter narrows the source, else "14 photos".
    private var countText: String {
        let n = model.library.count
        if let total = chips["total"].int, !(chips["chips"].array ?? []).isEmpty, Int(total) != n {
            return String(localized: "\(n) of \(Int(total)) photos")
        }
        return String(localized: "\(n) photos")
    }

    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        if model.selectMode {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { endSelecting() }
            }
            ToolbarItem(placement: .topBarTrailing) {
                if model.library.selected.count < model.library.count {
                    Button("Select All") { model.perform("library.selectAll") }
                } else {
                    Button("Deselect All") { model.perform("library.select", ["ids": []]) }
                }
            }
            if let album = model.addingTo {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") { Task { await SelectionBar.add(Array(model.library.selected), to: album, model) } }
                        .disabled(model.library.selected.isEmpty)
                }
            } else {
                SelectionBar(model: model)
            }
        } else if model.painting != nil {
            ToolbarItem(placement: .confirmationAction) {
                Button("Done") { model.painting = nil }
                    .keyboardShortcut(.cancelAction)
            }
        } else {
            // a phone's albums are a page over the grid (an iPad's are in the sidebar)
            if horizontalSize != .regular {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Albums", systemImage: "rectangle.stack") { model.sheet = .albums }
                }
                .duoPriority(.high)
            }
            if model.albums.sync.map({ $0.state != "off" }) == true {
                ToolbarItem(placement: .topBarTrailing) {
                    CloudButton()
                }
                // (it says something while it works or when something is wrong; otherwise it can go first)
                .duoPriority(["syncing", "error", "conflict"].contains(model.state.sync?.state ?? "") || model.state.unsaved > 0 ? .high : .low)
                ToolbarSpacer(.fixed, placement: .topBarTrailing)
            }
            if model.library.count > 0 {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Select") {
                        model.selectMode = true
                        model.perform("library.select", ["ids": []])
                    }
                }
                ToolbarSpacer(.fixed, placement: .topBarTrailing)
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button("Filter", systemImage: model.library.state?.isFiltered == true ? "line.3.horizontal.decrease.circle.fill" : "line.3.horizontal.decrease") {
                    model.sheet = .filter
                }
            }
            .duoPriority(.high)
            // the grid's "…": the system's overflow menu on iOS 27, so these and what overflowed are in one place
            DuoOverflow { GridMoreItems() }
            // a phone's search waits in the bottom bar as a button; an iPad's is in the top bar
            if horizontalSize != .regular {
                DefaultToolbarItem(kind: .search, placement: .bottomBar)
            }
            ToolbarSpacer(.flexible, placement: .bottomBar)
            if model.library.count > 0 || model.plainAlbum != nil {
                ToolbarItem(placement: .bottomBar) {
                    Picker("Show", selection: Bindable(model.prefs).gridSpan) {
                        ForEach(Prefs.GridSpan.allCases) { span in
                            Text(span.label).tag(span)
                        }
                    }
                    .pickerStyle(.segmented)
                    .fixedSize()
                    .haptic(.selection, trigger: model.prefs.gridSpan)
                }
                // the segmented control draws its own glass capsule; the bar's would be a second one around it
                .sharedBackgroundVisibility(.hidden)
                ToolbarSpacer(.flexible, placement: .bottomBar)
            }
            ToolbarItem(placement: .bottomBar) {
                addButton
            }
            .duoPriority(.high)
        }
    }

    private func endSelecting() {
        model.selectMode = false
        if let album = model.addingTo {
            model.addingTo = nil
            model.perform("library.source", ["kind": "album", "id": .int(Int64(clamping: album))])
        }
    }

    private var addButton: some View {
        Menu {
            Section("Add Photos From") {
                if let album = model.plainAlbum {
                    Button("LightCraft", systemImage: "photo.stack") { startAdding(album) }
                }
                Button("Photos…", systemImage: "photo.on.rectangle") { importer.pickingPhotos = true }
                Button("Files…", systemImage: "folder") { importer.pickingFiles = true }
                Button("Folder from Files…", systemImage: "folder.badge.plus") { importer.pickingFolder = true }
                Button("Connected Camera/Card", systemImage: "sdcard") {
                    if model.cameras.devices.isEmpty {
                        model.show(String(localized: "No camera or card connected: plug in a camera or a card reader (USB-C or Lightning), then try again"))
                    } else {
                        model.sheet = .cameraImport
                    }
                }
                Button("Lightroom Classic Catalog…", systemImage: "square.stack.3d.up") { model.picker = .lightroom }
            }
        } label: {
            Label("Add Photos", systemImage: "plus")
        }
        .buttonStyle(.glassProminent)
    }

    /// The album's + ▸ LightCraft: All Photos to choose from, Add in the top bar.
    private func startAdding(_ album: UInt64) {
        model.addingTo = album
        model.selectMode = true
        model.perform("library.source", ["kind": "all"])
        model.perform("library.select", ["ids": []])
    }
}

/// The title's menu (`SourceMenu`), while neither choosing nor painting.
private struct TitleMenu: ViewModifier {
    let on: Bool

    func body(content: Content) -> some View {
        if on {
            content.toolbarTitleMenu { SourceMenu() }
        } else {
            content
        }
    }
}

/// Words, or a description (when this device or the sync server can search by one), as the
/// search's scopes.
private struct DescribeScopes: ViewModifier {
    let describes: Bool
    @Binding var scope: SearchScope

    func body(content: Content) -> some View {
        if describes {
            content.searchScopes($scope, activation: .onSearchPresentation) {
                Text("Words").tag(SearchScope.text)
                Text("Describe").tag(SearchScope.describe)
            }
        } else {
            content
        }
    }
}

/// The title's menu: the built-in collections (the one shown ticked), People, and all the albums.
private struct SourceMenu: View {
    @Environment(AppModel.self) private var model
    @Environment(\.horizontalSizeClass) private var horizontalSize

    private static let sources: [(String, LocalizedStringKey, String)] = [
        ("all", "All Photos", "photo.on.rectangle"), ("recentlyAdded", "Recently Added", "clock"),
        ("picks", "Picks", "flag"), ("recentlyDeleted", "Recently Deleted", "trash"),
    ]

    var body: some View {
        let kind = model.library.state?.source.kind ?? "all"
        let deleted = (model.library.stats?.deleted ?? 0) > 0
        Picker("Show", selection: Binding(get: { kind }, set: { model.perform("library.source", ["kind": .string($0)]) })) {
            ForEach(Self.sources.filter { $0.0 != "recentlyDeleted" || deleted || kind == $0.0 }, id: \.0) { id, label, symbol in
                Label(label, systemImage: symbol).tag(id)
            }
        }
        .pickerStyle(.inline)
        Section {
            Button("People", systemImage: "person.2") { model.path.append(.people) }
            Button("Albums", systemImage: "rectangle.stack") {
                // an iPad has the albums in a column: show it
                if horizontalSize == .regular {
                    withAnimation { model.sidebar = .all }
                } else {
                    model.sheet = .albums
                }
            }
        }
    }
}

/// What narrows the grid, each filter as a chip with its × (a tap takes it off), then Clear All.
private struct FilterChips: View {
    @Environment(AppModel.self) private var model
    let chips: [JSONValue]

    var body: some View {
        if !chips.isEmpty {
            ScrollView(.horizontal) {
                HStack(spacing: 6) {
                    ForEach(Array(chips.enumerated()), id: \.offset) { _, chip in
                        Button {
                            model.perform("library.filter", chip["clear"])
                        } label: {
                            Label(chip["label"].string ?? "", systemImage: "xmark")
                                .labelStyle(TrailingIcon())
                                .lineLimit(1)
                        }
                        .accessibilityHint("Takes this filter off")
                    }
                    if chips.count > 1 {
                        Button("Clear All") { model.perform("library.clearFilter") }
                    }
                }
                .buttonStyle(.bordered)
                .buttonBorderShape(.capsule)
                .controlSize(.small)
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
            }
            .scrollIndicators(.hidden)
            .animation(.snappy, value: chips.count)
        }
    }

    /// The title, then the icon (a chip's ×).
    private struct TrailingIcon: LabelStyle {
        func makeBody(configuration: Configuration) -> some View {
            HStack(spacing: 4) {
                configuration.title
                configuration.icon.font(.caption2.weight(.bold))
            }
        }
    }
}

/// The grid's order (in the grid's "…").
struct SortItems: View {
    @Environment(AppModel.self) private var model

    private static let keys: [(String, LocalizedStringKey)] = [
        ("captureDate", "Capture Time"), ("importDate", "Date Added"), ("editDate", "Edit Time"),
        ("fileName", "File Name"), ("rating", "Rating"), ("fileSize", "File Size"),
    ]

    var body: some View {
        let sort = model.library.state?.sort
        Section {
            ForEach(Self.keys, id: \.0) { key, label in
                Toggle(isOn: Binding(get: { sort?.key == key }, set: { _ in model.perform("library.sort", ["key": .string(key)]) })) {
                    Text(label)
                }
            }
        }
        Section {
            Toggle("Ascending", isOn: Binding(get: { sort?.ascending == true }, set: { _ in model.perform("library.sort", ["ascending": true]) }))
            Toggle("Descending", isOn: Binding(get: { sort?.ascending == false }, set: { _ in model.perform("library.sort", ["ascending": false]) }))
        }
    }
}

/// The grid's "…" actions: albums, the grid's look, Map, Settings, Help and All Commands. The items only:
/// `DuoOverflow` puts them in the overflow menu.
struct GridMoreItems: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var prefs = model.prefs
        Group {
            Button("New Album…", systemImage: "rectangle.stack.badge.plus") {
                model.sheet = .newAlbum(parent: nil, folder: false, addSelected: false)
            }
            Section {
                Menu {
                    SortItems()
                } label: {
                    Label("Sort By", systemImage: "arrow.up.arrow.down")
                }
                Toggle(isOn: $prefs.squareGrid) { Label("Square Thumbnails", systemImage: "square.grid.3x3") }
                Toggle(isOn: $prefs.gridInfo) { Label("Grid Info", systemImage: "textformat") }
                Menu {
                    Picker("Ratings & Flags", selection: $prefs.badges) {
                        ForEach(Prefs.Badges.allCases) { Text($0.label).tag($0) }
                    }
                } label: {
                    Label("Ratings & Flags", systemImage: "star")
                }
            }
            Section {
                Button("Slideshow", systemImage: "play.rectangle") { model.cover = .slideshow }
                Button("Map", systemImage: "map") { model.path.append(.map) }
                Button("People", systemImage: "person.2") { model.path.append(.people) }
            }
            Section {
                Button("Auto-Stack by Capture Time…", systemImage: "square.stack.3d.down.right") { model.sheet = .autoStack }
                Button("Keyword Sets…", systemImage: "tag.square") { model.sheet = .keywordSets }
                Button("Keyword Painter…", systemImage: "paintbrush.pointed") { model.askPainter = true }
                Button("Color Label Names…", systemImage: "circle.lefthalf.filled") { model.sheet = .labelNames }
            }
            Section {
                Button("Settings", systemImage: "gear") { model.sheet = .settings(nil) }
                Button("Help", systemImage: "questionmark.circle") {
                    if let url = URL(string: "https://github.com/storytold/lightcraft") { UIApplication.shared.open(url) }
                }
                Button("About LightCraft", systemImage: "info.circle") { model.sheet = .about }
            }
            Section {
                Button("All Commands…", systemImage: "command") { model.path.append(.commands) }
            }
        }
    }
}
/// What the grid's search field looks for: words in the photos' metadata, or a description.
enum SearchScope: Hashable {
    case text, describe
}
/// What is dropped on the grid: a picture from another app (imported), or text — the grid's own
/// photos carried back (`PhotoDrag` offers its ids as text too, taken first here), which are left alone.
nonisolated enum GridDrop: Transferable {
    case text
    case file(StagedFile)

    static var transferRepresentation: some TransferRepresentation {
        ProxyRepresentation(importing: { (_: String) in GridDrop.text })
        ProxyRepresentation(importing: { (f: StagedFile) in GridDrop.file(f) })
    }
}
#endif
