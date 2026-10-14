#if os(iOS)
import LightCraftCore
import SwiftUI

/// A photo on a dark canvas, as Lightroom's mobile app shows it: back, its date, undo, share and
/// "…" above it; swipe sideways for the next; the tools in the bottom bar (Presets, Crop, Edit,
/// Masking, Remove, Info), the Edit tool's groups in a row of their own and the open tool's panel
/// between the photo and the tools. Crop, Masking, Remove and Red Eye take the screen with Cancel
/// and Done above. A tap on the photo hides the bars (review mode).
struct PhotoScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(\.verticalSizeClass) private var verticalSize
    @Environment(\.horizontalSizeClass) private var horizontalSize
    let start: UInt64
    @State private var review = false

    var body: some View {
        let editor = model.editor
        // an iPad: the tools in the inspector beside the photo; a phone held sideways: in a column
        // at the right (egui's wide layout, 316 pt); held upright: under the photo
        let inspector = horizontalSize == .regular && verticalSize != .compact
        let sideways = verticalSize == .compact
        Group {
            if inspector {
                // (clipped: the next photos mustn't show through the inspector's glass)
                pager.clipped()
            } else if sideways, editor.tool != nil {
                HStack(spacing: 0) {
                    pager
                    if !review {
                        ToolColumn()
                            .frame(width: 316)
                            .background(Color(uiColor: .secondarySystemBackground))
                            .transition(.move(edge: .trailing))
                    }
                }
            } else {
                VStack(spacing: 0) {
                    pager
                    if !review {
                        if let tool = editor.tool, tool.hasScreen {
                            ToolScreenBar(tool: tool)
                        } else {
                            if editor.tool == .edit { GroupBar() }
                            if let tool = editor.tool { ToolPanel(tool: tool) }
                        }
                    }
                }
            }
        }
        .background(Color.black.ignoresSafeArea())
        .inspector(isPresented: Binding(get: { inspector && editor.tool != nil && !review }, set: { if !$0 { editor.tool = nil } })) {
            ToolColumn()
                .inspectorColumnWidth(min: 300, ideal: 340, max: 420)
        }
        .background(DismissGuard(disabled: editor.touching))
        .toolbar(review ? .hidden : .visible, for: .navigationBar)
        .toolbar(review || editor.tool?.hasScreen == true ? .hidden : .visible, for: .bottomBar)
        .statusBarHidden(review)
        .navigationBarBackButtonHidden(editor.tool?.hasScreen == true)
        .navigationTitle(title)
        .navigationSubtitle(Text(subtitle))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let tool = editor.tool, tool.hasScreen {
                ScreenToolbar(model: model)
                ToolbarItemGroup(placement: .topBarTrailing) {
                    UndoMenu()
                    if tool == .masking {
                        MaskOverlayButton()
                        MaskMoreMenu()
                    }
                }
            } else {
                ToolbarItem(placement: .topBarTrailing) {
                    UndoMenu()
                }
                ToolbarSpacer(.fixed, placement: .topBarTrailing)
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button("Share", systemImage: "square.and.arrow.up") {
                        if let id = model.state.active { model.sheet = .export([id]) }
                    }
                    PhotoMoreMenu()
                }
                ToolBar(model: model)
            }
        }
        // editing's canvas is dark, whatever the app's look (as Photos' editor and Lightroom)
        .environment(\.colorScheme, .dark)
        .toolbarColorScheme(.dark, for: .navigationBar, .bottomBar)
        .animation(.snappy, value: review)
        .animation(.snappy, value: editor.tool)
        .onChange(of: editor.tool) { editor.picking = nil }
        .haptic(.selection, trigger: editor.tool)
        .task(id: editor.tool) {
            // a tool screen's Cancel goes back to how the photo was when it opened
            if editor.tool?.hasScreen == true { editor.screenBase = await undoDepth(model) }
        }
        .onAppear {
            if editor.tool == nil { editor.tool = .edit }
            Task { await editor.reload(model) }
        }
        .onDisappear {
            if model.path.isEmpty { editor.tool = nil }
        }
    }

    /// The photo's capture date (an open tool screen's name instead), then its time.
    private var title: String {
        if let tool = model.editor.tool, tool.hasScreen { return String(localized: tool.screenTitle) }
        guard let summary = active else { return "" }
        return summary.captured.flatMap(InfoPanel.captureDate)?.formatted(date: .abbreviated, time: .omitted) ?? summary.fileName
    }

    private var subtitle: String {
        if model.editor.tool?.hasScreen == true { return "" }
        guard let iso = active?.captured, iso.count > 10, let date = InfoPanel.captureDate(iso) else { return "" }
        return date.formatted(date: .omitted, time: .shortened)
    }

    private var active: PhotoSummary? {
        model.state.active.flatMap { model.library.index(of: $0) }.flatMap { model.library.loaded(at: $0) }
    }

    private var pager: some View {
        VStack(spacing: 0) {
            // Reference View: the reference photo beside the one being edited
            if model.editor.referenceView, let ref = model.reference, ref != model.state.active, !review {
                let layout = verticalSize == .compact || horizontalSize == .regular ? AnyLayout(HStackLayout(spacing: 4)) : AnyLayout(VStackLayout(spacing: 4))
                layout {
                    PhotoPicture(photo: ref)
                        .padding(12)
                        .overlay(alignment: .bottom) {
                            Text("Reference").font(.caption.weight(.semibold)).padding(4).glassEffect(.regular, in: .capsule).padding(6)
                        }
                    photoPager
                }
            } else {
                photoPager
            }
            if model.prefs.filmstrip, !review {
                FilmstripView()
            }
        }
    }

    private var photoPager: some View {
        PhotoPager(start: start, startIndex: model.library.index(of: start), review: $review)
            .overlay(alignment: .topTrailing) {
                if model.editor.tool == .masking, !review {
                    MaskColumn().padding(10).transition(.move(edge: .trailing).combined(with: .opacity))
                }
            }
    }
}

extension EnvironmentValues {
    /// The tools are in a column beside the photo (an iPad's inspector, a phone held sideways), not
    /// under it. (An inspector's content is compact width, so the size classes can't tell.)
    @Entry var toolColumn = false
}

/// The tools in a column (an iPad's inspector, a phone held sideways): a tool's own screen (Crop,
/// Masking, Remove, Red Eye), or the Edit groups and the open tool's controls.
private struct ToolColumn: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let editor = model.editor
        VStack(spacing: 0) {
            if let tool = editor.tool, tool.hasScreen {
                // (at the bottom, where the thumb is, as on a phone held upright)
                Spacer(minLength: 0)
                ViewThatFits(in: .vertical) {
                    ToolScreenBar(tool: tool)
                    ScrollView { ToolScreenBar(tool: tool) }
                }
            } else {
                if editor.tool == .edit { GroupBar() }
                if let tool = editor.tool {
                    ScrollView { ToolPanelContent(tool: tool) }
                } else {
                    Spacer()
                }
            }
        }
        .environment(\.toolColumn, true)
    }
}

/// The photos of the grid, one per page: swipe for the next. The active one renders at the screen's
/// size; the others show their thumbnails until they become active.
struct PhotoPager: View {
    @Environment(AppModel.self) private var model
    let start: UInt64
    @Binding var review: Bool
    @State private var position: Int?
    @State private var zoomed = false
    /// Where the photo opened is in the grid (found once: pages being read again mustn't lose it).
    @State private var startIndex: Int?
    /// The photo a swipe selected, until the engine says it's the active one (states from earlier
    /// swipes mustn't pull the pager back).
    @State private var asked: UInt64?

    /// `startIndex`: where `start` is in the grid, when its page is loaded. The scroll view opens on
    /// it from its first layout: set later (on appear), the first page could take its place and
    /// become the photo selected.
    init(start: UInt64, startIndex: Int?, review: Binding<Bool>) {
        self.start = start
        _review = review
        _position = State(initialValue: startIndex)
        _startIndex = State(initialValue: startIndex)
    }

    var body: some View {
        let library = model.library
        Group {
            if let startIndex = startIndex ?? library.index(of: start) {
                ScrollView(.horizontal) {
                    LazyHStack(spacing: 0) {
                        ForEach(0..<library.count, id: \.self) { i in
                            PhotoPage(index: i, active: i == (position ?? startIndex), review: $review, zoomed: $zoomed)
                                .containerRelativeFrame(.horizontal)
                        }
                    }
                    .scrollTargetLayout()
                }
                .scrollTargetBehavior(.paging)
                .scrollIndicators(.hidden)
                .scrollPosition(id: $position)
                .scrollDisabled(zoomed || model.editor.dragging != nil || model.editor.tool?.hasScreen == true)
                .onAppear {
                    if position == nil { position = startIndex }
                    self.startIndex = startIndex
                }
                .onChange(of: position) { _, i in
                    guard let i, let photo = library.photo(at: i, model.engine) else { return }
                    if photo.id != model.state.active {
                        asked = photo.id
                        Task { await model.select(photo.id) }
                    }
                    var near: [UInt64] = []
                    if let next = library.photo(at: i + 1, model.engine) { near.append(next.id) }
                    if let prev = library.photo(at: i - 1, model.engine) { near.append(prev.id) }
                    model.prefetch(near)
                }
                .haptic(.impact(weight: .light), trigger: position)
                // the active photo's place: the engine moved on (Auto Advance, next / previous), or the
                // grid's order changed under it (an import, a sync, sorting by edit time) — the page
                // shown stays the photo being edited
                .onChange(of: model.state.active.flatMap { library.index(of: $0) }) { _, i in
                    if let a = asked {
                        guard model.state.active == a else { return }
                        asked = nil
                    }
                    guard let i, i != position else { return }
                    withAnimation(.snappy) { position = i }
                }
            } else {
                ZoomablePhoto(photo: start, review: $review, zoomed: $zoomed)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// One page of the pager.
private struct PhotoPage: View {
    @Environment(AppModel.self) private var model
    let index: Int
    let active: Bool
    @Binding var review: Bool
    @Binding var zoomed: Bool

    var body: some View {
        if let photo = model.library.photo(at: index, model.engine) {
            if active {
                ZoomablePhoto(photo: photo.id, review: $review, zoomed: $zoomed)
            } else if let picture = model.images.view(for: photo.id) {
                Image(decorative: picture.image, scale: 1)
                    .resizable()
                    .scaledToFit()
                    .padding(24)
                    .task { model.thumbAppeared(photo.id); model.requestThumb(photo.id, edge: 768) }
                    .onDisappear { model.thumbDisappeared(photo.id) }
            } else {
                ProgressView().task { model.thumbAppeared(photo.id); model.requestThumb(photo.id, edge: 768) }
            }
        } else {
            ProgressView()
        }
    }
}

/// What the loupe asks the engine for: a new request when any of it changes.
nonisolated struct ViewRequest: Hashable {
    let photo: UInt64
    let width: Int
    let height: Int
    let revision: UInt64
    let dragging: Bool
    let crop: Bool
    let overlay: JSONValue?
    let proof: JSONValue?
    let zoom: ZoomRequest?
}
    /// The screen shows HDR: the same look is asked for again, as an HDR picture.
    let hdr: Bool

/// A zoomed loupe's view: the photo's long edge as drawn (pixels) and the part of it on screen, in
/// thousandths of its width and height (x0, y0, x1, y1).
nonisolated struct ZoomRequest: Hashable {
    let drawn: Int
    let visible: [Int]
}

/// The active photo, fitted: pinch to zoom, drag to pan, double tap to zoom in and back, a tap to
/// hide the bars, and a long press to see it as it was before the edits.
struct ZoomablePhoto: View {
    @Environment(AppModel.self) private var model
    @Environment(\.displayScale) private var displayScale
    let photo: UInt64
    @Binding var review: Bool
    @Binding var zoomed: Bool
    @State private var zoom: CGFloat = 1
    @State private var zoomAtStart: CGFloat = 1
    @State private var pan: CGSize = .zero
    @State private var panAtStart: CGSize = .zero
    @State private var canvas: CGSize = .zero
    /// The render asked for last.
    @State private var lastRequest: ViewRequest?
    @State private var showBefore = false
    /// How the shown photo maps to image coordinates (taps on it for the pickers and tools).
    @State private var frame: PhotoFrame?
    /// A review swipe under way: the rating or flag it started from and where it is now.
    @State private var swipe: ReviewSwipe?
    /// Points of travel per star and per flag step (as the egui app's review mode).
    private let perStar: CGFloat = 44
    private let perFlag: CGFloat = 56
    /// The largest render asked for (zoomed views get sharper up to this).
    private let maxEdge: CGFloat = 4096
    private let margin: CGFloat = 24

    var body: some View {
        let picture = showBefore ? (model.images.before(for: photo) ?? model.images.view(for: photo)) : model.images.view(for: photo)
        let compare = model.editor.beforeAfter && model.editor.tool?.hasScreen != true
        ZStack {
            if compare {
                BeforeAfterView(photo: photo, wide: canvas.width > canvas.height)
                    .padding(review ? 0 : margin)
            } else if let picture {
                // (an HDR edit on a screen with headroom: its float picture, drawn brighter than white)
                Image(decorative: picture.hdr ?? picture.image, scale: 1)
                    .resizable()
                    .scaledToFit()
                    // the crop's Expand: what the photo doesn't cover yet comes transparent
                    .background { if model.editor.setting("crop", "expand").bool == true { Checkerboard() } }
                    .padding(review ? 0 : margin)
                    .scaleEffect(zoom)
                    .offset(pan)
                    .accessibilityLabel("Photo")
                // zoomed in: the sharp window over it, where it belongs in the photo
                if zoom > 1.01, let region = model.images.region(for: photo), let w = region.window, w.full.count == 2,
                   let rect = shownRect(picture), w.full[0] > 0, w.full[1] > 0, rect.height > 0,
                   abs(Double(w.full[0]) / Double(w.full[1]) / (rect.width / rect.height) - 1) < 0.02
                {
                    let (fw, fh) = (CGFloat(w.full[0]), CGFloat(w.full[1]))
                    Image(decorative: region.hdr ?? region.image, scale: 1)
                        .resizable()
                        .frame(width: rect.width * CGFloat(w.w) / fw, height: rect.height * CGFloat(w.h) / fh)
                        .position(x: rect.minX + rect.width * (CGFloat(w.x) + CGFloat(w.w) / 2) / fw, y: rect.minY + rect.height * (CGFloat(w.y) + CGFloat(w.h) / 2) / fh)
                        .allowsHitTesting(false)
                        .accessibilityHidden(true)
                }
            } else if let error = model.images.failed["main"] {
                ContentUnavailableView("This photo can't be opened", systemImage: "exclamationmark.triangle", description: Text(error))
            } else {
                ProgressView()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .overlay {
            if model.editor.tool == .crop, !showBefore, let rect = shownRect(picture) {
                CropOverlay(photo: photo, rect: rect)
            } else if model.editor.tool == .remove, !showBefore, let rect = shownRect(picture) {
                RemoveCanvas(photo: photo, rect: rect)
            } else if model.editor.tool == .masking, !showBefore, let rect = shownRect(picture) {
                MaskCanvas(photo: photo, rect: rect)
            } else if model.editor.tool == .redEye, !showBefore, let rect = shownRect(picture) {
                RedEyeCanvas(photo: photo, rect: rect)
            } else if model.prefs.faceBoxes || model.editor.tagFace, model.editor.tool?.hasScreen != true, !compare, !showBefore, !review,
                      let rect = shownRect(picture)
            {
                FacesOverlay(photo: photo, rect: rect)
            }
        }
        .overlay(alignment: .topLeading) { InfoOverlay(photo: photo).opacity(review ? 0 : 1) }
        .overlay(alignment: .topTrailing) {
            if model.prefs.showHistogram, !review, let h = model.images.view(for: photo)?.histogram {
                HistogramView(histogram: h).padding(10)
            }
        }
        .overlay {
            // the clipping warning: red around a photo with whites clipped, blue with blacks
            if model.prefs.showClipping, !review, let h = model.images.view(for: photo)?.histogram, let c = h.clipping, c.count == 2,
               let rect = shownRect(picture)
            {
                ZStack {
                    if c[1] > 0.003 { Rectangle().stroke(.red, lineWidth: 2).frame(width: rect.width, height: rect.height).position(x: rect.midX, y: rect.midY) }
                    if c[0] > 0.003 { Rectangle().stroke(.blue, lineWidth: 2).frame(width: rect.width + 6, height: rect.height + 6).position(x: rect.midX, y: rect.midY) }
                }
                .allowsHitTesting(false)
            }
        }
        // soft proofing: a paper-white surround and the proof's name, as Lightroom shows it
        .background { if model.editor.softProof { Color.white } }
        .overlay(alignment: .top) {
            if model.editor.softProof {
                Text("Proof Preview · \(SoftProof.label(model.editor.proofSpace))")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.black.opacity(0.7))
                    .padding(6)
            }
        }
        .overlay(alignment: .topLeading) {
            // zoomed in: the whole photo small, with the part on screen marked
            if zoom > 1.01, model.prefs.navigator, let v = zoomRequest?.visible, v.count == 4 {
                NavigatorView(photo: photo, visible: CGRect(x: CGFloat(v[0]) / 1000, y: CGFloat(v[1]) / 1000,
                                                             width: CGFloat(v[2] - v[0]) / 1000, height: CGFloat(v[3] - v[1]) / 1000))
                    .padding(12)
            }
        }
        .overlay(alignment: .bottom) {
            if showBefore {
                Text("Before")
                    .font(.footnote.weight(.semibold))
                    .padding(.horizontal, 12).padding(.vertical, 6)
                    .glassEffect(.regular, in: .capsule)
                    .padding(.bottom, 12)
            }
        }
        .overlay(alignment: .bottom) {
            if review, let summary {
                ReviewHUD(rating: swipe?.rating ?? summary.rating ?? 0, flag: swipe?.flag ?? Self.flagIndex(summary.flag))
                    .transition(.opacity)
            }
        }
        .contentShape(Rectangle())
        .gesture(magnify.simultaneously(with: drag))
        .simultaneousGesture(reviewSwipe, including: review && !zoomed ? .all : .none)
        .haptic(.selection, trigger: swipe?.rating)
        .haptic(.selection, trigger: swipe?.flag)
        .onTapGesture(count: 2) {
            model.buzz(.light)
            withAnimation(.snappy) {
                if zoom > 1 {
                    zoom = 1
                    pan = .zero
                } else {
                    zoom = model.prefs.clickZoom
                }
                zoomAtStart = zoom
                panAtStart = pan
                zoomed = zoom > 1
            }
        }
        .onTapGesture(count: 1) { location in tapped(at: location, picture: picture) }
        .onLongPressGesture(minimumDuration: 0.4, maximumDistance: 10) {
        } onPressingChanged: { pressing in
            showBefore = pressing && !model.editor.touching
            if showBefore { model.requestBefore(photo) }
        }
        .haptic(trigger: showBefore) { _, new in new ? .impact(weight: .medium) : nil }
        .onGeometryChange(for: CGSize.self) { $0.size } action: { canvas = $0 }
        .task(id: request) {
            guard let r = request, !r.dragging else { return }
            // only the size moved (a panel sliding in or out, a rotation): one render once it has
            // settled, not one a frame (the next frame's change cancels this wait)
            if let last = lastRequest, last.photo == r.photo, last.revision == r.revision, last.width != r.width || last.height != r.height {
                try? await Task.sleep(for: .milliseconds(120))
                guard !Task.isCancelled else { return }
            }
            lastRequest = r
            model.requestView(r.photo, width: r.width, height: r.height, applyCrop: r.crop, zoom: r.zoom)
            if compare { model.requestBefore(photo) }
        }
        .onChange(of: compare) { _, on in
            if on { model.requestBefore(photo) }
        }
        .onChange(of: photo) {
            zoom = 1; zoomAtStart = 1; pan = .zero; panAtStart = .zero; zoomed = false
        }
        .task(id: request) {
            guard let engine = model.engine, let r = request else { return }
            frame = try? await engine.execute("view.frame", ["id": .int(Int64(clamping: photo)), "applyCrop": .bool(r.crop)], as: PhotoFrame.self)
        }
    }

    /// The photo's grid summary (rating, flag), when its page is loaded.
    private var summary: PhotoSummary? {
        model.library.index(of: photo).flatMap { model.library.loaded(at: $0) }
    }

    static func flagIndex(_ flag: String?) -> Int {
        flag == "pick" ? 1 : flag == "reject" ? -1 : 0
    }

    /// Review mode: up and down on the left half sets the stars, on the right half the flag.
    private var reviewSwipe: some Gesture {
        DragGesture(minimumDistance: 12)
            .onChanged { g in
                guard abs(g.translation.height) > abs(g.translation.width) || swipe != nil else { return }
                let stars = g.startLocation.x < canvas.width / 2
                let start = swipe ?? ReviewSwipe(stars: stars, startRating: summary?.rating ?? 0, startFlag: Self.flagIndex(summary?.flag))
                var next = start
                if stars {
                    next.rating = min(max(start.startRating + Int((-g.translation.height / perStar).rounded()), 0), 5)
                } else {
                    next.flag = min(max(start.startFlag + Int((-g.translation.height / perFlag).rounded()), -1), 1)
                }
                swipe = next
            }
            .onEnded { _ in
                guard let s = swipe else { return }
                swipe = nil
                let ids: JSONValue = [.int(Int64(clamping: photo))]
                if s.stars, let r = s.rating, r != s.startRating {
                    model.perform("photo.rate", ["ids": ids, "rating": .int(Int64(r)), "advance": .bool(model.prefs.autoAdvance)])
                } else if !s.stars, let f = s.flag, f != s.startFlag {
                    model.perform("photo.flag", ["ids": ids, "flag": .string(f == 1 ? "pick" : f == -1 ? "reject" : "none"), "advance": .bool(model.prefs.autoAdvance)])
                }
            }
    }

    /// Where the photo is drawn on the canvas: fitted inside the margin, then zoomed and panned.
    func shownRect(_ picture: ImageStore.Picture?) -> CGRect? {
        zoomedRect(picture, zoom: zoom, pan: pan)
    }

    /// Where the photo is drawn at `zoom` and `pan` (view math only: fitted, then scaled).
    private func zoomedRect(_ picture: ImageStore.Picture?, zoom: CGFloat, pan: CGSize) -> CGRect? {
        guard let picture, canvas.width > 0, canvas.height > 0 else { return nil }
        let m = review ? 0 : margin
        let area = CGRect(x: m, y: m, width: canvas.width - 2 * m, height: canvas.height - 2 * m)
        let aspect = CGFloat(picture.image.width) / CGFloat(max(picture.image.height, 1))
        var size = area.width / area.height > aspect ? CGSize(width: area.height * aspect, height: area.height) : CGSize(width: area.width, height: area.width / aspect)
        size = CGSize(width: size.width * zoom, height: size.height * zoom)
        let center = CGPoint(x: canvas.width / 2 + pan.width, y: canvas.height / 2 + pan.height)
        return CGRect(x: center.x - size.width / 2, y: center.y - size.height / 2, width: size.width, height: size.height)
    }

    /// A tap: a picker takes the colour there; otherwise the bars come and go.
    private func tapped(at location: CGPoint, picture: ImageStore.Picture?) {
        guard let pick = model.editor.picking else {
            if !model.editor.touching { review.toggle() }
            return
        }
        guard let rect = shownRect(picture), rect.contains(location), let frame else { return }
        let n = frame.norm(u: (location.x - rect.minX) / rect.width, v: (location.y - rect.minY) / rect.height)
        let at: JSONValue = ["x": .double(n.x), "y": .double(n.y)]
        model.editor.picking = nil
        switch pick {
        case .whiteBalance: model.perform("develop.wbPick", at)
        case .pointColor: model.perform("pointColor.pick", at)
        }
    }

    /// The render this screen wants: the canvas in pixels, and once zoomed in (when the gesture
    /// has settled), the part of the photo on screen, which comes as a sharp window.
    private var request: ViewRequest? {
        guard canvas.width > margin * 2, canvas.height > margin * 2 else { return nil }
        var w = (canvas.width - 2 * margin) * displayScale
        var h = (canvas.height - 2 * margin) * displayScale
        let long = max(w, h)
        if long > maxEdge {
            w *= maxEdge / long
            h *= maxEdge / long
        }
        return ViewRequest(photo: photo, width: Int(max(w, 1)), height: Int(max(h, 1)), revision: model.state.catalogRevision, dragging: model.state.interaction != nil,
                           crop: model.editor.tool != .crop, overlay: model.viewOverlay, proof: model.viewProof, zoom: zoomRequest, hdr: model.hdrDisplay)
    }

    /// The settled zoom's view (`nil` at fit): the photo's long edge as drawn and what is on screen.
    private var zoomRequest: ZoomRequest? {
        guard zoomAtStart > 1.01, let rect = zoomedRect(model.images.view(for: photo), zoom: zoomAtStart, pan: panAtStart), rect.width > 0, rect.height > 0 else { return nil }
        let vis = rect.intersection(CGRect(origin: .zero, size: canvas))
        guard !vis.isNull else { return nil }
        let f = { (v: CGFloat, lo: CGFloat, size: CGFloat) in Int(((v - lo) / size * 1000).rounded()) }
        return ZoomRequest(drawn: Int(max(rect.width, rect.height) * displayScale),
                           visible: [f(vis.minX, rect.minX, rect.width), f(vis.minY, rect.minY, rect.height), f(vis.maxX, rect.minX, rect.width), f(vis.maxY, rect.minY, rect.height)])
    }

    private var magnify: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                zoom = min(max(zoomAtStart * value.magnification, 1), 8)
                pan = clamped(pan)
                zoomed = zoom > 1.01
            }
            .onEnded { _ in
                zoomAtStart = zoom
                if zoom <= 1.01 {
                    withAnimation(.snappy) {
                        zoom = 1
                        pan = .zero
                    }
                }
                panAtStart = pan
                zoomed = zoom > 1.01
            }
    }

    private var drag: some Gesture {
        DragGesture(minimumDistance: zoomed ? 4 : 10_000)
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

/// The file name and date, or the exposure, over the photo's corner (Settings ▸ Interface).
private struct InfoOverlay: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64

    var body: some View {
        if model.prefs.infoOverlay != .off, let summary = model.library.index(of: photo).flatMap({ model.library.loaded(at: $0) }) {
            VStack(alignment: .leading, spacing: 2) {
                Text(summary.fileName).font(.subheadline.weight(.semibold))
                if model.prefs.infoOverlay == .file {
                    Text("\(summary.captured.map(Self.date) ?? "") · \(summary.width) × \(summary.height)").font(.caption)
                } else {
                    Text(summary.camera ?? "").font(.caption)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .glassEffect(.regular, in: .rect(cornerRadius: 14))
            .padding(.horizontal, 30)
            .padding(.top, 30)
        }
    }

    static func date(_ iso: String) -> String {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withFullDate, .withTime, .withColonSeparatorInTime]
        guard let d = f.date(from: String(iso.prefix(19))) else { return String(iso.prefix(10)) }
        return d.formatted(date: .abbreviated, time: .shortened)
    }
}

/// The tools under a photo, in the bottom bar: icons, the open one switched on (a tap on it closes
/// it).
struct ToolBar: ToolbarContent {
    let model: AppModel

    var body: some ToolbarContent {
        let editor = model.editor
        ToolbarSpacer(.flexible, placement: .bottomBar)
        ToolbarItemGroup(placement: .bottomBar) {
            ForEach(Tool.bar) { tool in
                Toggle(isOn: Binding(get: { editor.tool == tool }, set: { on in
                    withAnimation(.snappy) { editor.tool = on ? tool : nil }
                    if on, tool == .crop || tool == .masking || tool == .remove {
                        model.perform("develop.cancelInteraction")
                    }
                })) {
                    Label(LocalizedStringKey(tool.label), systemImage: tool.symbol)
                }
                .disabled(model.state.active == nil)
            }
        }
        ToolbarSpacer(.flexible, placement: .bottomBar)
    }
}

/// The Edit tool's groups: Auto, then Profile, Light, Color… (one open at a time).
struct GroupBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let current = EditGroup(rawValue: model.prefs.editGroup) ?? .light
        GroupChips(
            items: EditGroup.allCases.map { .init(id: $0.rawValue, label: $0.label, symbol: $0.symbol) },
            selected: current.rawValue,
            auto: {
                Task {
                    if await model.run("develop.auto") != nil { model.show(String(localized: "Auto settings applied")) }
                }
            },
            choose: { model.prefs.editGroup = $0 }
        )
        .haptic(.selection, trigger: model.prefs.editGroup)
    }
}

/// A row of groups as glass chips (a symbol and the name), the open one prominent, scrolling
/// sideways when they don't fit; Auto first when given.
struct GroupChips: View {
    struct Item: Identifiable {
        let id: String
        let label: String
        let symbol: String
    }

    let items: [Item]
    let selected: String?
    var enabled = true
    var auto: (() -> Void)? = nil
    let choose: (String) -> Void

    var body: some View {
        ScrollView(.horizontal) {
            GlassEffectContainer {
                HStack(spacing: 6) {
                    if let auto {
                        Button(action: auto) {
                            Label("Auto", systemImage: "wand.and.stars")
                        }
                        .buttonStyle(.glass)
                    }
                    ForEach(items) { item in
                        chip(item, on: enabled && item.id == selected)
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
        }
        .scrollIndicators(.hidden)
        .scrollClipDisabled()
        .font(.subheadline.weight(.medium))
        .disabled(!enabled)
        .dynamicTypeSize(...DynamicTypeSize.xxxLarge)
    }

    @ViewBuilder private func chip(_ item: Item, on: Bool) -> some View {
        let button = Button { choose(item.id) } label: {
            Label(LocalizedStringKey(item.label), systemImage: item.symbol)
        }
        .accessibilityAddTraits(on ? .isSelected : [])
        if on {
            button.buttonStyle(.glassProminent)
        } else {
            button.buttonStyle(.glass)
        }
    }
}

/// The open tool's panel between the photo and the tools: a grabber to make it taller or shorter,
/// then the tool's controls.
struct ToolPanel: View {
    @Environment(AppModel.self) private var model
    let tool: Tool
    @State private var dragFrom: CGFloat?
    @State private var dragHeight: CGFloat?

    var body: some View {
        let editor = model.editor
        Group {
            if dials {
                // dials take the height they need: nothing to resize or scroll
                ToolPanelContent(tool: tool)
            } else {
                VStack(spacing: 0) {
                    Capsule()
                        .fill(Color.secondary.opacity(0.5))
                        .frame(width: 36, height: 5)
                        .frame(maxWidth: .infinity, minHeight: 20)
                        .contentShape(Rectangle())
                        .onTapGesture { withAnimation(.snappy) { editor.detent = (editor.detent + 1) % Self.stops.count } }
                        .gesture(grabberDrag)
                        .accessibilityLabel("Resize panel")
                        .accessibilityAction { editor.detent = (editor.detent + 1) % Self.stops.count }
                    ScrollView { ToolPanelContent(tool: tool) }
                        .scrollIndicators(.automatic)
                }
                .frame(height: dragHeight ?? Self.stops[min(editor.detent, Self.stops.count - 1)])
            }
        }
        // a card floating over the canvas, as a sheet is in iOS 26 (opaque: glass would mirror the
        // group chips above it)
        .clipShape(.rect(cornerRadius: 28))
        .background(Color(uiColor: .secondarySystemBackground), in: .rect(cornerRadius: 28))
        .padding(.horizontal, 8)
        .padding(.bottom, 4)
        .transition(.move(edge: .bottom).combined(with: .opacity))
    }

    /// The Edit tool's group as dials (the profiles keep their grid).
    private var dials: Bool {
        tool == .edit && model.prefs.editGroup != EditGroup.profile.rawValue && model.prefs.dials(column: false)
    }

    /// The panel's heights: a few rows, more, most of the screen.
    static let stops: [CGFloat] = [140, 250, 420]

    private var grabberDrag: some Gesture {
        // (on screen, not in the grabber: it moves with the panel's height, and a translation
        // measured from a moving view makes the height jump back and forth)
        DragGesture(coordinateSpace: .global)
            .onChanged { g in
                let from = dragFrom ?? Self.stops[min(model.editor.detent, Self.stops.count - 1)]
                dragFrom = from
                dragHeight = min(max(from - g.translation.height, 80), 560)
            }
            .onEnded { g in
                let h = dragHeight ?? 0
                dragFrom = nil
                withAnimation(.snappy) {
                    dragHeight = nil
                    if h < Self.stops[0] - 50 {
                        model.editor.tool = nil
                    } else {
                        model.editor.detent = Self.stops.enumerated().min { abs($0.element - h) < abs($1.element - h) }?.offset ?? 1
                    }
                }
            }
    }
}

/// A tool's controls, in the panel under the photo or the side panel.
struct ToolPanelContent: View {
    let tool: Tool

    var body: some View {
        Group {
            switch tool {
            case .edit: EditGroupPanel()
            case .presets: PresetsPanel()
            case .info: InfoPanel()
            case .crop, .masking, .remove, .redEye: EmptyView()
            }
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 12)
    }
}

/// Undo and Redo side by side (the menu stays open to step back and forth), then Reset, Versions,
/// History.
struct UndoMenu: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Menu {
            ControlGroup {
                Button("Undo", systemImage: "arrow.uturn.backward") { model.undo() }
                    .disabled(model.state.undo == nil)
                    .menuActionDismissBehavior(.disabled)
                Button("Redo", systemImage: "arrow.uturn.forward") { model.redo() }
                    .disabled(model.state.redo == nil)
                    .menuActionDismissBehavior(.disabled)
            }
            Section {
                Button("Reset", systemImage: "arrow.counterclockwise") { model.perform("develop.reset"); model.buzz(.light) }
                Button("Versions", systemImage: "square.stack") { model.sheet = .history(versions: true) }
                Button("History", systemImage: "clock.arrow.circlepath") { model.sheet = .history(versions: false) }
            }
        } label: {
            Label("Undo", systemImage: "arrow.uturn.backward")
        } primaryAction: {
            model.undo()
        }
        .disabled(model.state.active == nil)
    }
}

/// A photo's "…": rating, flag and label, copying edits, versions, history, keywords, Before /
/// After, deleting.
struct PhotoMoreMenu: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let ids = model.state.active.map { [$0] } ?? []
        Menu {
            Menu("Rating", systemImage: "star") { RatingItems(ids: ids) }
            Menu("Flag", systemImage: "flag") { FlagItems(ids: ids) }
            Menu("Color Label", systemImage: "circle.lefthalf.filled") { LabelItems(ids: ids) }
            Section {
                Button("Copy Edit Settings", systemImage: "doc.on.doc") {
                    Task { if await model.run("develop.copy") != nil { model.show(String(localized: "Edit settings copied")) } }
                }
                Button("Copy Settings…", systemImage: "checklist") { model.sheet = .copySettings }
                Button("Paste Edit Settings", systemImage: "doc.on.clipboard") { model.perform("develop.paste") }
                    .disabled(model.library.state?.clipboard != true)
                Button("Reset All Edits", systemImage: "arrow.counterclockwise") { model.perform("develop.reset"); model.buzz(.light) }
                if model.superResAvailable {
                    Button("Super Resolution…", systemImage: "arrow.up.left.and.arrow.down.right") { model.sheet = .superRes }
                }
            }
            Section {
                Toggle("Faces", systemImage: "person.crop.square", isOn: Binding(get: { model.prefs.faceBoxes }, set: { model.prefs.faceBoxes = $0 }))
                Button("Tag a Face", systemImage: "person.crop.rectangle.badge.plus") {
                    model.prefs.faceBoxes = true
                    model.editor.tagFace = true
                    if model.editor.tool?.hasScreen == true { model.editor.tool = .edit }
                }
                Toggle("Before / After", systemImage: "rectangle.split.2x1", isOn: Binding(get: { model.editor.beforeAfter }, set: { model.editor.beforeAfter = $0 }))
                Picker("Before / After Layout", systemImage: "rectangle.split.2x1", selection: Binding(get: { model.prefs.beforeAfterMode }, set: {
                    model.prefs.beforeAfterMode = $0
                    model.editor.beforeAfter = true
                })) {
                    Text("Automatic").tag("auto")
                    Text("Left / Right").tag("sideBySide")
                    Text("Top / Bottom").tag("topBottom")
                    Text("Split Left / Right").tag("split")
                    Text("Split Top / Bottom").tag("splitTopBottom")
                }
                .pickerStyle(.menu)
                Toggle("Soft Proofing", systemImage: "printer", isOn: Binding(get: { model.editor.softProof }, set: { model.editor.softProof = $0 }))
                if model.editor.softProof {
                    Picker("Proof As", systemImage: "paintpalette", selection: Binding(get: { model.editor.proofSpace }, set: { model.editor.proofSpace = $0 })) {
                        ForEach(SoftProof.spaces, id: \.0) { Text(LocalizedStringKey($0.1)).tag($0.0) }
                    }
                    .pickerStyle(.menu)
                    Toggle("Destination Gamut Warning", systemImage: "exclamationmark.triangle", isOn: Binding(get: { model.editor.proofWarning }, set: { model.editor.proofWarning = $0 }))
                }
                Toggle("Histogram", systemImage: "chart.bar.xaxis", isOn: Binding(get: { model.prefs.showHistogram }, set: { model.prefs.showHistogram = $0 }))
                Button("Red Eye", systemImage: "eye") {
                    model.perform("develop.cancelInteraction")
                    model.editor.tool = .redEye
                }
            }
            Section {
                if UIApplication.shared.supportsMultipleScenes {
                    Button("Second Window", systemImage: "macwindow.on.rectangle") { model.secondWindowAsked = true }
                }
                Button("Set as Reference", systemImage: "pin") {
                    model.reference = model.state.active
                    model.editor.referenceView = true
                }
                .disabled(model.state.active == nil)
                Toggle("Reference View", systemImage: "rectangle.lefthalf.inset.filled", isOn: Binding(get: { model.editor.referenceView }, set: { model.editor.referenceView = $0 }))
                    .disabled(model.reference == nil)
                Toggle("Filmstrip", systemImage: "film", isOn: Binding(get: { model.prefs.filmstrip }, set: { model.prefs.filmstrip = $0 }))
                Toggle("Navigator", systemImage: "scope", isOn: Binding(get: { model.prefs.navigator }, set: { model.prefs.navigator = $0 }))
                Button("Lights Out", systemImage: "lightbulb.slash") { model.cover = .lightsOut }
                Button("Slideshow", systemImage: "play.rectangle") { model.cover = .slideshow }
            }
            Section {
                Button("Versions", systemImage: "square.stack") { model.sheet = .history(versions: true) }
                Button("History", systemImage: "clock.arrow.circlepath") { model.sheet = .history(versions: false) }
                Button("Keywords", systemImage: "tag") { model.sheet = .keywords }
            }
            PhotoManageItems(ids: ids)
            Section {
                Button("Delete Photo", systemImage: "trash", role: .destructive) { model.confirmDelete = ids }
                Button("All Commands…", systemImage: "command") { model.path.append(.commands) }
            }
        } label: {
            Label("More", systemImage: "ellipsis")
        }
        .disabled(ids.isEmpty)
    }
}

/// Crop, Masking and Remove have a screen of their own; until then, their bar.
struct ToolScreenBar: View {
    @Environment(AppModel.self) private var model
    let tool: Tool

    var body: some View {
        switch tool {
        case .crop: CropTools()
        case .masking: MaskingTools()
        case .remove: RemoveTools()
        case .redEye: RedEyeTools()
        default: EmptyView()
        }
    }
}
#endif

#if os(iOS)
/// A review swipe: which side, where it started, where it is.
nonisolated struct ReviewSwipe: Equatable {
    let stars: Bool
    let startRating: Int
    let startFlag: Int
    var rating: Int?
    var flag: Int?
}

/// Review mode's readout: the stars at the bottom left, the flag at the bottom right.
struct ReviewHUD: View {
    let rating: Int
    let flag: Int

    var body: some View {
        HStack {
            HStack(spacing: 6) {
                ForEach(1...5, id: \.self) { i in
                    Image(systemName: i <= rating ? "star.fill" : "star")
                        .font(.title3)
                        .foregroundStyle(i <= rating ? Color.yellow : .white.opacity(0.7))
                }
            }
            .padding(.horizontal, 14)
            .frame(height: 44)
            .glassEffect(.regular, in: .capsule)
            .accessibilityElement()
            .accessibilityLabel("\(rating) stars")
            Spacer()
            Image(systemName: flag == 1 ? "flag.fill" : flag == -1 ? "flag.slash.fill" : "flag")
                .font(.title3)
                .foregroundStyle(flag == -1 ? Color.red : .white)
                .frame(width: 44, height: 44)
                .glassEffect(.regular, in: .circle)
                .accessibilityLabel(flag == 1 ? "Picked" : flag == -1 ? "Rejected" : "Unflagged")
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 24)
        .animation(.snappy, value: rating)
        .animation(.snappy, value: flag)
    }
}
#endif

#if os(iOS)
/// Before / After: the photo as it was and as it is, side by side or one above the other (on its
/// own: side by side on a wide screen), or split by a line to drag across (left / right or top /
/// bottom), as egui's Before/After modes.
struct BeforeAfterView: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    let wide: Bool

    var body: some View {
        let before = model.images.before(for: photo)
        let after = model.images.view(for: photo)
        switch mode {
        case "sideBySide":
            HStack(spacing: 12) { half(before, "Before"); half(after, "After") }
        case "topBottom":
            VStack(spacing: 12) { half(before, "Before"); half(after, "After") }
        case "split", "splitTopBottom":
            SplitBeforeAfter(before: before, after: after, across: mode == "split")
        default:
            let layout = wide ? AnyLayout(HStackLayout(spacing: 12)) : AnyLayout(VStackLayout(spacing: 12))
            layout { half(before, "Before"); half(after, "After") }
        }
    }

    private var mode: String { model.prefs.beforeAfterMode }

    private func half(_ picture: ImageStore.Picture?, _ label: LocalizedStringKey) -> some View {
        Color.clear
            .overlay {
                if let picture {
                    Image(decorative: picture.image, scale: 1).resizable().scaledToFit()
                } else {
                    ProgressView()
                }
            }
            .overlay(alignment: .bottomLeading) { tag(label) }
            .accessibilityElement(children: .combine)
    }

    private func tag(_ label: LocalizedStringKey) -> some View {
        Text(label)
            .font(.caption.weight(.semibold))
            .padding(.horizontal, 10).padding(.vertical, 4)
            .glassEffect(.regular, in: .capsule)
            .padding(8)
    }
}

/// Before / After split: After, with Before over the part on one side of a line to drag (Shapes
/// over the photo's own frame, no GeometryReader; a view of its own keeps its type small).
private struct SplitBeforeAfter: View {
    let before: ImageStore.Picture?
    let after: ImageStore.Picture?
    let across: Bool
    @State private var split: CGFloat = 0.5
    @State private var size: CGSize = .zero

    var body: some View {
        if let after {
            photo(after)
                .overlay { if let before { photo(before).mask(SplitPart(fraction: split, across: across)) } }
                .overlay { line }
                .onGeometryChange(for: CGSize.self) { $0.size } action: { size = $0 }
                .contentShape(Rectangle())
                .gesture(drag)
                .overlay(alignment: across ? .bottomLeading : .topLeading) { tag("Before") }
                .overlay(alignment: .bottomTrailing) { tag("After") }
                .accessibilityElement()
                .accessibilityLabel("Before and After")
                .accessibilityValue("\(Int(split * 100)) %")
                .accessibilityAdjustableAction { d in split = min(max(split + (d == .increment ? 0.1 : -0.1), 0), 1) }
        } else {
            ProgressView()
        }
    }

    private func photo(_ p: ImageStore.Picture) -> some View {
        Image(decorative: p.image, scale: 1).resizable().scaledToFit()
    }

    private var line: some View {
        SplitLine(fraction: split, across: across).stroke(.white, lineWidth: 2).shadow(color: .black.opacity(0.5), radius: 2)
    }

    private func tag(_ label: LocalizedStringKey) -> some View {
        Text(label)
            .font(.caption.weight(.semibold))
            .padding(.horizontal, 10).padding(.vertical, 4)
            .glassEffect(.regular, in: .capsule)
            .padding(8)
    }

    private var drag: some Gesture {
        DragGesture(minimumDistance: 0).onChanged { v in
            let f = across ? v.location.x / max(size.width, 1) : v.location.y / max(size.height, 1)
            split = min(max(f, 0), 1)
        }
    }
}

/// The part of the photo before the split line (Before's mask).
private nonisolated struct SplitPart: Shape {
    var fraction: CGFloat
    let across: Bool

    func path(in rect: CGRect) -> Path {
        Path(across ? CGRect(x: rect.minX, y: rect.minY, width: rect.width * fraction, height: rect.height)
                    : CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: rect.height * fraction))
    }
}

/// The split line, across the photo.
private nonisolated struct SplitLine: Shape {
    var fraction: CGFloat
    let across: Bool

    func path(in rect: CGRect) -> Path {
        var p = Path()
        if across {
            let x = rect.minX + rect.width * fraction
            p.move(to: CGPoint(x: x, y: rect.minY))
            p.addLine(to: CGPoint(x: x, y: rect.maxY))
        } else {
            let y = rect.minY + rect.height * fraction
            p.move(to: CGPoint(x: rect.minX, y: y))
            p.addLine(to: CGPoint(x: rect.maxX, y: y))
        }
        return p
    }
}

/// The loupe render's histogram: luminance in grey, red, green and blue over it, the clipping
/// triangles at its ends (lit when pixels are clipped; a tap shows the clipping warning).
struct HistogramView: View {
    @Environment(AppModel.self) private var model
    let histogram: Histogram

    var body: some View {
        let clip = histogram.clipping ?? [0, 0]
        VStack(spacing: 2) {
            HStack {
                triangle(lit: (clip.first ?? 0) > 0.003, color: .blue, pointsLeft: true)
                Spacer()
                triangle(lit: (clip.last ?? 0) > 0.003, color: .red, pointsLeft: false)
            }
            Canvas { ctx, size in
                // scaled to the tallest bin away from the ends (a clipped end would flatten the rest)
                let peak = Double([histogram.r, histogram.g, histogram.b, histogram.luma].flatMap { $0.dropFirst().dropLast() }.max() ?? 1)
                func plot(_ bins: [UInt32]) -> Path {
                    var p = Path()
                    guard bins.count > 1, peak > 0 else { return p }
                    p.move(to: CGPoint(x: 0, y: size.height))
                    for (i, v) in bins.enumerated() {
                        let x = size.width * CGFloat(i) / CGFloat(bins.count - 1)
                        p.addLine(to: CGPoint(x: x, y: size.height * (1 - min(CGFloat(Double(v) / peak), 1))))
                    }
                    p.addLine(to: CGPoint(x: size.width, y: size.height))
                    p.closeSubpath()
                    return p
                }
                ctx.fill(plot(histogram.luma), with: .color(.gray.opacity(0.5)))
                ctx.blendMode = .plusLighter
                ctx.fill(plot(histogram.r), with: .color(.red.opacity(0.45)))
                ctx.fill(plot(histogram.g), with: .color(.green.opacity(0.45)))
                ctx.fill(plot(histogram.b), with: .color(.blue.opacity(0.45)))
            }
        }
        .frame(width: 140, height: 72)
        .padding(8)
        .glassEffect(.regular, in: .rect(cornerRadius: 12))
        .onTapGesture { model.prefs.showClipping.toggle() }
        .accessibilityElement()
        .accessibilityLabel("Histogram")
        .accessibilityValue(model.prefs.showClipping ? "Clipping shown" : "")
        .accessibilityHint("Shows or hides the clipping warning")
    }

    private func triangle(lit: Bool, color: Color, pointsLeft: Bool) -> some View {
        Image(systemName: pointsLeft ? "arrowtriangle.left.fill" : "arrowtriangle.right.fill")
            .font(.system(size: 9))
            .foregroundStyle(lit ? color : Color.secondary.opacity(0.4))
            .overlay {
                if model.prefs.showClipping { Circle().stroke(color.opacity(0.6), lineWidth: 1).frame(width: 14, height: 14) }
            }
    }
}
/// Soft proofing's spaces, as the engine names them and as people do.
enum SoftProof {
    static let spaces: [(String, String)] = [
        ("srgb", "sRGB"), ("displayP3", "Display P3"), ("adobeRgb", "Adobe RGB"), ("proPhoto", "ProPhoto RGB"), ("rec2020", "Rec. 2020"),
    ]

    static func label(_ space: String) -> String {
        spaces.first { $0.0 == space }?.1 ?? space
    }
}
#endif
