#if os(iOS)
import LightCraftCore
import PhotosUI
import SwiftUI

/// The library: the collection's name as the title (a tap opens the albums), Select and "…" above,
/// the grid (All, Months or Years) and, under it, the span capsule and the + button. Choosing
/// photos turns the bars into Cancel / Select All and an action bar.
struct LibraryScreen: View {
    @Environment(AppModel.self) private var model
    let namespace: Namespace.ID
    @State private var jump: Int?
    @State private var importer = ImportModel()
    @State private var scope = SearchScope.text
    /// This device or the sync server can search by description (`vision.model.status`).
    @State private var describes = false
    @State private var painterText = ""
    @State private var dropping = false

    var body: some View {
        @Bindable var prefs = model.prefs
        Group {
            switch model.prefs.gridSpan {
            case .all: GridView(namespace: namespace, jump: $jump)
            case .months, .years: DateBlocksView(span: model.prefs.gridSpan) { index in
                withAnimation(.snappy) { model.prefs.gridSpan = .all }
                jump = index
            }
            }
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            if !model.selectMode { LibraryHeader() }
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
        .overlay(alignment: .bottom) {
            if model.selectMode {
                SelectionBar()
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            } else if let k = model.painting {
                PainterBar(keyword: k)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            } else if model.searching {
                SearchBar(scope: $scope, describes: describes)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            } else if model.library.count > 0 || model.plainAlbum != nil {
                bottomControls
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            } else {
                HStack { Spacer(); addButton }.padding(20)
            }
        }
        .animation(.snappy, value: model.selectMode)
        .animation(.snappy, value: model.painting)
        .animation(.snappy, value: model.searching)
        .alert("Keyword Painter", isPresented: Bindable(model).askPainter) {
            TextField("Keyword", text: $painterText)
            Button("Paint") {
                let k = painterText.trimmingCharacters(in: .whitespaces)
                if !k.isEmpty { model.painting = k }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Tap photos to give them the keyword; tap again to take it away.")
        }
        .navigationTitle(model.selectMode ? selectTitle : "")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { toolbar }
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
        .task(id: model.state.catalogRevision) {
            guard let engine = model.engine else { return }
            describes = (try? await engine.execute("vision.model.status"))?["available"].bool ?? false
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

    private var selectTitle: String {
        if let album = model.addingTo, let node = model.albums.node(album) {
            return String(localized: "Add to \(node.name)")
        }
        let n = model.library.selected.count
        return n == 0 ? String(localized: "Select Photos") : String(localized: "\(n) Selected")
    }

    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        if model.selectMode {
            ToolbarItem(placement: .topBarLeading) {
                Button("Cancel") { endSelecting() }
            }
            ToolbarItem(placement: .topBarTrailing) {
                if model.library.selected.count < model.library.count {
                    Button("Select All") { model.perform("library.selectAll") }
                } else {
                    Button("Deselect All") { model.perform("library.select", ["ids": []]) }
                }
            }
        } else {
            if model.albums.sync.map({ $0.state != "off" }) == true {
                ToolbarItem(placement: .topBarTrailing) {
                    CloudButton()
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                if model.library.count > 0 {
                    Button("Select") {
                        model.selectMode = true
                        model.perform("library.select", ["ids": []])
                    }
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                GridMoreMenu()
            }
        }
    }

    private func endSelecting() {
        model.selectMode = false
        if let album = model.addingTo {
            model.addingTo = nil
            model.perform("library.source", ["kind": "album", "id": .int(Int64(clamping: album))])
        }
    }

    /// Years / Months / All, and + (add photos), floating over the grid's bottom.
    private var bottomControls: some View {
        @Bindable var prefs = model.prefs
        return HStack(spacing: 12) {
            Button("Search", systemImage: "magnifyingglass") { model.searching = true }
                .labelStyle(.iconOnly)
                .font(.title3.weight(.semibold))
                .frame(width: 48, height: 48)
                .glassEffect(.regular.interactive(), in: .circle)
            Spacer(minLength: 0)
            Picker("Show", selection: $prefs.gridSpan) {
                ForEach(Prefs.GridSpan.allCases) { span in
                    Text(span.label).tag(span)
                }
            }
            .pickerStyle(.segmented)
            .frame(width: 220)
            .padding(4)
            .glassEffect(.regular.interactive(), in: .capsule)
            .haptic(.selection, trigger: model.prefs.gridSpan)
            Spacer(minLength: 0)
            addButton
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 8)
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
                Button("Lightroom Classic Catalog…", systemImage: "square.stack.3d.up") { model.picker = .lightroom }
            }
        } label: {
            Image(systemName: "plus")
                .font(.title2.weight(.semibold))
                .frame(width: 56, height: 56)
        }
        .buttonStyle(.glassProminent)
        .buttonBorderShape(.circle)
        .accessibilityLabel("Add Photos")
    }

    /// The album's + ▸ LightCraft: All Photos to choose from, Add in the action bar.
    private func startAdding(_ album: UInt64) {
        model.addingTo = album
        model.selectMode = true
        model.perform("library.source", ["kind": "all"])
        model.perform("library.select", ["ids": []])
    }
}

/// The collection's name (a tap opens the albums), the count, and Sort and Filter.
struct LibraryHeader: View {
    @Environment(AppModel.self) private var model
    @Environment(\.horizontalSizeClass) private var horizontalSize
    /// What narrows the grid (`library.filterChips`), and the counts.
    @State private var chips: JSONValue = .null

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            let list = chips["chips"].array ?? []
            if !list.isEmpty {
                // each filter as a chip with its ×, then Clear All
                ScrollView(.horizontal) {
                    HStack(spacing: 6) {
                        ForEach(Array(list.enumerated()), id: \.offset) { _, chip in
                            Button {
                                model.perform("library.filter", chip["clear"])
                            } label: {
                                HStack(spacing: 4) {
                                    Text(chip["label"].string ?? "").lineLimit(1)
                                    Image(systemName: "xmark").font(.caption2.weight(.bold))
                                }
                                .font(.footnote)
                                .padding(.horizontal, 10).padding(.vertical, 5)
                                .background(Color.accentColor.opacity(0.15), in: .capsule)
                            }
                            .buttonStyle(.plain)
                            .accessibilityHint("Takes this filter off")
                        }
                        if list.count > 1 {
                            Button("Clear All") { model.perform("library.clearFilter") }
                                .font(.footnote)
                        }
                    }
                    .padding(.horizontal, 16)
                }
                .scrollIndicators(.hidden)
            }
        }
        .padding(.vertical, 6)
        .background(.bar)
        .task(id: Key(gen: model.state.visibleGen, count: model.state.visibleCount)) {
            guard let engine = model.engine else { return }
            chips = (try? await engine.execute("library.filterChips")) ?? .null
        }
        .animation(.snappy, value: chips["chips"].array?.count ?? 0)
    }

    private nonisolated struct Key: Hashable {
        let gen: UInt64
        let count: Int
    }

    /// The collection's name: a built-in one in the reader's language, an album or folder as named.
    private var title: String {
        guard let state = model.library.state else { return String(localized: "All Photos") }
        if ["album", "folder", "libraryFolder"].contains(state.source.kind) { return state.sourceLabel }
        return Bundle.main.localizedString(forKey: state.sourceLabel, value: state.sourceLabel, table: nil)
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 8) {
            Button {
                // an iPad has the albums in a column: the title shows or hides it
                if horizontalSize == .regular {
                    withAnimation { model.sidebar = model.sidebar == .detailOnly ? .all : .detailOnly }
                } else {
                    model.sheet = .albums
                }
            } label: {
                VStack(alignment: .leading, spacing: 0) {
                    HStack(spacing: 6) {
                        Text(title)
                            .font(.title.weight(.bold))
                            .lineLimit(1)
                        Image(systemName: "chevron.down")
                            .font(.headline.weight(.semibold))
                            .foregroundStyle(.secondary)
                    }
                    Text(countText)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .buttonStyle(.plain)
            .accessibilityHint("Shows your albums")
            Spacer()
            HStack(spacing: 0) {
                SortMenu()
                Button {
                    model.sheet = .filter
                } label: {
                    Image(systemName: model.library.state?.isFiltered == true ? "line.3.horizontal.decrease.circle.fill" : "line.3.horizontal.decrease")
                        .frame(width: 40, height: 40)
                }
                .accessibilityLabel("Filter")
            }
            .glassEffect(.regular.interactive(), in: .capsule)
        }
        .padding(.horizontal, 16)
    }

    /// "14 of 120 photos" when a filter narrows the source, else "14 photos".
    private var countText: String {
        let n = model.library.count
        if let total = chips["total"].int, !(chips["chips"].array ?? []).isEmpty, Int(total) != n {
            return String(localized: "\(n) of \(Int(total)) photos")
        }
        return String(localized: "\(n) photos")
    }
}

/// The grid's order.
struct SortMenu: View {
    @Environment(AppModel.self) private var model

    private static let keys: [(String, LocalizedStringKey)] = [
        ("captureDate", "Capture Time"), ("importDate", "Date Added"), ("editDate", "Edit Time"),
        ("fileName", "File Name"), ("rating", "Rating"), ("fileSize", "File Size"),
    ]

    var body: some View {
        let sort = model.library.state?.sort
        Menu {
            Section("Sort By") {
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
        } label: {
            Image(systemName: "arrow.up.arrow.down")
                .frame(width: 40, height: 40)
        }
        .accessibilityLabel("Sort")
    }
}

/// The grid's "…": albums, the grid's look, Map, Settings, Help and All Commands.
struct GridMoreMenu: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var prefs = model.prefs
        Menu {
            Button("New Album…", systemImage: "rectangle.stack.badge.plus") {
                model.sheet = .newAlbum(parent: nil, folder: false, addSelected: false)
            }
            Section {
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
        } label: {
            Label("More", systemImage: "ellipsis")
        }
    }
}
/// What the grid's search field looks for: words in the photos' metadata, or a description.
enum SearchScope: Hashable {
    case text, describe
}
/// Under the grid while painting: the keyword, and Done.
private struct PainterBar: View {
    @Environment(AppModel.self) private var model
    let keyword: String

    var body: some View {
        HStack(spacing: 12) {
            Label {
                Text("Painting “\(keyword)”")
            } icon: {
                Image(systemName: "paintbrush.pointed")
            }
            .lineLimit(1)
            Spacer(minLength: 0)
            Button("Done") { model.painting = nil }
                .buttonStyle(.glassProminent)
                .keyboardShortcut(.cancelAction)
        }
        .padding(.leading, 18).padding(.trailing, 8).padding(.vertical, 8)
        .glassEffect(.regular, in: .capsule)
        .padding(.horizontal, 20)
        .padding(.bottom, 8)
    }
}
/// The grid's search field, at its foot while searching: words narrow the grid as they are typed;
/// with Describe (when this device or the sync server can), a description is searched on Return.
private struct SearchBar: View {
    @Environment(AppModel.self) private var model
    @Binding var scope: SearchScope
    let describes: Bool
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField(LocalizedStringKey(scope == .describe ? "Describe a photo…" : "Search Photos"), text: Bindable(model).searchText)
                    .focused($focused)
                    .submitLabel(.search)
                    .onSubmit(submit)
                if !model.searchText.isEmpty {
                    Button("Clear", systemImage: "xmark.circle.fill") { model.searchText = "" }
                        .labelStyle(.iconOnly)
                        .foregroundStyle(.secondary)
                }
                if describes {
                    Button("Describe") { scope = scope == .describe ? .text : .describe }
                        .font(.caption.weight(.semibold))
                        .buttonStyle(.bordered)
                        .buttonBorderShape(.capsule)
                        .tint(scope == .describe ? .accentColor : .secondary)
                        .controlSize(.small)
                        .accessibilityAddTraits(scope == .describe ? .isSelected : [])
                        .accessibilityHint("Search by describing the photo, in your own words")
                }
            }
            .padding(.horizontal, 14)
            .frame(height: 48)
            .glassEffect(.regular.interactive(), in: .capsule)
            Button("Close", systemImage: "xmark") { model.searching = false }
                .labelStyle(.iconOnly)
                .font(.title3.weight(.semibold))
                .frame(width: 48, height: 48)
                .glassEffect(.regular.interactive(), in: .circle)
                .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 8)
        .onAppear { focused = true }
    }

    private func submit() {
        let q = model.searchText.trimmingCharacters(in: .whitespaces)
        if scope == .describe, !q.isEmpty { model.perform("library.search", ["q": .string(q)]) }
    }
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
