import LightCraftCore
import SwiftUI

/// Every photo of the grid's source: square tiles edge to edge under sticky date headers, a pinch
/// changing how many go across. A tap opens a photo (or chooses it, in select mode); a long press
/// starts choosing.
struct GridView: View {
    @Environment(AppModel.self) private var model
    /// The grid's width: wider screens get more columns, so the tiles keep their size.
    @State private var gridWidth: CGFloat = 0
    let namespace: Namespace.ID
    /// Scroll to this grid index (Months, Years, a photo put away).
    @Binding var jump: Int?
    @State private var pinchFrom: Int?
    /// The thumbnail size a pinch started from (a Mac).
    @State private var pinchSize: Double?
    @State private var tracker = VisibleTracker()
    /// The scrubber shows while the grid scrolls and a moment after.
    @State private var scrolling = false
    @State private var scrubbing: Int?
    /// Where each cell on screen is (in the grid's own space), for choosing by drag.
    @State private var frames = CellFrames()
    /// A sideways drag choosing photos (select mode).
    @State private var choosing: Choosing?
    /// Where the grid is scrolled to.
    @State private var position = ScrollPosition(edge: .top)
    /// This grid has appeared once (the first time it scrolls back to where the last one was left, `LibraryModel.anchor`).
    @State private var appeared = false
    /// The room a Mac's grid leaves at its left and right.
    static let sidePadding: CGFloat = 24

    var body: some View {
        let library = model.library
        #if os(macOS)
        // a Mac's tiles are about as wide as its thumbnail-size slider says: as many as fit
        let across = max(1, Int((gridWidth + 2) / (model.prefs.thumbSize + 2)))
        #else
        // a phone's columns for each phone's width (sideways, an iPad: two or three times as many)
        let across = max(1, model.prefs.gridColumns) * max(1, Int((gridWidth / 400).rounded()))
        #endif
        let columns = Array(repeating: GridItem(.flexible(), spacing: 2), count: across)
        ScrollViewReader { proxy in
            ScrollView {
                LazyVGrid(columns: columns, spacing: 2) {
                    if library.groups.isEmpty {
                        ForEach(0..<library.count, id: \.self) { i in
                            cell(i)
                        }
                    } else {
                        ForEach(library.groups) { group in
                            Section {
                                ForEach(group.indices(in: library.count), id: \.self) { i in
                                    cell(i)
                                }
                            } header: {
                                DateHeader(group: group)
                            }
                        }
                    }
                }
                .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { gridWidth = $0 }
                #if os(macOS)
                // (room at the sides, as Photos leaves; the width above is what is left for the tiles)
                .padding(.horizontal, Self.sidePadding)
                #endif
                .coordinateSpace(name: "grid")
                .simultaneousGesture(chooseDrag, including: model.selectMode ? .all : .none)
            }
            .scrollIndicators(library.count > 200 ? .hidden : .visible)
            .scrollPosition($position)
            // the photos, then their date headers, arriving after the first layout leave the grid
            // scrolled under the bar: back to the top while nobody has scrolled it
            .onChange(of: library.count) { toTop() }
            .onChange(of: library.groups.count) { toTop() }
            .onScrollPhaseChange { _, phase, context in
                if phase.isScrolling {
                    scrolling = true
                } else {
                    // where it came to rest, for a grid the system rebuilds: the cell in the middle of what is on screen
                    // (a grid just made says it is at rest before its cells exist: nothing to write then)
                    let g = context.geometry
                    if let middle = frames.index(nearest: g.contentOffset.y + g.containerSize.height / 2) { library.anchor = middle }
                    Task {
                        try? await Task.sleep(for: .seconds(1.4))
                        if scrubbing == nil { scrolling = false }
                    }
                }
            }
            .overlay(alignment: .trailing) {
                if library.count > 200 {
                    Scrubber(count: library.count, at: scrubbing ?? model.visibleRange?.lowerBound ?? 0, label: scrubbing.flatMap { library.group(at: $0).map { dateTitle(key: $0.key, label: $0.label) } }) { i in
                        scrubbing = i
                        if let i { proxy.scrollTo(i, anchor: .top) }
                    }
                    .opacity(scrolling || scrubbing != nil ? 1 : 0)
                    .animation(.easeOut(duration: 0.2), value: scrolling)
                }
            }
            .onChange(of: jump) { _, target in
                guard let target else { return }
                proxy.scrollTo(target, anchor: .center)
                library.anchor = target
                jump = nil
            }
            #if os(macOS)
            // the arrow keys move the chosen photo: it stays on screen
            .onChange(of: model.state.active) { _, active in
                guard let active, let i = library.index(of: active) else { return }
                proxy.scrollTo(i)
            }
            #endif
            .onAppear {
                defer { appeared = true }
                if let target = jump {
                    proxy.scrollTo(target, anchor: .center)
                    library.anchor = target
                    jump = nil
                } else if !appeared, let anchor = library.anchor, anchor > 0, anchor < library.count {
                    // a grid made again after a fold, an unfold or a rotation: back to where it was
                    proxy.scrollTo(anchor, anchor: .center)
                }
            }
        }
        .simultaneousGesture(pinch)
        // (Up and Down move by a row)
        .onChange(of: across, initial: true) { _, columns in model.gridAcross = columns }
        .haptic(.selection, trigger: model.prefs.gridColumns)
        .haptic(.selection, trigger: choosing?.last)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width in
            // cells under 110 pt: a header a year, else a month
            #if os(macOS)
            let cell = CGFloat(model.prefs.thumbSize)
            #else
            let cell = width / CGFloat(max(model.prefs.gridColumns, 1))
            #endif
            let by = cell < 110 ? "year" : "month"
            if by != model.library.autoGroup {
                model.library.autoGroup = by
                if let engine = model.engine { Task { await model.library.reloadGroups(engine) } }
            }
        }
        .overlay {
            if library.count == 0, model.engine != nil {
                empty
            }
        }
    }

    /// Another photo list clears the anchor, so a grid that has scrolled back to one is not sent to the top by its count.
    private func toTop() {
        if !position.isPositionedByUser, model.library.anchor == nil { position.scrollTo(edge: .top) }
    }

    private func cell(_ i: Int) -> some View {
        GridCell(index: i, namespace: namespace)
            .id(i)
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .named("grid")) } action: { frames.rects[i] = $0 }
            .onAppear {
                tracker.appeared(i)
                model.visibleRange = tracker.range
            }
            .onDisappear {
                tracker.disappeared(i)
                frames.rects[i] = nil
                model.visibleRange = tracker.range
            }
    }

    /// Choosing photos by touch, as iOS Photos does: a drag that sets off sideways from a photo
    /// chooses the run from it to the one under the finger (unchooses it, when the first was
    /// chosen: `library.chooseRun`); an up or down drag scrolls instead.
    private var chooseDrag: some Gesture {
        DragGesture(minimumDistance: 12, coordinateSpace: .named("grid"))
            .onChanged { g in
                if choosing == nil {
                    let d = g.translation
                    guard abs(d.width) >= 12, abs(d.width) >= 2 * abs(d.height), let start = frames.index(at: g.startLocation),
                          let first = model.library.photo(at: start, model.engine)?.id else { return }
                    choosing = Choosing(start: start, adding: !model.library.selected.contains(first), base: Array(model.library.selected))
                }
                guard var c = choosing, let cur = frames.index(at: g.location), c.last != cur else { return }
                c.last = cur
                choosing = c
                model.perform("library.chooseRun", ["base": AppModel.ids(c.base), "from": .int(Int64(c.start)), "to": .int(Int64(cur)), "adding": .bool(c.adding)])
            }
            .onEnded { _ in choosing = nil }
    }

    /// Pinch in for more photos across, out for fewer (1, 2, 3, 4, 5, 7, 10 or 14); on a Mac, out for
    /// bigger thumbnails.
    private var pinch: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                #if os(macOS)
                let size = pinchSize ?? model.prefs.thumbSize
                pinchSize = size
                model.prefs.thumbSize = min(max(size * value.magnification, Prefs.thumbSizes.lowerBound), Prefs.thumbSizes.upperBound)
                #else
                let from = pinchFrom ?? model.prefs.gridColumns
                pinchFrom = from
                let target = Double(from) / max(value.magnification, 0.05)
                let steps = Prefs.columnSteps
                // the nearest step on a log scale
                let best = steps.min { abs(log(Double($0)) - log(target)) < abs(log(Double($1)) - log(target)) } ?? from
                if best != model.prefs.gridColumns {
                    withAnimation(.snappy(duration: 0.3)) { model.prefs.gridColumns = best }
                }
                #endif
            }
            .onEnded { _ in
                pinchFrom = nil
                pinchSize = nil
            }
    }

    @ViewBuilder private var empty: some View {
        if model.library.state?.isFiltered == true {
            ContentUnavailableView {
                Label("No Photos Match", systemImage: "line.3.horizontal.decrease.circle")
            } description: {
                Text("No photos match the active filters.")
            } actions: {
                Button("Clear Filter") { model.perform("library.clearFilter") }
            }
        } else {
            ContentUnavailableView {
                Label("No Photos", systemImage: "photo.on.rectangle")
            } description: {
                Text("Tap + to add photos from Photos or Files.")
            }
        }
    }
}

/// Which grid indexes are on screen (a plain class: updating it doesn't redraw the grid).
final class VisibleTracker {
    private var shown: Set<Int> = []

    func appeared(_ i: Int) { shown.insert(i) }
    func disappeared(_ i: Int) { shown.remove(i) }

    var range: Range<Int>? {
        guard let lo = shown.min(), let hi = shown.max() else { return nil }
        return lo..<(hi + 1)
    }
}

/// A date header over its photos: the date and how many (a tap chooses them, in select mode).
struct DateHeader: View {
    @Environment(AppModel.self) private var model
    let group: DateGroup

    var body: some View {
        HStack(spacing: 6) {
            Text(dateTitle(key: group.key, label: group.label)).font(.subheadline.weight(.semibold))
            Text("· ^[\(group.count) photo](inflect: true)").font(.subheadline).foregroundStyle(.secondary)
            Spacer()
            if model.selectMode {
                Button("Select") {
                    Task { await chooseRun() }
                }
                .font(.subheadline)
            }
        }
        #if os(macOS)
        // (a Mac's grid is already inset: the text lines up with the first tile)
        .padding(.horizontal, 2)
        #else
        .padding(.horizontal, 16)
        #endif
        .frame(height: 40)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Choose the photos under this header.
    private func chooseRun() async {
        guard let engine = model.engine else { return }
        let page: JSONValue = ["offset": .int(Int64(group.start)), "limit": .int(Int64(group.count))]
        guard let result = try? await engine.execute("catalog.query", page, as: PhotoPage.self) else { return }
        await model.run("library.select", ["ids": AppModel.ids(result.photos.map(\.id)), "mode": "add"])
    }
}

/// One square of the grid: a placeholder until its page of photos has loaded, then the photo.
struct GridCell: View {
    @Environment(AppModel.self) private var model
    let index: Int
    let namespace: Namespace.ID

    var body: some View {
        if let photo = model.library.photo(at: index, model.engine) {
            PhotoCell(photo: photo, namespace: namespace)
        } else {
            // (an aspect-ratio grid has no shape to show yet)
            (model.prefs.aspectGrid ? Color.clear : Color.lcSecondaryBackground)
                .aspectRatio(1, contentMode: .fit)
        }
    }
}

/// A photo in the grid: its picture filling the square (or, in an aspect-ratio grid, as large as fits in it), its marks, and whether it's chosen.
struct PhotoCell: View {
    @Environment(AppModel.self) private var model
    let photo: PhotoSummary
    let namespace: Namespace.ID
    /// Pixels asked for on the long edge (the engine rounds it to its own sizes).
    private var edge: Int { model.prefs.gridColumns <= 2 ? 768 : 384 }

    /// The room round a photo in an aspect-ratio grid.
    private static let fitInset: CGFloat = 6
    /// How much a tile's corners are rounded: a little for the squares, more for photos in their own shape (a phone's squares are sharp).
    private var radius: CGFloat {
        #if os(macOS)
        model.prefs.aspectGrid ? 8 : 4
        #else
        model.prefs.aspectGrid ? 6 : 0
        #endif
    }

    var body: some View {
        if model.prefs.aspectGrid {
            // the cell stays square, so the rows line up; the photo sits in it in its own shape, as large
            // as fits (its longer side at most the cell's)
            Color.clear
                .aspectRatio(1, contentMode: .fit)
                .overlay {
                    tile
                        .padding(Self.fitInset)
                }
        } else {
            tile
        }
    }

    @ViewBuilder private var tile: some View {
        let chosen = model.library.selected.contains(photo.id)
        Color.lcSecondaryBackground
            .aspectRatio(model.prefs.aspectGrid ? photo.aspect : 1, contentMode: .fit)
            .overlay {
                if let picture = model.images.thumb(photo.id) {
                    Image(decorative: picture.image, scale: 1)
                        .resizable()
                        .scaledToFill()
                        .transition(.opacity)
                }
            }
            .overlay(alignment: .bottom) { CellBadges(photo: photo) }
            .overlay(alignment: .topLeading) {
                // Grid Info: the file name over the photo
                if model.prefs.gridInfo {
                    Text(photo.fileName)
                        .font(.caption2)
                        .lineLimit(1)
                        .padding(.horizontal, 4).padding(.vertical, 1)
                        .background(.black.opacity(0.45), in: .rect(cornerRadius: 3))
                        .foregroundStyle(.white)
                        .padding(3)
                        .accessibilityHidden(true)
                }
            }
            .overlay {
                if photo.isReject { Color.black.opacity(0.45) }
            }
            .overlay {
                if model.selectMode { SelectionMark(chosen: chosen) }
            }
            .overlay(alignment: .topTrailing) {
                // painting: the photos that have the keyword
                if let k = model.painting, photo.keywords?.contains(where: { $0.caseInsensitiveCompare(k) == .orderedSame }) == true {
                    Image(systemName: "tag.fill")
                        .font(.caption)
                        .foregroundStyle(.white)
                        .padding(5)
                        .background(Color.accentColor, in: .circle)
                        .padding(4)
                        .accessibilityHidden(true)
                }
            }
            .clipShape(.rect(cornerRadius: radius))
            .contentShape(.rect(cornerRadius: radius))
            .matchedTransitionSource(id: photo.id, in: namespace)
            // on screen (render.retain keeps its render queued), and asked again when the library
            // changes (an edit): the engine answers only what changed
            .onAppear { model.thumbAppeared(photo.id) }
            .onDisappear { model.thumbDisappeared(photo.id) }
            .task(id: model.settledRevision) { model.requestThumb(photo.id, edge: edge) }
            #if os(macOS)
            // a click chooses, a double click opens, a right click has the menu (Mac/MacGridCell.swift)
            .modifier(MacCellInteraction(photo: photo, radius: radius))
            #else
            .onTapGesture { tapped() }
            .onLongPressGesture(minimumDuration: 0.5) { longPressed() }
            // choosing photos: the chosen ones are lifted and carried onto an album (added to it) or
            // into another app (a JPEG); not otherwise, where a long press starts choosing
            .modifier(DragChosen(on: model.selectMode && model.library.selected.contains(photo.id), photo: photo.id))
            #endif
            .accessibilityElement()
            .accessibilityLabel(photo.title ?? photo.fileName)
            .accessibilityValue(Self.spoken(photo))
            // an iPad's pointer lifts the photo under it
            .lcHoverLift()
            .accessibilityAddTraits(.isButton)
            .accessibilityAddTraits(model.selectMode && chosen ? .isSelected : [])
    }

    #if os(iOS)
    private func tapped() {
        if let k = model.painting {
            model.perform("keyword.toggle", ["ids": [.int(Int64(clamping: photo.id))], "keyword": .string(k)])
            model.buzz(.selection)
        } else if model.selectMode {
            model.perform("library.select", ["ids": [.int(Int64(clamping: photo.id))], "mode": "toggle"])
            model.buzz(.selection)
        } else {
            model.open(photo.id)
        }
    }

    private func longPressed() {
        if !model.selectMode {
            model.selectMode = true
            model.perform("library.select", ["ids": [.int(Int64(clamping: photo.id))]])
        } else {
            model.perform("library.select", ["ids": [.int(Int64(clamping: photo.id))], "mode": "toggle"])
            model.buzz(.selection)
        }
    }

    #endif

    /// What VoiceOver says after the photo's name: its stars, pick or reject, whether it is edited.
    static func spoken(_ photo: PhotoSummary) -> String {
        var parts: [String] = []
        if let r = photo.rating, r > 0 { parts.append(String(localized: "\(r) stars")) }
        if photo.isPick { parts.append(String(localized: "Picked")) }
        if photo.isReject { parts.append(String(localized: "Rejected")) }
        if photo.edited == true { parts.append(String(localized: "Edited")) }
        return parts.joined(separator: ", ")
    }
}

/// The check in a chosen cell's corner (and a ring round it).
struct SelectionMark: View {
    let chosen: Bool

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            if chosen {
                Rectangle().strokeBorder(Color.accentColor, lineWidth: 3)
            }
            Image(systemName: chosen ? "checkmark.circle.fill" : "circle")
                .font(.title3)
                .symbolRenderingMode(.palette)
                .foregroundStyle(.white, chosen ? Color.accentColor : .black.opacity(0.25))
                .shadow(radius: 2)
                .padding(5)
        }
    }
}

/// A cell's rating, flag, edited mark and colour label (per Settings ▸ Grid).
struct CellBadges: View {
    @Environment(AppModel.self) private var model
    let photo: PhotoSummary
    @ScaledMetric(relativeTo: .caption2) private var size: CGFloat = 9

    var body: some View {
        let rated = (photo.rating ?? 0) > 0 || photo.flag != "none" && photo.flag != nil || photo.label != nil
        let show = switch model.prefs.badges {
        case .always: true
        case .never: false
        case .auto: rated
        }
        if show, model.prefs.gridColumns <= 5 {
            HStack(spacing: 3) {
                if let r = photo.rating, r > 0 {
                    Text(String(repeating: "★", count: r)).font(.system(size: min(size, 14)))
                }
                if photo.isPick {
                    Image(systemName: "flag.fill").font(.system(size: min(size, 14)))
                } else if photo.isReject {
                    Image(systemName: "flag.slash.fill").font(.system(size: min(size, 14)))
                }
                Spacer(minLength: 0)
                if photo.edited == true {
                    Image(systemName: "slider.horizontal.3").font(.system(size: min(size, 14)))
                }
                if let label = photo.label {
                    Circle().fill(LabelColor.color(label)).frame(width: 8, height: 8)
                }
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 5)
            .padding(.vertical, 3)
            .background(LinearGradient(colors: [.clear, .black.opacity(0.55)], startPoint: .top, endPoint: .bottom))
        }
    }
}

/// The colour labels' colours.
enum LabelColor {
    static let all = ["red", "yellow", "green", "blue", "purple"]

    static func color(_ label: String) -> Color {
        switch label {
        case "red": .red
        case "yellow": .yellow
        case "green": .green
        case "blue": .blue
        case "purple": .purple
        default: .gray
        }
    }
}

/// The grid's scrubber: a handle down the right edge to drag through a long library, the month of
/// the photos it is over in a bubble beside it.
struct Scrubber: View {
    let count: Int
    let at: Int
    let label: String?
    let moved: (Int?) -> Void
    @State private var grabbed = false

    var body: some View {
        GeometryReader { geo in
            let usable = max(geo.size.height - 48, 1)
            let y = CGFloat(at) / CGFloat(max(count - 1, 1)) * usable
            ZStack(alignment: .topTrailing) {
                Color.clear
                HStack(spacing: 8) {
                    if grabbed, let label {
                        Text(label)
                            .font(.subheadline.weight(.semibold))
                            .padding(.horizontal, 12).padding(.vertical, 6)
                            .glassEffect(.regular, in: .capsule)
                    }
                    Capsule()
                        .fill(.secondary)
                        .frame(width: grabbed ? 9 : 6, height: 48)
                        .frame(width: 44)
                        .contentShape(Rectangle())
                        .gesture(
                            DragGesture(minimumDistance: 0, coordinateSpace: .named("scrubber"))
                                .onChanged { g in
                                    grabbed = true
                                    // (before layout the track has no height: nothing to scrub)
                                    guard usable > 0, count > 0 else { return }
                                    let f = min(max((g.location.y - 24) / usable, 0), 1)
                                    moved(Int(f * CGFloat(count - 1)))
                                }
                                .onEnded { _ in
                                    grabbed = false
                                    moved(nil)
                                }
                        )
                        .accessibilityLabel("Scroll through photos")
                }
                .offset(y: y)
            }
            .coordinateSpace(name: "scrubber")
        }
        .padding(.vertical, 8)
        .haptic(.selection, trigger: grabbed)
    }
}

/// Where the grid's cells on screen are (a reference: updating it redraws nothing).
final class CellFrames {
    var rects: [Int: CGRect] = [:]

    /// The cell under `p`.
    func index(at p: CGPoint) -> Int? {
        rects.first { $0.value.contains(p) }?.key
    }

    /// The cell whose middle is nearest to `y` (the first of its row).
    func index(nearest y: CGFloat) -> Int? {
        rects.min { (abs($0.value.midY - y), $0.value.minX) < (abs($1.value.midY - y), $1.value.minX) }?.key
    }
}

/// A drag choosing photos: where it began, whether it chooses or unchooses, what was chosen
/// before it, and the photo under the finger last.
struct Choosing {
    let start: Int
    let adding: Bool
    let base: [UInt64]
    var last: Int?
}
/// The chosen photos carried from a chosen one (with how many on the picture).
struct DragChosen: ViewModifier {
    @Environment(AppModel.self) private var model
    let on: Bool
    let photo: UInt64

    func body(content: Content) -> some View {
        if on {
            // (a Mac drags any photo: the chosen ones if it is one of them, else itself)
            let ids = model.library.selected.contains(photo) ? Array(model.library.selected) : [photo]
            content.draggable(PhotoDrag(ids: ids, engine: model.engine)) {
                if let picture = model.images.thumb(photo) {
                    Image(decorative: picture.image, scale: 1).resizable().scaledToFill()
                        .frame(width: 120, height: 120).clipShape(.rect(cornerRadius: 8))
                        .overlay(alignment: .topTrailing) {
                            if ids.count > 1 {
                                Text(verbatim: "\(ids.count)").font(.caption.bold()).padding(6).background(.tint, in: .circle).foregroundStyle(.white).padding(4)
                            }
                        }
                }
            }
        } else {
            content
        }
    }
}

/// Photos carried from the grid: onto an album (`lightcraft-photos:` ids, added to it) or into
/// another app, which gets a JPEG of the first as edited (`app.export`, not remembered for Export
/// with Previous).
nonisolated struct PhotoDrag: Transferable {
    let ids: [UInt64]
    let engine: Engine?

    static let prefix = "lightcraft-photos:"

    static var transferRepresentation: some TransferRepresentation {
        // (the picture first: another app takes the richest form it can; this app's drops ask for the text)
        // ponytail: another app gets the first photo only; a file per photo needs a multi-item drag
        FileRepresentation(exportedContentType: .jpeg) { drag in SentTransferredFile(try await drag.exported()) }
        ProxyRepresentation { (drag: PhotoDrag) in prefix + drag.ids.map(String.init).joined(separator: ",") }
    }

    /// The photo ids a dropped text names (`lightcraft-photos:1,2`), if any.
    static func ids(_ items: [String]) -> [UInt64] {
        items.flatMap { item -> [UInt64] in
            guard item.hasPrefix(prefix) else { return [] }
            return item.dropFirst(prefix.count).split(separator: ",").compactMap { UInt64($0) }
        }
    }

    private func exported() async throws -> URL {
        guard let engine, let id = ids.first else { throw CocoaError(.fileNoSuchFile) }
        let dir = URL(fileURLWithPath: NSTemporaryDirectory()).appending(path: "Drag").appending(path: UUID().uuidString.prefix(8).description)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let r = try await engine.execute("app.export", ["dir": .string(dir.path), "ids": [.int(Int64(clamping: id))], "format": "jpeg",
                                                         "longEdge": 4096, "conflict": "unique", "remember": false])
        guard let path = r["files"][0]["path"].string else { throw CocoaError(.fileWriteUnknown) }
        return URL(fileURLWithPath: path)
    }
}
