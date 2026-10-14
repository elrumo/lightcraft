#if os(iOS)
import LightCraftCore
import SwiftUI

/// A photo, as Lightroom's mobile app shows it: back, undo, share and "…" above it; swipe sideways
/// for the next; the tools under it (Presets, Crop, Edit, Masking, Remove, Info), the Edit tool's
/// groups in a row of their own and the open tool's panel between the photo and the tools. A tap
/// on the photo hides the bars (review mode).
struct PhotoScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(\.verticalSizeClass) private var verticalSize
    @Environment(\.horizontalSizeClass) private var horizontalSize
    let start: UInt64
    @State private var review = false

    var body: some View {
        let editor = model.editor
        // a phone held sideways, or an iPad: the photo beside a side panel (egui's wide layout,
        // from 600 pt: 316 pt sideways, 340 pt on a tablet)
        let sideways = verticalSize == .compact || horizontalSize == .regular
        Group {
            if sideways, editor.tool != nil {
                HStack(spacing: 0) {
                    pager
                    if !review { SidePanel(width: verticalSize == .compact ? 316 : 340).transition(.move(edge: .trailing)) }
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
                            ToolBar()
                        }
                    }
                }
            }
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(review ? .hidden : .visible, for: .navigationBar)
        .statusBarHidden(review)
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                UndoMenu()
                if editor.tool == .masking {
                    MaskOverlayButton()
                    MaskMoreMenu()
                } else {
                    Button("Share", systemImage: "square.and.arrow.up") {
                        if let id = model.state.active { model.sheet = .export([id]) }
                    }
                    PhotoMoreMenu()
                }
            }
        }
        .navigationBarTitleDisplayMode(.inline)
        .animation(.snappy, value: review)
        .animation(.snappy, value: editor.tool)
        .onAppear {
            if editor.tool == nil { editor.tool = .edit }
            Task { await editor.reload(model) }
        }
        .onDisappear {
            if model.path.isEmpty { editor.tool = nil }
        }
    }

    private var pager: some View {
        PhotoPager(start: start, review: $review)
            .overlay(alignment: .topTrailing) {
                if model.editor.tool == .masking, !review {
                    MaskColumn().padding(10).transition(.move(edge: .trailing).combined(with: .opacity))
                }
            }
    }
}

/// Held sideways, the tools in a column at the right: a tool's own screen (Crop, Masking, Remove,
/// Red Eye), or the Edit groups, the open tool's controls and the tool bar.
private struct SidePanel: View {
    @Environment(AppModel.self) private var model
    let width: CGFloat

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
                ToolBar()
            }
        }
        .frame(width: width)
        .background(Color(uiColor: .secondarySystemBackground))
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
                    Task { await model.select(photo.id) }
                    var near: [UInt64] = []
                    if let next = library.photo(at: i + 1, model.engine) { near.append(next.id) }
                    if let prev = library.photo(at: i - 1, model.engine) { near.append(prev.id) }
                    model.prefetch(near)
                }
                .haptic(.impact(weight: .light), trigger: position)
                .onChange(of: model.state.active) { _, active in
                    // the engine moved on (Auto Advance, next / previous): follow it
                    guard let active, let i = library.index(of: active), i != position else { return }
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
    let zoom: ZoomRequest?
}

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
                Image(decorative: picture.image, scale: 1)
                    .resizable()
                    .scaledToFit()
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
                    Image(decorative: region.image, scale: 1)
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
            showBefore = pressing && model.editor.tool?.hasScreen != true
            if showBefore { model.requestBefore(photo) }
        }
        .haptic(trigger: showBefore) { _, new in new ? .impact(weight: .medium) : nil }
        .onGeometryChange(for: CGSize.self) { $0.size } action: { canvas = $0 }
        .task(id: request) {
            guard let r = request, !r.dragging else { return }
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
            review.toggle()
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
                           crop: model.editor.tool != .crop, overlay: model.viewOverlay, zoom: zoomRequest)
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
            .foregroundStyle(.white)
            .shadow(color: .black.opacity(0.6), radius: 2)
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

/// The tools under a photo, in a glass capsule; the open one tinted.
struct ToolBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let editor = model.editor
        GlassEffectContainer {
            HStack(spacing: 0) {
                ForEach(Tool.bar) { tool in
                    Button {
                        withAnimation(.snappy) { editor.tool = editor.tool == tool ? nil : tool }
                        if tool == .crop || tool == .masking || tool == .remove {
                            model.perform("develop.cancelInteraction")
                        }
                    } label: {
                        VStack(spacing: 3) {
                            Image(systemName: tool.symbol).font(.system(size: 20))
                            // a long translation shrinks to one line rather than pushing the bar up
                            Text(LocalizedStringKey(tool.label)).font(.system(size: 10, weight: .semibold))
                                .lineLimit(1).minimumScaleFactor(0.6)
                        }
                        .foregroundStyle(editor.tool == tool ? Color.accentColor : Color.primary)
                        .frame(maxWidth: .infinity, minHeight: 52)
                        .background {
                            if editor.tool == tool {
                                Capsule().fill(Color.accentColor.opacity(0.15)).padding(.horizontal, 2).padding(.vertical, 4)
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(editor.tool == tool ? .isSelected : [])
                    .disabled(model.state.active == nil)
                }
            }
            .padding(.horizontal, 4)
            .glassEffect(.regular, in: .capsule)
        }
        .frame(maxWidth: 480)
        .padding(.horizontal, 14)
        .padding(.vertical, 4)
        .haptic(.selection, trigger: editor.tool)
    }
}

/// The Edit tool's groups: Auto, then Profile, Light, Color… (one open at a time).
struct GroupBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let current = EditGroup(rawValue: model.prefs.editGroup) ?? .light
        ScrollView(.horizontal) {
            HStack(spacing: 4) {
                Button {
                    Task {
                        if await model.run("develop.auto") != nil { model.show(String(localized: "Auto settings applied")) }
                    }
                } label: {
                    groupLabel("wand.and.stars", "Auto", on: false)
                }
                Divider().frame(height: 32)
                ForEach(EditGroup.allCases) { group in
                    Button {
                        model.prefs.editGroup = group.rawValue
                    } label: {
                        groupLabel(group.symbol, group.label, on: group == current)
                    }
                    .accessibilityAddTraits(group == current ? .isSelected : [])
                }
            }
            .padding(.horizontal, 10)
        }
        .scrollIndicators(.hidden)
        .frame(height: 60)
        .buttonStyle(.plain)
        .haptic(.selection, trigger: model.prefs.editGroup)
    }

    private func groupLabel(_ symbol: String, _ label: String, on: Bool) -> some View {
        VStack(spacing: 3) {
            Image(systemName: symbol).font(.system(size: 18))
            Text(LocalizedStringKey(label)).font(.system(size: 11))
        }
        .foregroundStyle(on ? Color.accentColor : Color.primary)
        .frame(width: 64, height: 52)
        .background {
            if on { Capsule().fill(.thinMaterial) }
        }
        .contentShape(Rectangle())
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
        .background(Color(uiColor: .secondarySystemBackground), in: .rect(topLeadingRadius: 22, topTrailingRadius: 22))
        .transition(.move(edge: .bottom).combined(with: .opacity))
    }

    /// The panel's heights: a few rows, more, most of the screen.
    static let stops: [CGFloat] = [140, 250, 420]

    private var grabberDrag: some Gesture {
        DragGesture()
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
                Toggle("Histogram", systemImage: "chart.bar.xaxis", isOn: Binding(get: { model.prefs.showHistogram }, set: { model.prefs.showHistogram = $0 }))
                Button("Red Eye", systemImage: "eye") {
                    model.perform("develop.cancelInteraction")
                    model.editor.tool = .redEye
                }
            }
            Section {
                Button("Versions", systemImage: "square.stack") { model.sheet = .history(versions: true) }
                Button("History", systemImage: "clock.arrow.circlepath") { model.sheet = .history(versions: false) }
                Button("Keywords", systemImage: "tag") { model.sheet = .keywords }
            }
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
                        .font(.system(size: 20))
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
                .font(.system(size: 20))
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
/// Before / After: the photo as it was and as it is, side by side on a wide screen, one above the
/// other on a tall one.
struct BeforeAfterView: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    let wide: Bool

    var body: some View {
        let layout = wide ? AnyLayout(HStackLayout(spacing: 12)) : AnyLayout(VStackLayout(spacing: 12))
        layout {
            half(model.images.before(for: photo), "Before")
            half(model.images.view(for: photo), "After")
        }
    }

    private func half(_ picture: ImageStore.Picture?, _ label: LocalizedStringKey) -> some View {
        Color.clear
            .overlay {
                if let picture {
                    Image(decorative: picture.image, scale: 1).resizable().scaledToFit()
                } else {
                    ProgressView()
                }
            }
            .overlay(alignment: .bottomLeading) {
                Text(label)
                    .font(.caption.weight(.semibold))
                    .padding(.horizontal, 10).padding(.vertical, 4)
                    .glassEffect(.regular, in: .capsule)
                    .padding(8)
            }
            .accessibilityElement(children: .combine)
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
#endif
