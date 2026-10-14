#if os(macOS)
import AppKit
import LightCraftCore
import SwiftUI

/// What a Mac does with a grid cell that a phone doesn't: a click chooses it (⌘ adds it or takes it
/// out, ⇧ chooses the run from the last one chosen), a double click opens it, the pointer lights it,
/// a right click has its menu, and it can be dragged out (with the others chosen, when it is one of
/// them). The chosen cells have a ring; there is no select mode.
struct MacCellInteraction: ViewModifier {
    @Environment(AppModel.self) private var model
    let photo: PhotoSummary
    @State private var hovering = false

    func body(content: Content) -> some View {
        let chosen = model.library.selected.contains(photo.id)
        content
            .overlay { if hovering, !chosen { Color.white.opacity(0.12) } }
            .overlay { if chosen { Rectangle().strokeBorder(Color.accentColor, lineWidth: 3) } }
            .onHover { hovering = $0 }
            .onTapGesture { clicked() }
            .simultaneousGesture(TapGesture(count: 2).onEnded { model.open(photo.id) })
            .contextMenu { PhotoContextMenu(photo: photo.id) }
            .modifier(DragChosen(on: true, photo: photo.id))
    }

    private func clicked() {
        let id: JSONValue = [.int(Int64(clamping: photo.id))]
        if let keyword = model.painting {
            model.perform("keyword.toggle", ["ids": id, "keyword": .string(keyword)])
            return
        }
        let keys = NSEvent.modifierFlags
        let mode = keys.contains(.command) ? "toggle" : keys.contains(.shift) ? "range" : "replace"
        model.perform("library.select", ["ids": id, "mode": .string(mode)])
    }
}

/// A photo's menu in the grid (a right click): open it, rate, flag and label, albums, copying edits,
/// renaming and the rest, export, delete. It acts on the photos chosen when this is one of them,
/// else on this one.
struct PhotoContextMenu: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64

    var body: some View {
        let chosen = model.library.selected
        let ids = chosen.contains(photo) ? Array(chosen) : [photo]
        Button("Open", systemImage: "arrow.up.left.and.arrow.down.right") { model.open(photo) }
        Divider()
        Menu("Rating", systemImage: "star") { RatingItems(ids: ids) }
        Menu("Flag", systemImage: "flag") { FlagItems(ids: ids) }
        Menu("Color Label", systemImage: "circle.lefthalf.filled") { LabelItems(ids: ids) }
        Menu("Add to Album", systemImage: "rectangle.stack.badge.plus") { AlbumItems(ids: ids) }
        Divider()
        SelectionMoreItems(ids: ids)
        Divider()
        Button("Export…", systemImage: "square.and.arrow.up") { model.sheet = .export(ids) }
        DeleteButton(ids: ids) { Label("Delete", systemImage: "trash") }
    }
}

/// The bar under the grid, as Lightroom's has it: how much of the library the grid shows (Years,
/// Months, All) at the left, the rating and flag of what is chosen in the middle, and at the right how many
/// are chosen and the size of the thumbnails.
struct MacGridBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var prefs = model.prefs
        let chosen = Array(model.library.selected)
        let first = chosen.first.flatMap { model.library.index(of: $0) }.flatMap { model.library.loaded(at: $0) }
        GeometryReader { geo in
            HStack(spacing: 16) {
                Picker("Show", selection: $prefs.gridSpan) {
                    ForEach(Prefs.GridSpan.allCases) { Text($0.label).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                Spacer(minLength: 8)
                if geo.size.width >= 520 {
                    MacStars(rating: first?.rating ?? 0, ids: chosen)
                    MacFlags(flag: first?.flag, ids: chosen)
                    Spacer(minLength: 8)
                }
                Text(chosen.count > 1 ? "\(chosen.count) of \(model.library.count) selected" : "\(model.library.count) photos")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Image(systemName: "square.grid.3x3").font(.caption).foregroundStyle(.secondary)
                Slider(value: $prefs.thumbSize, in: Prefs.thumbSizes)
                    .frame(width: 110)
                    .controlSize(.small)
                    .accessibilityLabel("Thumbnail Size")
                Image(systemName: "square.grid.2x2").font(.caption).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 12)
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .frame(height: 38)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }
}
#endif
