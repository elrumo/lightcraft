#if os(macOS)
import LightCraftCore
import SwiftUI

/// A zoom of Months or Years: in (a pinch out) or out, about a point of their view (nil: where the pointer is).
struct BlockZoom: Equatable {
    var zoomIn: Bool
    var at: CGPoint?
}

/// Years or Months on a Mac, laid out as Photos has them: a column of at most 1100 pt, centred, with the
/// photos as rounded, softly shadowed tiles. Each year is a square card with the year over it, three across;
/// each month is a heading over a mosaic of its photos (`Mosaic`), the best one large and the day each was
/// taken on it. A click on a year shows its months, a click on a month's photo shows All at that photo.
/// A pinch does the same about the pointer: out on a year or a month goes in (Years to Months to All),
/// in on a month goes out to its year.
struct MacDateBlocksView: View {
    @Environment(AppModel.self) private var model
    let span: Prefs.GridSpan
    /// Show All at this grid index.
    let openAt: (Int) -> Void
    /// What is on screen: the blocks of one span, kept until the next span's are here, so a change of span
    /// is one cross-fade and never one span's layout over the other's blocks.
    @State private var shown: Shown
    @State private var width: CGFloat = 0
    /// Scroll to this block (set when the span changed and its blocks are on screen).
    @State private var scrollTo: String?
    /// The year (or month) a change of span is to bring into view: the one the pointer was over.
    @State private var aim: String?
    @State private var frames = BlockFrames()
    /// This pinch has zoomed (one pinch is one step).
    @State private var zoomed = false

    private static let gap: CGFloat = 20

    init(span: Prefs.GridSpan, openAt: @escaping (Int) -> Void) {
        self.span = span
        self.openAt = openAt
        _shown = State(initialValue: Shown(span: span, blocks: []))
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                content
                    .id(shown.span)
                    .transition(.opacity)
                    .frame(maxWidth: 1100)
                    .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
                    .padding(.horizontal, 32)
                    .padding(.vertical, 28)
                    .frame(maxWidth: .infinity)
            }
            .onChange(of: scrollTo) { _, key in
                guard let key else { return }
                scrollTo = nil
                Task {
                    try? await Task.sleep(for: .milliseconds(120))
                    proxy.scrollTo(key, anchor: .top)
                }
            }
        }
        // (the pointer and the blocks are both measured in this view's space, not the scrolled content's:
        // scrolling under a still pointer moves the blocks, not the pointer)
        .coordinateSpace(name: "blocks")
        .onContinuousHover(coordinateSpace: .named("blocks")) { phase in
            switch phase {
            case .active(let point): frames.pointer = point
            case .ended: frames.pointer = nil
            }
        }
        .simultaneousGesture(pinch)
        .onChange(of: model.blockZoom) { _, request in
            guard let request else { return }
            model.blockZoom = nil
            zoom(in: request.zoomIn, at: request.at ?? frames.pointer ?? CGPoint(x: width / 2, y: 200))
        }
        .overlay {
            // (an empty library: just emptied by a sign-out, or not filled yet)
            if shown.blocks.isEmpty && model.state.visibleCount == 0 {
                ContentUnavailableView {
                    Label("No Photos", systemImage: "photo.on.rectangle")
                } description: {
                    Text("Tap + to add photos from Photos or Files.")
                }
            }
        }
        .task(id: Key(span: span, gen: model.state.visibleGen, rev: model.settledRevision)) {
            guard let engine = model.engine else { return }
            let name = span == .years ? "years" : "months"
            let blocks = (try? await engine.execute("library.blocks", ["span": .string(name)], as: [DateBlockInfo].self)) ?? []
            guard !Task.isCancelled else { return }
            let changed = shown.span != span
            withAnimation(.snappy) { shown = Shown(span: span, blocks: blocks) }
            // a new span starts where the pointer was (or at the top), not at the old span's offset
            if changed {
                let year = aim
                aim = nil
                scrollTo = blocks.first { year == nil || $0.key == year || (year?.isEmpty == false && $0.key.hasPrefix(year ?? "")) }?.key
            }
        }
    }

    @ViewBuilder private var content: some View {
        switch shown.span {
        case .years:
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: Self.gap), count: width < 560 ? 2 : 3), spacing: Self.gap) {
                ForEach(shown.blocks) { block in
                    YearCard(block: block, frames: frames) { zoomIntoMonths(of: block.key) }
                        .id(block.key)
                }
            }
        default:
            LazyVStack(alignment: .leading, spacing: 44) {
                ForEach(Array(shown.blocks.enumerated()), id: \.element.id) { n, block in
                    MonthBlock(block: block, mirrored: n % 2 == 1, frames: frames, openAt: openAt)
                        .id(block.key)
                }
            }
        }
    }

    /// Out on a year or a month goes in, in on a month goes out; one step for each pinch.
    private var pinch: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                guard !zoomed, value.magnification > 1.2 || value.magnification < 0.82 else { return }
                zoomed = true
                zoom(in: value.magnification > 1, at: frames.pointer ?? CGPoint(x: width / 2, y: 200))
            }
            .onEnded { _ in zoomed = false }
    }

    /// One step in or out about `point`: the year or month under it (the nearest one, over a gap).
    private func zoom(in zoomIn: Bool, at point: CGPoint?) {
        guard let point else { return }
        switch shown.span {
        case .years:
            if zoomIn, let key = frames.block(near: point) { zoomIntoMonths(of: key) }
        case .months:
            if zoomIn {
                let first = frames.block(near: point).flatMap { key in shown.blocks.first { $0.key == key }?.photos.map(\.index).min() }
                if let index = frames.tile(at: point) ?? first { openAt(index) }
            } else if let key = frames.block(near: point) {
                aim = String(key.prefix(4))
                model.prefs.gridSpan = .years
            }
        case .all:
            break
        }
    }

    private func zoomIntoMonths(of year: String) {
        aim = year
        model.prefs.gridSpan = .months
    }

    private struct Shown {
        let span: Prefs.GridSpan
        let blocks: [DateBlockInfo]
    }

    private nonisolated struct Key: Hashable {
        let span: Prefs.GridSpan
        let gen: UInt64
        let rev: UInt64
    }
}

/// A year: its best photo as a square card, the year over it.
private struct YearCard: View {
    let block: DateBlockInfo
    let frames: BlockFrames
    let open: () -> Void

    var body: some View {
        MacBlockTile(photo: block.photos.first?.id ?? 0, edge: 768, radius: 14)
            .aspectRatio(1, contentMode: .fit)
            .overlay(alignment: .topLeading) {
                Text(block.key.isEmpty ? String(localized: "No Date") : dateTitle(key: block.key, label: block.label))
                    .font(.system(size: 30, weight: .bold))
                    .foregroundStyle(.white)
                    .shadow(color: .black.opacity(0.4), radius: 5, y: 1)
                    .padding(22)
                    .allowsHitTesting(false)
            }
            .onTapGesture(perform: open)
            .tracked { frames.blocks[block.key] = $0 }
    }
}

/// A month: its name, then its photos (up to six) in a mosaic, the best one large. The mosaics of
/// neighbouring months are mirrored, so the large photo changes sides.
private struct MonthBlock: View {
    let block: DateBlockInfo
    let mirrored: Bool
    let frames: BlockFrames
    let openAt: (Int) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Button {
                openAt(block.photos.map(\.index).min() ?? 0)
            } label: {
                Text(dateTitle(key: block.key, label: block.label, abbreviated: true))
                    .font(.system(size: 24, weight: .bold))
            }
            .buttonStyle(.plain)
            Mosaic(specs: Mosaic.specs(for: block.photos.count, mirrored: mirrored), gap: 18) {
                ForEach(Array(block.photos.prefix(6).enumerated()), id: \.element.id) { n, shown in
                    MacBlockTile(photo: shown.id, edge: n == 0 ? 1024 : 768, radius: 12)
                        .overlay(alignment: .topLeading) {
                            if let day = shown.day {
                                Text(day)
                                    .font(.system(size: 20, weight: .bold))
                                    .foregroundStyle(.white)
                                    .shadow(color: .black.opacity(0.4), radius: 3, y: 1)
                                    .padding(12)
                                    .allowsHitTesting(false)
                            }
                        }
                        .onTapGesture { openAt(shown.index) }
                        .tracked { frames.tiles[shown.index] = $0 }
                }
            }
        }
        .tracked { frames.blocks[block.key] = $0 }
    }
}

/// A rounded photo with Photos' soft shadow.
private struct MacBlockTile: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    /// Pixels asked for on the long edge.
    let edge: Int
    let radius: CGFloat

    var body: some View {
        Color.lcSecondaryBackground
            .overlay {
                if let picture = model.images.thumb(photo) {
                    Image(decorative: picture.image, scale: 1).resizable().scaledToFill()
                }
            }
            .clipShape(.rect(cornerRadius: radius))
            .contentShape(.rect(cornerRadius: radius))
            .shadow(color: .black.opacity(0.2), radius: 9, y: 3)
            .task(id: model.settledRevision) { model.requestThumb(photo, edge: edge) }
            .onAppear { model.thumbAppeared(photo) }
            .onDisappear { model.thumbDisappeared(photo) }
    }
}

/// A month's photos on a grid of twelve columns of square units (so halves, thirds and quarters all meet
/// the same gaps): each subview has a place in `specs`, in units, the first being the large one.
private struct Mosaic: Layout {
    struct Spec {
        let c, r, w, h: Int
    }

    let specs: [Spec]
    let gap: CGFloat

    private func unit(_ width: CGFloat) -> CGFloat { max((width - 11 * gap) / 12, 1) }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? 600
        let rows = specs.prefix(subviews.count).map { $0.r + $0.h }.max() ?? 0
        return CGSize(width: width, height: max(CGFloat(rows) * (unit(width) + gap) - gap, 0))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let u = unit(bounds.width)
        for (n, subview) in subviews.enumerated() {
            guard specs.indices.contains(n) else { continue }
            let s = specs[n]
            subview.place(
                at: CGPoint(x: bounds.minX + CGFloat(s.c) * (u + gap), y: bounds.minY + CGFloat(s.r) * (u + gap)),
                anchor: .topLeading,
                proposal: ProposedViewSize(width: CGFloat(s.w) * u + CGFloat(s.w - 1) * gap, height: CGFloat(s.h) * u + CGFloat(s.h - 1) * gap)
            )
        }
    }

    /// The places for `count` photos (0 to 6): the large one 6 by 6 units, left (or right, mirrored).
    static func specs(for count: Int, mirrored: Bool) -> [Spec] {
        let base: [Spec] = switch count {
        case 0: []
        case 1: [Spec(c: 0, r: 0, w: 12, h: 6)]
        case 2: [Spec(c: 0, r: 0, w: 6, h: 5), Spec(c: 6, r: 0, w: 6, h: 5)]
        case 3: [Spec(c: 0, r: 0, w: 6, h: 6), Spec(c: 6, r: 0, w: 6, h: 3), Spec(c: 6, r: 3, w: 6, h: 3)]
        case 4: [Spec(c: 0, r: 0, w: 6, h: 6), Spec(c: 6, r: 0, w: 6, h: 3), Spec(c: 6, r: 3, w: 3, h: 3), Spec(c: 9, r: 3, w: 3, h: 3)]
        case 5: [Spec(c: 0, r: 0, w: 6, h: 6), Spec(c: 6, r: 0, w: 3, h: 3), Spec(c: 9, r: 0, w: 3, h: 3), Spec(c: 6, r: 3, w: 3, h: 3), Spec(c: 9, r: 3, w: 3, h: 3)]
        default: [Spec(c: 0, r: 0, w: 6, h: 6), Spec(c: 6, r: 0, w: 3, h: 3), Spec(c: 9, r: 0, w: 3, h: 3),
                  Spec(c: 6, r: 3, w: 2, h: 3), Spec(c: 8, r: 3, w: 2, h: 3), Spec(c: 10, r: 3, w: 2, h: 3)]
        }
        return mirrored ? base.map { Spec(c: 12 - $0.c - $0.w, r: $0.r, w: $0.w, h: $0.h) } : base
    }
}

/// Where the blocks and the photos on screen are, and the pointer, in the view's space (a reference: updating it
/// redraws nothing).
final class BlockFrames {
    var blocks: [String: CGRect] = [:]
    var tiles: [Int: CGRect] = [:]
    var pointer: CGPoint?

    /// The photo (its grid index) under `p`.
    func tile(at p: CGPoint) -> Int? {
        tiles.first { $0.value.contains(p) }?.key
    }

    /// The block under `p`, or the nearest one when it is over a gap.
    func block(near p: CGPoint) -> String? {
        blocks.min { $0.value.distance(to: p) < $1.value.distance(to: p) }?.key
    }

}

private extension CGRect {
    func distance(to p: CGPoint) -> CGFloat {
        hypot(max(minX - p.x, 0, p.x - maxX), max(minY - p.y, 0, p.y - maxY))
    }
}

private extension View {
    /// Tells `set` where this is (in the "blocks" space, the blocks view's) while it is on screen, nil when it is not.
    func tracked(_ set: @escaping (CGRect?) -> Void) -> some View {
        onGeometryChange(for: CGRect.self) { $0.frame(in: .named("blocks")) } action: { set($0) }
            .onDisappear { set(nil) }
    }
}
#endif
