#if os(iOS)
import LightCraftCore
import SwiftUI

/// Months or Years, as iOS's Photos has them for getting about a large library fast: each month a
/// heading over a few of its photos (the best one large, the rest spread through the month, each
/// with its day), each year a card with its best photo. A tap on a month's photo opens All at that
/// photo, on its heading at the month's first photo; a tap on a year shows its months.
struct DateBlocksView: View {
    @Environment(AppModel.self) private var model
    let span: Prefs.GridSpan
    /// Show All at this grid index.
    let openAt: (Int) -> Void
    @State private var blocks: [DateBlockInfo] = []
    @State private var jumpTo: String?

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 22) {
                    ForEach(blocks) { block in
                        Group {
                            if span == .years {
                                YearCard(block: block) {
                                    withAnimation(.snappy) { model.prefs.gridSpan = .months }
                                }
                            } else {
                                MonthBlock(block: block, openAt: openAt)
                            }
                        }
                        .id(block.key)
                    }
                }
                .padding(.horizontal, 12)
                .padding(.top, 8)
                Color.clear.frame(height: 96)
            }
            .onChange(of: jumpTo) { _, key in
                if let key { proxy.scrollTo(key, anchor: .top) }
            }
        }
        .task(id: Key(span: span, gen: model.state.visibleGen, rev: model.state.catalogRevision)) {
            guard let engine = model.engine else { return }
            let name = span == .years ? "years" : "months"
            blocks = (try? await engine.execute("library.blocks", ["span": .string(name)], as: [DateBlockInfo].self)) ?? []
        }
        .haptic(.impact(weight: .light), trigger: span)
    }

    private nonisolated struct Key: Hashable {
        let span: Prefs.GridSpan
        let gen: UInt64
        let rev: UInt64
    }
}

/// A month: its name and count, then its photos in 1, 2, 3 or 6 tiles, the best one large.
private struct MonthBlock: View {
    let block: DateBlockInfo
    let openAt: (Int) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                openAt(block.photos.map(\.index).min() ?? 0)
            } label: {
                VStack(alignment: .leading, spacing: 0) {
                    Text(dateTitle(key: block.key, label: block.label)).font(.title2.weight(.semibold))
                    Text("\(block.count) photos").font(.footnote).foregroundStyle(.secondary)
                }
            }
            .buttonStyle(.plain)
            tiles
        }
    }

    @ViewBuilder private var tiles: some View {
        let shown = block.photos
        let gap: CGFloat = 4
        switch shown.count {
        case 0: EmptyView()
        case 1: tile(shown[0]).aspectRatio(1 / 0.6, contentMode: .fit)
        case 2:
            HStack(spacing: gap) { tile(shown[0]); tile(shown[1]) }.aspectRatio(2, contentMode: .fit)
        case 3...5:
            HStack(spacing: gap) {
                tile(shown[0]).aspectRatio(1, contentMode: .fit)
                VStack(spacing: gap) { tile(shown[1]); tile(shown[2]) }
                    .aspectRatio(0.5, contentMode: .fit)
            }
        default:
            VStack(spacing: gap) {
                HStack(spacing: gap) {
                    tile(shown[0]).aspectRatio(1, contentMode: .fit)
                    VStack(spacing: gap) { tile(shown[1]); tile(shown[2]) }
                        .aspectRatio(0.5, contentMode: .fit)
                }
                HStack(spacing: gap) {
                    tile(shown[3]); tile(shown[4]); tile(shown[5])
                }
                .aspectRatio(3, contentMode: .fit)
            }
        }
    }

    private func tile(_ s: DateBlockInfo.Shown) -> some View {
        BlockTile(photo: s.id, caption: s.day)
            .onTapGesture { openAt(s.index) }
    }
}

/// A year: its best photo, the year over it.
private struct YearCard: View {
    let block: DateBlockInfo
    let open: () -> Void

    var body: some View {
        BlockTile(photo: block.photos.first?.id ?? 0, caption: nil)
            .aspectRatio(1 / 0.66, contentMode: .fit)
            .overlay(alignment: .topLeading) {
                VStack(alignment: .leading, spacing: 0) {
                    Text(block.key.isEmpty ? String(localized: "No Date") : block.key)
                        .font(.system(size: 30, weight: .bold))
                    Text("\(block.count) photos").font(.footnote)
                }
                .foregroundStyle(.white)
                .shadow(radius: 4)
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(LinearGradient(colors: [.black.opacity(0.45), .clear], startPoint: .top, endPoint: .bottom))
            }
            .clipShape(.rect(cornerRadius: 14))
            .onTapGesture(perform: open)
    }
}

/// A rounded photo tile, with the day it was taken in its corner.
private struct BlockTile: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    let caption: String?

    var body: some View {
        Color(uiColor: .secondarySystemBackground)
            .overlay {
                if let picture = model.images.thumb(photo) {
                    Image(decorative: picture.image, scale: 1).resizable().scaledToFill()
                }
            }
            .overlay(alignment: .topLeading) {
                if let caption {
                    Text(caption)
                        .font(.system(size: 20, weight: .semibold))
                        .foregroundStyle(.white)
                        .shadow(radius: 3)
                        .padding(8)
                }
            }
            .clipShape(.rect(cornerRadius: 14))
            .contentShape(.rect(cornerRadius: 14))
            .task(id: model.state.catalogRevision) { model.requestThumb(photo, edge: 768) }
            .onAppear { model.thumbAppeared(photo) }
            .onDisappear { model.thumbDisappeared(photo) }
    }
}
#endif
