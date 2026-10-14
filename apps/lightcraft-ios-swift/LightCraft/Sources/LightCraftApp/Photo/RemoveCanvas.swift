#if os(iOS)
import LightCraftCore
import SwiftUI

/// The Remove tool on the photo: paint over something to take it away (a trail at the brush's
/// size), the spots as pins (tap one for its menu, drag it to move it). The engine picks the
/// source and fills the spot (`spot.add`, `spot.update`).
struct RemoveCanvas: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    /// Where the shown photo is drawn.
    let rect: CGRect
    @State private var frame: PhotoFrame?
    @State private var trail: [CGPoint] = []
    @State private var moving: (index: Int, from: CGPoint)?
    @State private var menu: Int?

    var body: some View {
        let spots = model.editor.setting("spots").array ?? []
        ZStack {
            Canvas { ctx, _ in
                guard trail.count > 0 else { return }
                var p = Path()
                p.addLines(trail)
                ctx.stroke(p, with: .color(.white.opacity(0.45)), style: StrokeStyle(lineWidth: brushRadius * 2, lineCap: .round, lineJoin: .round))
                ctx.stroke(p, with: .color(.white), style: StrokeStyle(lineWidth: 1, lineCap: .round, lineJoin: .round))
            }
            .allowsHitTesting(false)
            ForEach(Array(spots.enumerated()), id: \.offset) { i, spot in
                if let at = pin(spot) {
                    let chosen = model.state.activeSpot == i
                    Circle()
                        .fill(chosen ? Color.accentColor : .white)
                        .stroke(.black.opacity(0.4), lineWidth: 1)
                        .frame(width: 14, height: 14)
                        .frame(width: 44, height: 44)
                        .contentShape(Circle())
                        .position(at)
                        .onTapGesture {
                            model.perform("spot.select", ["index": .int(Int64(i))])
                            menu = i
                        }
                        .gesture(pinDrag(i, at))
                        .accessibilityLabel("Spot \(i + 1)")
                        .accessibilityAddTraits(.isButton)
                }
            }
        }
        .contentShape(Rectangle())
        .gesture(paint, including: tapsToSelect ? .none : .all)
        .simultaneousGesture(SpatialTapGesture().onEnded { tap in
            guard let n = norm(tap.location) else { return }
            Task { await model.removeObject(at: n) }
        }, including: tapsToSelect ? .all : .none)
        .overlay(alignment: .top) {
            if model.editor.selecting {
                ProgressView().padding(12).background(.regularMaterial, in: Capsule()).padding(.top, 12)
                    .transition(.scale.combined(with: .opacity))
            }
        }
        .animation(.snappy, value: model.editor.selecting)
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
        .task(id: model.state.catalogRevision) {
            guard let engine = model.engine else { return }
            frame = try? await engine.execute("view.frame", ["id": .int(Int64(clamping: photo))], as: PhotoFrame.self)
        }
    }

    /// Remove selects what a tap lands on (Tap to Remove) instead of painting.
    private var tapsToSelect: Bool { model.editor.removeMode == "remove" && model.editor.removeTap }

    /// The brush's radius on screen: its size is a fraction of the photo's long edge.
    private var brushRadius: CGFloat {
        CGFloat(model.editor.removeSize) * max(rect.width, rect.height)
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

    /// Where a spot's pin goes: the middle of what was painted.
    private func pin(_ spot: JSONValue) -> CGPoint? {
        let pts = (spot["points"].array ?? []).compactMap { p -> (Double, Double)? in
            if let x = p["x"].double, let y = p["y"].double { return (x, y) }
            if let a = p.array, a.count == 2, let x = a[0].double, let y = a[1].double { return (x, y) }
            return nil
        }
        guard !pts.isEmpty else { return nil }
        // (a tapped object's points are its centre, then the corners of its box)
        if spot["shape"] != .null, let centre = pts.first { return screen(centre.0, centre.1) }
        let x = pts.map(\.0).reduce(0, +) / Double(pts.count)
        let y = pts.map(\.1).reduce(0, +) / Double(pts.count)
        return screen(x, y)
    }

    private var paint: some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { g in
                if trail.last.map({ hypot($0.x - g.location.x, $0.y - g.location.y) > 2 }) ?? true {
                    trail.append(g.location)
                }
            }
            .onEnded { _ in
                let pts: [JSONValue] = trail.compactMap { p in norm(p).map { [.double($0.x), .double($0.y)] } }
                trail = []
                guard !pts.isEmpty else { return }
                let e = model.editor
                // Generative AI: the MI-GAN model fills it, when it's here (else texture from around it, and a word why)
                let ai = e.removeMode == "remove" && model.prefs.removeAI
                if ai && !e.eraseReady {
                    model.show(String(localized: "AI Remove needs its model: download it in the Remove panel. This stroke was filled from around it."))
                } else if ai {
                    model.show(String(localized: "Removing with AI…"))
                }
                model.perform("spot.add", ["mode": .string(e.removeMode), "points": .array(pts), "size": .double(e.removeSize), "feather": .double(e.removeFeather),
                                           "opacity": .double(e.removeOpacity), "ai": .bool(ai && e.eraseReady)])
            }
    }

    private func pinDrag(_ i: Int, _ at: CGPoint) -> some Gesture {
        DragGesture(minimumDistance: 4)
            .onChanged { g in
                if moving == nil {
                    moving = (i, g.startLocation)
                    model.engine?.send("engine.execute", ["command": "develop.beginInteraction", "params": ["label": "Edit Spot"]])
                }
            }
            .onEnded { g in
                defer {
                    moving = nil
                    model.engine?.send("engine.execute", ["command": "develop.endInteraction"])
                }
                guard let a = norm(g.startLocation), let b = norm(g.location) else { return }
                model.engine?.send("engine.execute", ["command": "spot.update", "params": ["index": .int(Int64(i)), "move": [.double(b.x - a.x), .double(b.y - a.y)]]])
            }
    }
}
#endif
