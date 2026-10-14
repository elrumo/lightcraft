#if os(iOS)
import LightCraftCore
import SwiftUI

/// The crop frame over the whole photo (Crop & Rotate): the outside darkened, the frame with its
/// thirds, brackets at the corners. Drag a corner or an edge to resize, inside to move, outside to
/// rotate; a double tap inside resets the crop. The geometry is the engine's (`crop.drag`,
/// `crop.straighten`, `view.frame`): this draws its answers and passes the finger's points on.
struct CropOverlay: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    /// Where the whole photo is drawn.
    let rect: CGRect
    @State private var frame: PhotoFrame?
    @State private var drag: CropDrag?
    /// The composition guide's lines across the crop (`crop.guides`).
    @State private var guides: [[[Double]]] = []

    /// How far from a handle a finger still takes it (points).
    private let reach: CGFloat = 28

    var body: some View {
        Canvas { ctx, size in
            guard let corners = screenCorners else { return }
            var outside = Path(CGRect(origin: .zero, size: size))
            outside.addPath(Path { p in p.addLines(corners); p.closeSubpath() })
            ctx.fill(outside, with: .color(.black.opacity(0.55)), style: FillStyle(eoFill: true))
            var frameLine = Path()
            frameLine.addLines(corners)
            frameLine.closeSubpath()
            ctx.stroke(frameLine, with: .color(.white), lineWidth: 1.5)
            // the composition guide, across the crop (u along its top edge, v down its side)
            let (o, du, dv) = (corners[0], CGPoint(x: corners[1].x - corners[0].x, y: corners[1].y - corners[0].y),
                               CGPoint(x: corners[3].x - corners[0].x, y: corners[3].y - corners[0].y))
            for line in guides {
                let pts = line.compactMap { q in q.count == 2 ? CGPoint(x: o.x + du.x * q[0] + dv.x * q[1], y: o.y + du.y * q[0] + dv.y * q[1]) : nil }
                guard pts.count > 1 else { continue }
                var p = Path()
                p.addLines(pts)
                ctx.stroke(p, with: .color(.white.opacity(drag == nil ? 0.35 : 0.7)), lineWidth: 0.75)
            }
            // brackets at the corners
            for i in 0..<4 {
                let c = corners[i]
                let a = corners[(i + 1) % 4], b = corners[(i + 3) % 4]
                var p = Path()
                p.move(to: towards(c, a, 22))
                p.addLine(to: c)
                p.addLine(to: towards(c, b, 22))
                ctx.stroke(p, with: .color(.white), style: StrokeStyle(lineWidth: 4, lineCap: .round, lineJoin: .round))
            }
        }
        .allowsHitTesting(false)
        .contentShape(Rectangle())
        .overlay {
            Color.clear
                .contentShape(Rectangle())
                .highPriorityGesture(dragGesture)
                .simultaneousGesture(SpatialTapGesture(count: 2).onEnded { tap in
                    if let c = screenCorners, Path({ p in p.addLines(c); p.closeSubpath() }).contains(tap.location) {
                        model.perform("crop.reset")
                    }
                })
        }
        .task(id: model.state.catalogRevision) { await load() }
        .task(id: GuideKey(kind: model.prefs.cropGuide, orientation: model.prefs.cropGuideOrientation, crop: frame?.crop?.rect ?? [])) { await loadGuides() }
        .accessibilityElement()
        .accessibilityLabel("Crop frame")
        .accessibilityHint("Drag a corner or an edge to crop, outside the frame to rotate")
    }

    private func load() async {
        guard let engine = model.engine else { return }
        frame = try? await engine.execute("view.frame", ["id": .int(Int64(clamping: photo)), "applyCrop": false], as: PhotoFrame.self)
    }

    private nonisolated struct GuideKey: Hashable {
        let kind: String
        let orientation: Int
        let crop: [Double]
    }

    /// The guide for the crop as it is (its shape decides where the diagonals and the spiral go).
    private func loadGuides() async {
        guard let engine = model.engine, let frame, let r = frame.crop?.rect, r.count == 4 else { return }
        let (w, h) = ((r[2] - r[0]) * frame.width, (r[3] - r[1]) * frame.height)
        let answer = try? await engine.execute("crop.guides", ["kind": .string(model.prefs.cropGuide), "orientation": .int(Int64(model.prefs.cropGuideOrientation)),
                                                               "width": .double(w), "height": .double(h)])
        guides = (answer?["paths"].array ?? []).map { ($0.array ?? []).map { ($0.array ?? []).compactMap(\.double) } }
    }

    /// The crop's corners on screen (top-left, top-right, bottom-right, bottom-left of the crop).
    private var screenCorners: [CGPoint]? {
        guard let frame, let quad = frame.crop?.quad, quad.count == 4 else { return nil }
        return quad.map { q in
            let s = frame.shown(x: q[0], y: q.count > 1 ? q[1] : 0)
            return CGPoint(x: rect.minX + s.u * rect.width, y: rect.minY + s.v * rect.height)
        }
    }

    /// A screen point in the straightened frame.
    private func straight(_ p: CGPoint) -> (x: Double, y: Double)? {
        guard let frame, let crop = frame.crop, rect.width > 0, rect.height > 0 else { return nil }
        let n = frame.norm(u: (p.x - rect.minX) / rect.width, v: (p.y - rect.minY) / rect.height)
        return crop.straight(x: n.x, y: n.y)
    }

    private nonisolated struct CropDrag: Equatable {
        /// `topLeft`…`left`, `move`, or `rotate`.
        let handle: String
        let start: [Double]
        let angle: Double
        let centre: CGPoint
        let from: CGPoint
    }

    private static let names = ["topLeft", "topRight", "bottomRight", "bottomLeft", "top", "right", "bottom", "left"]

    /// The handle under `p`: a corner, an edge's middle, inside (move) or outside (rotate).
    private func handle(at p: CGPoint, _ c: [CGPoint]) -> String {
        let points = c + (0..<4).map { lerp(c[$0], c[($0 + 1) % 4], 0.5) }
        // edge mids are top, right, bottom, left in the crop's order
        let best = points.enumerated().min { hypot($0.element.x - p.x, $0.element.y - p.y) < hypot($1.element.x - p.x, $1.element.y - p.y) }
        if let best, hypot(best.element.x - p.x, best.element.y - p.y) < reach {
            return Self.names[best.offset]
        }
        return Path({ path in path.addLines(c); path.closeSubpath() }).contains(p) ? "move" : "rotate"
    }

    private var dragGesture: some Gesture {
        DragGesture(minimumDistance: 2)
            .onChanged { g in
                guard let c = screenCorners, let crop = frame?.crop else { return }
                if drag == nil {
                    let h = handle(at: g.startLocation, c)
                    let centre = CGPoint(x: c.map(\.x).reduce(0, +) / 4, y: c.map(\.y).reduce(0, +) / 4)
                    drag = CropDrag(handle: h, start: crop.rect, angle: crop.angle, centre: centre, from: g.startLocation)
                    model.engine?.send("engine.execute", ["command": "develop.beginInteraction", "params": ["label": .string(h == "rotate" ? "Straighten" : "Crop")]])
                }
                guard let d = drag else { return }
                if d.handle == "rotate" {
                    let a0 = atan2(d.from.y - d.centre.y, d.from.x - d.centre.x)
                    let a1 = atan2(g.location.y - d.centre.y, g.location.x - d.centre.x)
                    let angle = min(max(d.angle + Double(a1 - a0) * 180 / .pi, -45), 45)
                    model.engine?.send("engine.execute", ["command": "crop.straighten", "params": ["angle": .double(angle)]])
                } else if let from = straight(d.from), let to = straight(g.location) {
                    model.engine?.send("engine.execute", ["command": "crop.drag", "params": [
                        "handle": .string(d.handle), "from": [.double(from.x), .double(from.y)], "to": [.double(to.x), .double(to.y)],
                        "start": .array(d.start.map { .double($0) }),
                    ]])
                }
                Task { await load() }
            }
            .onEnded { _ in
                if drag != nil {
                    model.engine?.send("engine.execute", ["command": "develop.endInteraction"])
                }
                drag = nil
                Task { await load() }
            }
    }

    private func lerp(_ a: CGPoint, _ b: CGPoint, _ t: CGFloat) -> CGPoint {
        CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t)
    }

    private func towards(_ a: CGPoint, _ b: CGPoint, _ len: CGFloat) -> CGPoint {
        let d = max(hypot(b.x - a.x, b.y - a.y), 1)
        return CGPoint(x: a.x + (b.x - a.x) / d * min(len, d / 2), y: a.y + (b.y - a.y) / d * min(len, d / 2))
    }
}
#endif
