import LightCraftCore
import SwiftUI

/// The Remove tool on the photo: paint over something to take it away (a trail at the brush's
/// size), the spots as pins (tap one for its menu, drag it to move it). The engine picks the
/// source and fills the spot (`spot.add`, `spot.update`).
///
/// The selected spot shows the outline of what was painted; a Heal or Clone spot also shows its
/// source as an outline to drag. While a stroke waits for the engine it glows. A pin or a source
/// dragged moves only its outline: the engine is asked once, when the finger lifts.
struct RemoveCanvas: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    /// Where the shown photo is drawn.
    let rect: CGRect
    @State private var frame: PhotoFrame?
    @State private var trail: [CGPoint] = []
    @State private var moving: Moving?
    @State private var menu: Int?
    /// Strokes the engine is still filling (image coordinates): they stay drawn, glowing.
    @State private var pending: [Pending] = []
    /// A second finger landed during this stroke: it is dropped, the fingers move the photo.
    @State private var multi = false

    /// A pin (the spot) or a source outline being dragged: how far it is from where it was.
    private struct Moving {
        let index: Int
        let source: Bool
        var delta: CGSize
    }

    private struct Pending: Identifiable {
        let id = UUID()
        let points: [[Double]]
        let size: Double
        var done: Date?
    }

    var body: some View {
        let spots = model.editor.setting("spots").array ?? []
        ZStack {
            TimelineView(.animation(minimumInterval: nil, paused: pending.isEmpty)) { timeline in
                Canvas { ctx, _ in
                    if let i = model.state.activeSpot, spots.indices.contains(i) { drawSelected(ctx, spots[i], i) }
                    drawPending(ctx, at: timeline.date)
                    guard trail.count > 0 else { return }
                    var p = Path()
                    p.addLines(trail)
                    ctx.stroke(p, with: .color(.white.opacity(0.45)), style: StrokeStyle(lineWidth: brushRadius * 2, lineCap: .round, lineJoin: .round))
                    ctx.stroke(p, with: .color(.white), style: StrokeStyle(lineWidth: 1, lineCap: .round, lineJoin: .round))
                }
            }
            .allowsHitTesting(false)
            // a Heal or Clone spot's source: drag the outline to take it from somewhere else
            if let i = model.state.activeSpot, spots.indices.contains(i), let src = sourcePoints(spots[i], i), let r = radius(spots[i]) {
                Color.clear
                    .contentShape(StrokeGlow.path(src).strokedPath(StrokeGlow.style(r, extra: 20)))
                    .gesture(sourceDrag(i))
                    .accessibilityHidden(true)
                if let c = center(src) {
                    Circle()
                        .stroke(Color.accentColor, lineWidth: 2)
                        .frame(width: 12, height: 12)
                        .position(c)
                        .allowsHitTesting(false)
                }
            }
            ForEach(Array(spots.enumerated()), id: \.offset) { i, spot in
                if let at = pin(spot) {
                    let chosen = model.state.activeSpot == i
                    let shift = moving.flatMap { $0.index == i && !$0.source ? $0.delta : nil } ?? .zero
                    Circle()
                        .fill(chosen ? Color.accentColor : .white)
                        .stroke(.black.opacity(0.4), lineWidth: 1)
                        .frame(width: 14, height: 14)
                        .frame(width: 44, height: 44)
                        .contentShape(Circle())
                        .position(x: at.x + shift.width, y: at.y + shift.height)
                        .onTapGesture {
                            model.perform("spot.select", ["index": .int(Int64(i))])
                            menu = i
                        }
                        .gesture(pinDrag(i, spot))
                        .accessibilityLabel("Spot \(i + 1)")
                        .accessibilityAddTraits(.isButton)
                }
            }
        }
        .contentShape(Rectangle())
        .gesture(paint)
        .confirmationDialog(menu.map { String(localized: "Spot \($0 + 1)") } ?? "", isPresented: Binding(get: { menu != nil }, set: { if !$0 { menu = nil } }), titleVisibility: .visible) {
            if let i = menu {
                Button("Remove") { model.perform("spot.update", ["index": .int(Int64(i)), "mode": "remove"]) }
                Button("Heal") { model.perform("spot.update", ["index": .int(Int64(i)), "mode": "heal"]) }
                Button("Clone") { model.perform("spot.update", ["index": .int(Int64(i)), "mode": "clone"]) }
                Button("New Source") { model.perform("spot.refreshSource", ["index": .int(Int64(i))]) }
                Button("Delete Spot", role: .destructive) { model.perform("spot.delete", ["index": .int(Int64(i))]) }
            }
        }
        .haptic(.warning, trigger: menu == nil)
        .onChange(of: model.editor.fingers) { _, n in
            if n >= 2 {
                multi = true
                trail = []
                moving = nil
            } else if n == 0 {
                multi = false
            }
        }
        .task(id: model.state.catalogRevision) {
            guard let engine = model.engine else { return }
            frame = try? await engine.execute("view.frame", ["id": .int(Int64(clamping: photo))], as: PhotoFrame.self)
        }
    }

    /// The brush's radius on screen: its size is a fraction of the photo's long edge.
    private var brushRadius: CGFloat {
        CGFloat(model.editor.removeSize) * max(rect.width, rect.height)
    }

    /// A spot's radius on screen.
    private func radius(_ spot: JSONValue) -> CGFloat? {
        spot["size"].double.map { CGFloat($0) * max(rect.width, rect.height) }
    }

    /// A screen point in image coordinates.
    private func norm(_ p: CGPoint) -> (x: Double, y: Double)? {
        guard let frame, rect.width > 0, rect.height > 0 else { return nil }
        return frame.norm(u: (p.x - rect.minX) / rect.width, v: (p.y - rect.minY) / rect.height)
    }

    /// Image coordinates on screen.
    private func screen(_ x: Double, _ y: Double) -> CGPoint? {
        guard let frame else { return nil }
        let s = frame.shown(x: x, y: y)
        return CGPoint(x: rect.minX + s.u * rect.width, y: rect.minY + s.v * rect.height)
    }

    /// What was painted, in image coordinates.
    private func painted(_ spot: JSONValue) -> [(Double, Double)] {
        (spot["points"].array ?? []).compactMap { p in
            if let x = p["x"].double, let y = p["y"].double { return (x, y) }
            if let a = p.array, a.count == 2, let x = a[0].double, let y = a[1].double { return (x, y) }
            return nil
        }
    }

    /// Where a spot's pin goes: the middle of what was painted.
    private func pin(_ spot: JSONValue) -> CGPoint? {
        let pts = painted(spot)
        guard !pts.isEmpty else { return nil }
        let x = pts.map(\.0).reduce(0, +) / Double(pts.count)
        let y = pts.map(\.1).reduce(0, +) / Double(pts.count)
        return screen(x, y)
    }

    private func center(_ pts: [CGPoint]) -> CGPoint? {
        guard !pts.isEmpty else { return nil }
        return CGPoint(x: pts.map(\.x).reduce(0, +) / CGFloat(pts.count), y: pts.map(\.y).reduce(0, +) / CGFloat(pts.count))
    }

    /// A Heal or Clone spot's source offset from what was painted (image coordinates).
    private func sourceOffset(_ spot: JSONValue) -> (Double, Double)? {
        guard ["heal", "clone"].contains(spot["mode"].string ?? "") else { return nil }
        let o = spot["source_offset"]
        if let x = o["x"].double, let y = o["y"].double { return (x, y) }
        if let a = o.array, a.count == 2, let x = a[0].double, let y = a[1].double { return (x, y) }
        return nil
    }

    /// The spot's source on screen (with its drag, and the spot's if that is being dragged).
    private func sourcePoints(_ spot: JSONValue, _ index: Int) -> [CGPoint]? {
        guard let off = sourceOffset(spot) else { return nil }
        var shift = CGSize.zero
        if let m = moving, m.index == index { shift = m.delta }
        let pts = painted(spot).compactMap { screen($0.0 + off.0, $0.1 + off.1) }
        guard !pts.isEmpty else { return nil }
        return pts.map { CGPoint(x: $0.x + shift.width, y: $0.y + shift.height) }
    }

    // MARK: drawing

    /// The selected spot: the outline of what was painted (following its pin while that moves), and
    /// of its source.
    private func drawSelected(_ ctx: GraphicsContext, _ spot: JSONValue, _ index: Int) {
        guard let r = radius(spot) else { return }
        let shift = moving.flatMap { $0.index == index && !$0.source ? $0.delta : nil } ?? .zero
        let pts = painted(spot).compactMap { screen($0.0, $0.1) }
        StrokeGlow.outline(ctx, StrokeGlow.path(pts, offset: shift), radius: r)
        if let src = sourcePoints(spot, index) {
            StrokeGlow.outline(ctx, StrokeGlow.path(src), radius: r, color: .accentColor)
        }
    }

    private func drawPending(_ ctx: GraphicsContext, at date: Date) {
        let t = date.timeIntervalSinceReferenceDate
        for stroke in pending {
            let path = StrokeGlow.path(stroke.points.compactMap { $0.count == 2 ? screen($0[0], $0[1]) : nil })
            let r = CGFloat(stroke.size) * max(rect.width, rect.height)
            let alpha = stroke.done.map { max(0, 1 - date.timeIntervalSince($0) / 0.35) } ?? 1
            StrokeGlow.glow(ctx, path, radius: r, colors: StrokeGlow.shimmer, time: t, alpha: alpha)
        }
    }

    /// Show `stroke` glowing until `work` is done, then let it fade.
    private func waiting(_ stroke: Pending, _ work: () async -> Void) async {
        pending.append(stroke)
        await work()
        if let i = pending.firstIndex(where: { $0.id == stroke.id }) { pending[i].done = .now }
        try? await Task.sleep(for: .milliseconds(450))
        pending.removeAll { $0.id == stroke.id }
    }

    // MARK: gestures

    private var paint: some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { g in
                if model.editor.fingers >= 2 { multi = true }
                if multi {
                    trail = []
                    return
                }
                if trail.last.map({ hypot($0.x - g.location.x, $0.y - g.location.y) > 2 }) ?? true {
                    trail.append(g.location)
                }
            }
            .onEnded { _ in
                let image = trail.compactMap { p in norm(p).map { [$0.x, $0.y] } }
                let pts: [JSONValue] = image.map { [.double($0[0]), .double($0[1])] }
                trail = []
                guard !pts.isEmpty, !multi, model.editor.fingers < 2 else { return }
                let e = model.editor
                // Generative AI: the MI-GAN model fills it, when it's here (else texture from around it, and a word why)
                let ai = e.removeMode == "remove" && model.prefs.removeAI
                if ai && !e.eraseReady {
                    model.show(String(localized: "AI Remove needs its model: download it in the Remove panel. This stroke was filled from around it."))
                } else if ai {
                    model.show(String(localized: "Removing with AI…"))
                }
                let params: JSONValue = ["mode": .string(e.removeMode), "points": .array(pts), "size": .double(e.removeSize), "feather": .double(e.removeFeather),
                                         "opacity": .double(e.removeOpacity), "ai": .bool(ai && e.eraseReady)]
                Task {
                    await waiting(Pending(points: image, size: e.removeSize)) { await model.run("spot.add", params) }
                }
            }
    }

    /// Moving a spot: its outline follows the finger, and the engine hears of it once, at the end
    /// (an AI fill is painted again for the new place).
    private func pinDrag(_ i: Int, _ spot: JSONValue) -> some Gesture {
        DragGesture(minimumDistance: 4)
            .onChanged { g in
                guard !multi else { return }
                if moving == nil {
                    model.buzz(.medium)
                    model.perform("spot.select", ["index": .int(Int64(i))])
                }
                moving = Moving(index: i, source: false, delta: g.translation)
            }
            .onEnded { g in
                guard moving != nil, let a = norm(g.startLocation), let b = norm(g.location) else {
                    moving = nil
                    return
                }
                let d: JSONValue = [.double(b.x - a.x), .double(b.y - a.y)]
                let refill = spot["fill"] != .null && model.editor.eraseReady
                let moved = Pending(points: painted(spot).map { [$0.0 + b.x - a.x, $0.1 + b.y - a.y] }, size: spot["size"].double ?? model.editor.removeSize)
                model.buzz(.light)
                Task {
                    model.engine?.send("engine.execute", ["command": "develop.beginInteraction", "params": ["label": "Edit Spot"]])
                    await model.run("spot.update", ["index": .int(Int64(i)), "move": d])
                    moving = nil
                    if refill {
                        await waiting(moved) { await model.run("spot.erase", ["index": .int(Int64(i))]) }
                    }
                    model.engine?.send("engine.execute", ["command": "develop.endInteraction"])
                }
            }
    }

    /// Moving a Heal or Clone spot's source: its outline follows the finger, the engine is asked
    /// when it lifts.
    private func sourceDrag(_ i: Int) -> some Gesture {
        DragGesture(minimumDistance: 2)
            .onChanged { g in
                guard !multi else { return }
                if moving == nil { model.buzz(.medium) }
                moving = Moving(index: i, source: true, delta: g.translation)
            }
            .onEnded { g in
                guard moving != nil, let a = norm(g.startLocation), let b = norm(g.location) else {
                    moving = nil
                    return
                }
                model.buzz(.light)
                Task {
                    await model.run("spot.update", ["index": .int(Int64(i)), "moveSource": [.double(b.x - a.x), .double(b.y - a.y)]])
                    moving = nil
                }
            }
    }
}
