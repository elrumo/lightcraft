import LightCraftCore
import SwiftUI

/// A photo as it is, at a size of its own (`render.variant {edited}`): Compare, Survey, Reference,
/// the slideshow — several photos large at once, beside the loupe's own picture.
struct PhotoPicture: View {
    @Environment(AppModel.self) private var model
    @Environment(\.displayScale) private var displayScale
    let photo: UInt64
    @State private var slot: String?
    @State private var size: CGSize = .zero

    var body: some View {
        Color.clear
            .overlay {
                if let slot, let p = model.images.pictures[slot] {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFit()
                } else if let p = model.images.thumb(photo) {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFit()
                } else {
                    ProgressView()
                }
            }
            .onGeometryChange(for: CGSize.self) { $0.size } action: { size = $0 }
            .task(id: Key(photo: photo, edge: edge, revision: model.settledRevision)) {
                model.wantThumb(photo, edge: 256)
                guard let engine = model.engine, edge > 0 else { return }
                slot = (try? await engine.request("render.variant", ["photo": .int(Int64(clamping: photo)), "edited": true, "edge": .int(Int64(edge))]))?["slot"].string
            }
            .onDisappear { model.thumbDisappeared(photo) }
    }

    /// The long edge to draw at, in pixels (steps of 256, so a small change doesn't ask again).
    private var edge: Int {
        let px = max(size.width, size.height) * displayScale
        return px > 0 ? min(4096, Int((px / 256).rounded(.up)) * 256) : 0
    }

    private nonisolated struct Key: Hashable {
        let photo: UInt64
        let edge: Int
        let revision: UInt64
    }
}

/// Compare: the Select beside a Candidate from the chosen photos; Swap, Make Select (the candidate
/// becomes the Select and the next one comes up), the candidates one by one. A tap makes a side
/// the active photo (ratings and flags go to it).
struct CompareView: View {
    @Environment(AppModel.self) private var model
    let ids: [UInt64]
    @State private var select: UInt64?
    @State private var candidate: UInt64?
    /// The hinge across the screen when an iPhone Duo is partly open: each photo goes on a page of its own.
    @State private var fold: DuoFold?

    var body: some View {
        VStack(spacing: 8) {
            photos
                .padding(8)
            GlassEffectContainer {
                HStack(spacing: 12) {
                    Button("Previous Candidate", systemImage: "chevron.left") { step(-1) }
                    Button("Swap", systemImage: "arrow.left.arrow.right") { swap(&select, &candidate) }
                    Button("Make Select", systemImage: "checkmark.circle") { makeSelect() }
                    Button("Next Candidate", systemImage: "chevron.right") { step(1) }
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .controlSize(.large)
            }
            // (not across a hinge: on the second page of a book, the lower one of a tabletop)
            .padding(.leading, fold?.vertical == true ? fold?.frame.maxX ?? 0 : 0)
            .padding(.bottom, 8)
        }
        .background(Color.lcBackground)
        .onFold($fold)
        .navigationTitle("Compare")
        .inlineTitle()
        .onAppear {
            if select == nil { select = model.state.active.flatMap { ids.contains($0) ? $0 : nil } ?? ids.first }
            if candidate == nil { candidate = ids.first { $0 != select } }
            // the arrow keys, and the next photo after a rating with Auto Advance, bring up the next candidate
            model.cullStep = { by in step(by) }
        }
        .onDisappear { model.cullStep = nil }
    }

    /// The Select and the Candidate side by side; with a hinge, the Select on one page and the Candidate on the other.
    @ViewBuilder private var photos: some View {
        if let fold, fold.vertical {
            HStack(spacing: 0) {
                side(select, "Select").frame(width: max(0, fold.frame.minX - 8))
                Color.clear.frame(width: fold.frame.width)
                side(candidate, "Candidate")
            }
        } else if let fold {
            VStack(spacing: 0) {
                side(select, "Select").frame(height: max(0, fold.frame.minY - 8))
                Color.clear.frame(height: fold.frame.height)
                side(candidate, "Candidate")
            }
        } else {
            HStack(spacing: 8) {
                side(select, "Select")
                side(candidate, "Candidate")
            }
        }
    }

    private func side(_ photo: UInt64?, _ label: LocalizedStringKey) -> some View {
        Group {
            if let photo {
                PhotoPicture(photo: photo)
                    .overlay(alignment: .top) {
                        Text(label)
                            .font(.caption.weight(.semibold))
                            .padding(.horizontal, 10).padding(.vertical, 4)
                            .glassEffect(.regular, in: .capsule)
                            .padding(8)
                    }
                    .overlay(alignment: .topLeading) {
                        if model.state.active == photo {
                            RoundedRectangle(cornerRadius: 4).stroke(Color.accentColor, lineWidth: 3)
                        }
                    }
                    .contentShape(Rectangle())
                    .onTapGesture { Task { await model.select(photo) } }
                    .accessibilityElement()
                    .accessibilityLabel(label)
                    .accessibilityAddTraits(.isButton)
            } else {
                Color.clear
            }
        }
    }

    /// The next (or previous) chosen photo that isn't the Select, as the Candidate.
    private func step(_ by: Int) {
        let others = ids.filter { $0 != select }
        guard !others.isEmpty else { return }
        let at = candidate.flatMap { others.firstIndex(of: $0) } ?? -by
        candidate = others[((at + by) % others.count + others.count) % others.count]
    }

    private func makeSelect() {
        guard let c = candidate else { return }
        select = c
        candidate = nil
        step(1)
    }
}

/// Survey: the chosen photos side by side, as large as the screen allows; a tap makes one the
/// active photo, ✕ takes one out, a double tap opens it.
struct SurveyView: View {
    @Environment(AppModel.self) private var model
    @State var ids: [UInt64]

    var body: some View {
        GeometryReader { geo in
            let across = Self.columns(ids.count, in: geo.size)
            let rows = max(1, (ids.count + across - 1) / across)
            let h = (geo.size.height - CGFloat(rows + 1) * 8) / CGFloat(rows)
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 8), count: across), spacing: 8) {
                ForEach(ids, id: \.self) { photo in
                    PhotoPicture(photo: photo)
                        .frame(height: max(h, 60))
                        .overlay {
                            if model.state.active == photo {
                                RoundedRectangle(cornerRadius: 4).stroke(Color.accentColor, lineWidth: 3)
                            }
                        }
                        .overlay(alignment: .topTrailing) {
                            Button("Remove from Survey", systemImage: "xmark") { ids.removeAll { $0 == photo } }
                                .labelStyle(.iconOnly)
                                .buttonStyle(.glass)
                                .buttonBorderShape(.circle)
                                .controlSize(.small)
                                .padding(6)
                        }
                        .contentShape(Rectangle())
                        .onTapGesture(count: 2) { model.open(photo) }
                        .onTapGesture { Task { await model.select(photo) } }
                }
            }
            .padding(8)
        }
        .background(Color.lcBackground)
        .navigationTitle("Survey")
        .inlineTitle()
        .onAppear {
            // the arrow keys, and the next photo after a rating with Auto Advance, move the active photo along the survey
            model.cullStep = { by in
                guard !ids.isEmpty else { return }
                let at = model.state.active.flatMap { ids.firstIndex(of: $0) } ?? (by > 0 ? -1 : ids.count)
                let next = min(max(at + by, 0), ids.count - 1)
                Task { await model.select(ids[next]) }
            }
        }
        .onDisappear { model.cullStep = nil }
    }

    /// Columns that give `n` photos the largest tiles in `size` (photos about 3:2).
    static func columns(_ n: Int, in size: CGSize) -> Int {
        guard n > 1, size.width > 0, size.height > 0 else { return 1 }
        var best = (1, 0.0)
        for c in 1...n {
            let rows = Double((n + c - 1) / c)
            let w = Double(size.width) / Double(c)
            let h = Double(size.height) / rows
            let tile = min(w, h * 1.5)
            if tile > best.1 { best = (c, tile) }
        }
        return best.0
    }
}

/// A slideshow of the grid's photos over black: each a few seconds, crossfading; a tap pauses or
/// plays, ✕ ends it.
struct SlideshowView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var index = 0
    @State private var playing = true

    var body: some View {
        let library = model.library
        ZStack {
            Color.black.ignoresSafeArea()
            if let photo = library.photo(at: index, model.engine) {
                PhotoPicture(photo: photo.id)
                    .id(photo.id)
                    .transition(.opacity)
                    .ignoresSafeArea()
            }
        }
        .overlay(alignment: .topTrailing) {
            Button("Close", systemImage: "xmark") { dismiss() }
                .labelStyle(.iconOnly)
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .padding()
        }
        .overlay(alignment: .bottom) {
            if !playing {
                Label("Paused", systemImage: "pause.fill")
                    .padding(.horizontal, 14).padding(.vertical, 8)
                    .glassEffect(.regular, in: .capsule)
                    .padding()
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { playing.toggle() }
        .task(id: playing) {
            guard playing else { return }
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(4))
                guard !Task.isCancelled, library.count > 0 else { return }
                withAnimation(.easeInOut(duration: 0.8)) { index = (index + 1) % library.count }
            }
        }
        .onAppear {
            if let active = model.state.active, let i = library.index(of: active) { index = i }
            Platform.keepAwake(true)
        }
        .onDisappear { Platform.keepAwake(false) }
        .lcStatusBarHidden()
    }
}

/// Lights Out: the photo alone on black, nothing else; a tap brings the app back.
struct LightsOutView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            if let photo = model.state.active {
                PhotoPicture(photo: photo).padding()
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { dismiss() }
        .lcStatusBarHidden()
        .accessibilityAction(named: "Close") { dismiss() }
    }
}

/// The Navigator: the whole photo small in a corner while zoomed in, with the part on screen marked.
struct NavigatorView: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    /// The part on screen, as fractions of the photo (x0, y0, x1, y1).
    let visible: CGRect

    var body: some View {
        Group {
            if let p = model.images.thumb(photo) {
                Image(decorative: p.image, scale: 1).resizable().scaledToFit()
                    .overlay {
                        GeometryReader { g in
                            Rectangle()
                                .stroke(.white, lineWidth: 1.5)
                                .shadow(color: .black.opacity(0.6), radius: 1)
                                .frame(width: g.size.width * visible.width, height: g.size.height * visible.height)
                                .offset(x: g.size.width * visible.minX, y: g.size.height * visible.minY)
                        }
                    }
            }
        }
        .frame(width: 110, height: 110)
        .padding(6)
        .glassEffect(.regular, in: .rect(cornerRadius: 10))
        .task(id: photo) { model.wantThumb(photo, edge: 256) }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// The filmstrip under the photo: the grid's photos in a row; a tap shows one.
struct FilmstripView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let library = model.library
        ScrollViewReader { proxy in
            ScrollView(.horizontal) {
                LazyHStack(spacing: 4) {
                    ForEach(0..<library.count, id: \.self) { i in
                        FilmstripCell(index: i)
                            .id(i)
                    }
                }
                .padding(.horizontal, 8)
            }
            .lcNoScrollers()
            .frame(height: 64)
            .onChange(of: model.state.active) { _, active in
                guard let active, let i = library.index(of: active) else { return }
                withAnimation { proxy.scrollTo(i, anchor: .center) }
            }
        }
    }
}

private struct FilmstripCell: View {
    @Environment(AppModel.self) private var model
    let index: Int

    var body: some View {
        let photo = model.library.photo(at: index, model.engine)
        let on = photo.map { $0.id == model.state.active } ?? false
        Color.lcTertiaryFill
            .overlay {
                if let photo, let p = model.images.thumb(photo.id) {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFill()
                }
            }
            .frame(width: 56, height: 56)
            .clipShape(.rect(cornerRadius: 4))
            .overlay { if on { RoundedRectangle(cornerRadius: 4).stroke(Color.accentColor, lineWidth: 2) } }
            .contentShape(Rectangle())
            .onTapGesture { if let photo { Task { await model.select(photo.id) } } }
            .task(id: photo?.id) { if let id = photo?.id { model.wantThumb(id, edge: 128) } }
            .onDisappear { if let id = photo?.id { model.thumbDisappeared(id) } }
            .accessibilityLabel(photo?.title ?? photo?.fileName ?? "")
            .accessibilityAddTraits(on ? [.isButton, .isSelected] : .isButton)
    }
}
