#if os(iOS)
import LightCraftCore
import SwiftUI

/// Every photo of the grid's source: square tiles edge to edge under sticky date headers, a pinch
/// changing how many go across. A tap opens a photo (or chooses it, in select mode); a long press
/// starts choosing.
struct GridView: View {
    @Environment(AppModel.self) private var model
    let namespace: Namespace.ID
    /// Scroll to this grid index (Months, Years, a photo put away).
    @Binding var jump: Int?
    @State private var pinchFrom: Int?
    @State private var tracker = VisibleTracker()
    /// The scrubber shows while the grid scrolls and a moment after.
    @State private var scrolling = false
    @State private var scrubbing: Int?
    /// Where each cell on screen is (in the grid's own space), for choosing by drag.
    @State private var frames = CellFrames()
    /// A sideways drag choosing photos (select mode).
    @State private var choosing: Choosing?

    var body: some View {
        let library = model.library
        let columns = Array(repeating: GridItem(.flexible(), spacing: 2), count: max(1, model.prefs.gridColumns))
        ScrollViewReader { proxy in
            ScrollView {
                LazyVGrid(columns: columns, spacing: 2, pinnedViews: [.sectionHeaders]) {
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
                .coordinateSpace(name: "grid")
                .simultaneousGesture(chooseDrag, including: model.selectMode ? .all : .none)
                // room under the last row for the Years / Months / All capsule
                Color.clear.frame(height: 96)
            }
            .scrollIndicators(library.count > 200 ? .hidden : .visible)
            .onScrollPhaseChange { _, phase in
                if phase.isScrolling {
                    scrolling = true
                } else {
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
                jump = nil
            }
            .onAppear {
                if let target = jump {
                    proxy.scrollTo(target, anchor: .center)
                    jump = nil
                }
            }
        }
        .simultaneousGesture(pinch)
        .sensoryFeedback(.selection, trigger: model.prefs.gridColumns)
        .sensoryFeedback(.selection, trigger: choosing?.last)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width in
            // cells under 110 pt: a header a year, else a month
            let cell = width / CGFloat(max(model.prefs.gridColumns, 1))
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

    /// Pinch in for more photos across, out for fewer (1, 2, 3, 4, 5, 7, 10 or 14).
    private var pinch: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                let from = pinchFrom ?? model.prefs.gridColumns
                pinchFrom = from
                let target = Double(from) / max(value.magnification, 0.05)
                let steps = Prefs.columnSteps
                // the nearest step on a log scale
                let best = steps.min { abs(log(Double($0)) - log(target)) < abs(log(Double($1)) - log(target)) } ?? from
                if best != model.prefs.gridColumns {
                    withAnimation(.snappy(duration: 0.3)) { model.prefs.gridColumns = best }
                }
            }
            .onEnded { _ in pinchFrom = nil }
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
        .padding(.horizontal, 16)
        .frame(height: 40)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.bar)
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
            Color(uiColor: .secondarySystemBackground)
                .aspectRatio(1, contentMode: .fit)
        }
    }
}

/// A photo in the grid: its picture filling the square, its marks, and whether it's chosen.
struct PhotoCell: View {
    @Environment(AppModel.self) private var model
    let photo: PhotoSummary
    let namespace: Namespace.ID
    /// Pixels asked for on the long edge (the engine rounds it to its own sizes).
    private var edge: Int { model.prefs.gridColumns <= 2 ? 768 : 384 }

    var body: some View {
        let chosen = model.library.selected.contains(photo.id)
        Color(uiColor: .secondarySystemBackground)
            .aspectRatio(1, contentMode: .fit)
            .overlay {
                if let picture = model.images.thumb(photo.id) {
                    Image(decorative: picture.image, scale: 1)
                        .resizable()
                        .scaledToFill()
                        .transition(.opacity)
                }
            }
            .overlay(alignment: .bottom) { CellBadges(photo: photo) }
            .overlay {
                if photo.isReject { Color.black.opacity(0.45) }
            }
            .overlay {
                if model.selectMode { SelectionMark(chosen: chosen) }
            }
            .clipped()
            .contentShape(Rectangle())
            .matchedTransitionSource(id: photo.id, in: namespace)
            // on screen (render.retain keeps its render queued), and asked again when the library
            // changes (an edit): the engine answers only what changed
            .onAppear { model.thumbAppeared(photo.id) }
            .onDisappear { model.thumbDisappeared(photo.id) }
            .task(id: model.state.catalogRevision) { model.requestThumb(photo.id, edge: edge) }
            .onTapGesture { tapped() }
            .onLongPressGesture(minimumDuration: 0.5) { longPressed() }
            .accessibilityElement()
            .accessibilityLabel(photo.title ?? photo.fileName)
            .accessibilityAddTraits(.isButton)
            .accessibilityAddTraits(model.selectMode && chosen ? .isSelected : [])
    }

    private func tapped() {
        if model.selectMode {
            model.perform("library.select", ["ids": [.int(Int64(clamping: photo.id))], "mode": "toggle"])
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
        }
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
                    Text(String(repeating: "★", count: r)).font(.system(size: 9))
                }
                if photo.isPick {
                    Image(systemName: "flag.fill").font(.system(size: 9))
                } else if photo.isReject {
                    Image(systemName: "flag.slash.fill").font(.system(size: 9))
                }
                Spacer(minLength: 0)
                if photo.edited == true {
                    Image(systemName: "slider.horizontal.3").font(.system(size: 9))
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
#endif

#if os(iOS)
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
        .sensoryFeedback(.selection, trigger: grabbed)
    }
}

/// Where the grid's cells on screen are (a reference: updating it redraws nothing).
final class CellFrames {
    var rects: [Int: CGRect] = [:]

    /// The cell under `p`.
    func index(at p: CGPoint) -> Int? {
        rects.first { $0.value.contains(p) }?.key
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
#endif
