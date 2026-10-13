#if os(iOS)
import LightCraftCore
import SwiftUI

/// What the loupe asks the engine for: a new request when any of it changes.
nonisolated struct ViewRequest: Hashable {
    let photo: UInt64
    let width: Int
    let height: Int
    let revision: UInt64
    let dragging: Bool
}

/// One photo, fitted to the screen: pinch to zoom, drag to pan, double tap for 2× and back.
/// While a slider drags, the edit sheet asks for drafts; when it lets go, the full render follows.
struct LoupeView: View {
    let photoID: UInt64
    @Environment(AppModel.self) private var model
    @Environment(\.displayScale) private var displayScale
    @State private var zoom: CGFloat = 1
    @State private var zoomAtStart: CGFloat = 1
    @State private var pan: CGSize = .zero
    @State private var panAtStart: CGSize = .zero
    @State private var canvas: CGSize = .zero

    /// The largest render asked for (zoomed views get sharper up to this; tiles of the zoomed
    /// window are S1 work, docs/ios-swift.md).
    private let maxEdge: CGFloat = 4096

    var body: some View {
        @Bindable var model = model
        ZStack {
            Color.black.ignoresSafeArea()
            if let picture = model.images.view(for: photoID) {
                Image(decorative: picture.image, scale: 1)
                    .resizable()
                    .scaledToFit()
                    .scaleEffect(zoom)
                    .offset(pan)
                    .accessibilityLabel("Photo")
            } else if let error = model.images.failed["main"] {
                ContentUnavailableView("Can't show this photo", systemImage: "exclamationmark.triangle", description: Text(error))
            } else {
                ProgressView()
            }
        }
        .contentShape(Rectangle())
        .gesture(magnify.simultaneously(with: drag))
        .onTapGesture(count: 2) {
            withAnimation(.snappy) {
                if zoom > 1 {
                    zoom = 1
                    pan = .zero
                } else {
                    zoom = 2
                }
                zoomAtStart = zoom
                panAtStart = pan
            }
        }
        .onGeometryChange(for: CGSize.self) { $0.size } action: { canvas = $0 }
        .task { await model.select(photoID) }
        .task(id: request) {
            guard let r = request, !r.dragging else { return }
            model.requestView(r.photo, width: r.width, height: r.height)
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button("Undo", systemImage: "arrow.uturn.backward") { model.undo() }
                    .disabled(model.state.undo == nil)
                Button("Redo", systemImage: "arrow.uturn.forward") { model.redo() }
                    .disabled(model.state.redo == nil)
            }
            ToolbarItem(placement: .bottomBar) {
                Button("Edit", systemImage: "slider.horizontal.3") { model.editing = true }
            }
        }
        .sheet(isPresented: $model.editing) {
            EditSheet()
                .presentationDetents([.fraction(0.38), .medium, .large])
                .presentationBackgroundInteraction(.enabled(upThrough: .medium))
        }
    }

    /// The render this screen wants: the canvas in pixels, sharper when zoomed in.
    private var request: ViewRequest? {
        guard canvas.width > 0, canvas.height > 0 else { return nil }
        let zoomBucket: CGFloat = zoom >= 1.5 ? 2 : 1
        var w = canvas.width * displayScale * zoomBucket
        var h = canvas.height * displayScale * zoomBucket
        let long = max(w, h)
        if long > maxEdge {
            w *= maxEdge / long
            h *= maxEdge / long
        }
        return ViewRequest(photo: photoID, width: Int(w), height: Int(h), revision: model.state.catalogRevision, dragging: model.state.interaction != nil)
    }

    private var magnify: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                zoom = min(max(zoomAtStart * value.magnification, 1), 8)
                pan = clamped(pan)
            }
            .onEnded { _ in
                zoomAtStart = zoom
                if zoom <= 1.01 {
                    zoom = 1
                    pan = .zero
                }
                panAtStart = pan
            }
    }

    private var drag: some Gesture {
        DragGesture(minimumDistance: 4)
            .onChanged { value in
                guard zoom > 1 else { return }
                pan = clamped(CGSize(width: panAtStart.width + value.translation.width, height: panAtStart.height + value.translation.height))
            }
            .onEnded { _ in panAtStart = pan }
    }

    /// Keep the zoomed photo over the screen (view math only: the photo is fitted, then scaled).
    private func clamped(_ p: CGSize) -> CGSize {
        let maxX = canvas.width * (zoom - 1) / 2
        let maxY = canvas.height * (zoom - 1) / 2
        return CGSize(width: min(max(p.width, -maxX), maxX), height: min(max(p.height, -maxY), maxY))
    }
}
#endif
