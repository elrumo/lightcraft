#if os(iOS)
import LightCraftCore
import SwiftUI

/// The Masking tool on the photo: every mask's pin (tap one for its menu), the selected mask's
/// outline and handles (drag one to resize or turn it, inside it to move it: `mask.grab`,
/// `mask.drag`); with the brush in hand, paint (or erase) the mask (`mask.brushStroke`); with Color
/// Range, a tap samples the colour to select (`mask.sampleColor`).
struct MaskCanvas: View {
    @Environment(AppModel.self) private var model
    @Environment(\.preferredPencilDoubleTapAction) private var pencilAction
    let photo: UInt64
    /// Where the shown photo is drawn.
    let rect: CGRect
    @State private var frame: PhotoFrame?
    @State private var handles: MaskHandles?
    @State private var trail: [CGPoint] = []
    @State private var drag: MaskDrag?
    @State private var grabbing = false
    @State private var menu: JSONValue?
    /// Where a hovering Apple Pencil or pointer is: the brush's size drawn there.
    @State private var hover: CGPoint?

    /// How far (points) from a handle or pin a finger still takes it.
    private static let reach: CGFloat = 28

    var body: some View {
        let tool = model.editor.maskTool
        Canvas { ctx, _ in
            drawHandles(ctx)
            if tool == "brush", let hover, trail.isEmpty {
                let r = brushRadius
                let ring = Path(ellipseIn: CGRect(x: hover.x - r, y: hover.y - r, width: r * 2, height: r * 2))
                ctx.stroke(ring, with: .color(.black.opacity(0.5)), lineWidth: 3)
                ctx.stroke(ring, with: .color(model.editor.brushErase ? .red : .white), lineWidth: 1.5)
            }
            guard trail.count > 0 else { return }
            var p = Path()
            p.addLines(trail)
            let color: Color = model.editor.brushErase ? .red : .accentColor
            ctx.stroke(p, with: .color(color.opacity(0.4)), style: StrokeStyle(lineWidth: brushRadius * 2, lineCap: .round, lineJoin: .round))
        }
        .allowsHitTesting(false)
        .overlay {
            if tool == "brush" {
                Color.clear.contentShape(Rectangle()).gesture(paint).onContinuousHover(perform: hovered)
            } else if tool == "colorRange" {
                Color.clear.contentShape(Rectangle()).onTapGesture { location in
                    guard let n = norm(location) else { return }
                    model.perform("mask.sampleColor", ["x": .double(n.x), "y": .double(n.y)])
                }
            } else {
                // (the drag wins over the pager's scroll; a tap rides along with it)
                Color.clear.contentShape(Rectangle())
                    .highPriorityGesture(move, including: handles?.grips.isEmpty == false ? .all : .none)
                    .simultaneousGesture(SpatialTapGesture().onEnded { tap in Task { await tapped(tap.location) } })
            }
        }
        .confirmationDialog(menuTitle, isPresented: Binding(get: { menu != nil }, set: { if !$0 { menu = nil } }), titleVisibility: .visible) {
            if let id = menu {
                let mask = masks.first { $0["id"] == id }
                Button(mask?["visible"].bool == false ? "Show Mask" : "Hide Mask") { model.perform("mask.visible", ["id": id]) }
                Button("Invert") { model.perform("mask.invert", ["id": id]) }
                Button("Duplicate Mask") { model.perform("mask.duplicate", ["id": id]) }
                Button("Delete Mask", role: .destructive) { model.perform("mask.delete", ["id": id]); model.buzz(.warning) }
            }
        }
        .haptic(.selection, trigger: drag?.mask)
        // Apple Pencil: a double tap swaps the brush between paint and erase, when the system's
        // Pencil setting asks to switch tools
        .onPencilDoubleTap(perform: pencilTapped)
        .task(id: Key(revision: model.state.catalogRevision, mask: model.state.activeMask)) { await load() }
    }

    private func hovered(_ phase: HoverPhase) {
        if case .active(let at) = phase { hover = at } else { hover = nil }
    }

    private func pencilTapped(_: PencilDoubleTapGestureValue) {
        let swaps: Bool = pencilAction == .switchEraser || pencilAction == .switchPrevious
        guard model.editor.maskTool == "brush", swaps else { return }
        model.editor.brushErase.toggle()
        model.buzz(.selection)
    }

    private nonisolated struct Key: Hashable {
        let revision: UInt64
        let mask: UInt32?
    }

    private var masks: [JSONValue] { model.editor.setting("masks").array ?? [] }

    private var menuTitle: String {
        guard let id = menu else { return "" }
        return masks.first { $0["id"] == id }?["name"].string ?? String(localized: "Mask")
    }

    private func load() async {
        guard let engine = model.engine else { return }
        frame = try? await engine.execute("view.frame", ["id": .int(Int64(clamping: photo))], as: PhotoFrame.self)
        handles = try? await engine.execute("mask.handles", [:], as: MaskHandles.self)
    }

    private var brushRadius: CGFloat {
        CGFloat(model.editor.brushSize) * max(rect.width, rect.height)
    }

    private func norm(_ p: CGPoint) -> (x: Double, y: Double)? {
        guard let frame, rect.width > 0, rect.height > 0 else { return nil }
        return frame.norm(u: (p.x - rect.minX) / rect.width, v: (p.y - rect.minY) / rect.height)
    }

    private func screen(_ p: [Double]) -> CGPoint? {
        guard let frame, p.count == 2 else { return nil }
        let s = frame.shown(x: p[0], y: p[1])
        return CGPoint(x: rect.minX + s.u * rect.width, y: rect.minY + s.v * rect.height)
    }

    /// The finger's reach in long-edge units (what `mask.grab` measures in).
    private var reach: Double {
        guard let h = handles, h.long.count == 2, let o = screen([0, 0]), let x = screen([h.long[0], 0]) else { return 0.03 }
        return Double(Self.reach) / max(Double(hypot(x.x - o.x, x.y - o.y)), 1e-9)
    }

    // MARK: drawing

    private func drawHandles(_ ctx: GraphicsContext) {
        guard let h = handles else { return }
        let line = StrokeStyle(lineWidth: 1.5)
        for outline in h.outlines {
            let pts = outline.compactMap(screen)
            guard pts.count > 2 else { continue }
            var p = Path()
            p.addLines(pts)
            p.closeSubpath()
            ctx.stroke(p, with: .color(.black.opacity(0.25)), style: StrokeStyle(lineWidth: 3))
            ctx.stroke(p, with: .color(.white.opacity(0.9)), style: line)
        }
        let active = model.state.activeMask
        let selected = h.grips.filter { $0.handle > 0 }
        // a linear gradient: a line across the photo through its start and its end
        for g in selected where g.kind == "linear" {
            guard let a = screen(g.at), let other = selected.first(where: { $0.mask == g.mask && $0.component == g.component && $0.handle != g.handle }),
                  let b = screen(other.at) else { continue }
            let len = max(hypot(b.x - a.x, b.y - a.y), 1e-6)
            let perp = CGPoint(x: -(b.y - a.y) / len * 3000, y: (b.x - a.x) / len * 3000)
            var p = Path()
            p.move(to: CGPoint(x: a.x - perp.x, y: a.y - perp.y))
            p.addLine(to: CGPoint(x: a.x + perp.x, y: a.y + perp.y))
            ctx.stroke(p, with: .color(.black.opacity(0.2)), style: StrokeStyle(lineWidth: 3))
            ctx.stroke(p, with: .color(.white.opacity(g.handle == 1 ? 0.95 : 0.6)), style: StrokeStyle(lineWidth: 1.5))
        }
        // a radial's rotation grip, on an arm from its edge
        for g in selected where g.kind == "radial" && g.handle == 5 {
            guard let a = selected.first(where: { $0.mask == g.mask && $0.component == g.component && $0.handle == 1 }).flatMap({ screen($0.at) }),
                  let b = screen(g.at) else { continue }
            var p = Path()
            p.move(to: a)
            p.addLine(to: b)
            ctx.stroke(p, with: .color(.black.opacity(0.2)), style: StrokeStyle(lineWidth: 3))
            ctx.stroke(p, with: .color(.white.opacity(0.9)), style: StrokeStyle(lineWidth: 1))
        }
        for g in selected {
            guard let q = screen(g.at) else { continue }
            let r: CGFloat = g.handle == 5 ? 5 : 7
            let c = Path(ellipseIn: CGRect(x: q.x - r, y: q.y - r, width: r * 2, height: r * 2))
            ctx.fill(c, with: .color(.black.opacity(0.25)))
            ctx.stroke(c, with: .color(.white), style: StrokeStyle(lineWidth: 2))
        }
        guard model.prefs.maskPins else { return }
        for g in h.grips where g.handle == 0 {
            guard let q = screen(g.at) else { continue }
            let chosen = active == g.mask
            let c = Path(ellipseIn: CGRect(x: q.x - 9, y: q.y - 9, width: 18, height: 18))
            ctx.fill(c, with: .color(chosen ? .accentColor : Color(white: 0.8)))
            ctx.stroke(c, with: .color(.white), style: StrokeStyle(lineWidth: 2))
        }
    }

    // MARK: gestures

    /// A tap on a pin selects its mask and opens its menu.
    private func tapped(_ location: CGPoint) async {
        guard let engine = model.engine, let n = norm(location) else { return }
        let at: JSONValue = [.double(n.x), .double(n.y)]
        guard let hit = try? await engine.execute("mask.grab", ["at": at, "reach": .double(reach), "pins": .bool(model.prefs.maskPins), "body": false]),
              hit != .null, hit["handle"].int == 0 else { return }
        if hit["mask"].int != model.state.activeMask.map(Int64.init) {
            await model.run("mask.select", ["id": hit["mask"]])
        }
        menu = hit["mask"]
    }

    /// A drag from a handle resizes or turns the mask; from a pin, or inside a radial, moves it.
    private var move: some Gesture {
        DragGesture(minimumDistance: 4)
            .onChanged { g in
                if let d = drag {
                    send(d, to: g.location)
                } else if !grabbing {
                    grabbing = true
                    Task { await grab(from: g.startLocation, to: g.location) }
                }
            }
            .onEnded { g in
                if let d = drag {
                    send(d, to: g.location)
                    model.engine?.send("engine.execute", ["command": "develop.endInteraction"])
                }
                drag = nil
                grabbing = false
            }
    }

    private func grab(from start: CGPoint, to location: CGPoint) async {
        guard let engine = model.engine, let n = norm(start) else { return }
        let at: JSONValue = [.double(n.x), .double(n.y)]
        guard grabbing, let hit = try? await engine.execute("mask.grab", ["at": at, "reach": .double(reach), "pins": .bool(model.prefs.maskPins)]),
              hit != .null, grabbing else { return }
        if hit["mask"].int != model.state.activeMask.map(Int64.init) {
            await model.run("mask.select", ["id": hit["mask"]])
        }
        engine.send("engine.execute", ["command": "develop.beginInteraction", "params": ["label": "Edit Mask"]])
        let d = MaskDrag(mask: hit["mask"], component: hit["component"], handle: hit["handle"], shape: hit["shape"], from: at)
        drag = d
        send(d, to: location)
    }

    private func send(_ d: MaskDrag, to location: CGPoint) {
        guard let n = norm(location) else { return }
        model.engine?.send("engine.execute", ["command": "mask.drag", "params": [
            "id": d.mask, "component": d.component, "handle": d.handle, "shape": d.shape, "from": d.from, "to": [.double(n.x), .double(n.y)],
        ]])
        model.requestDraft()
        Task { await load() }
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
                var p: [String: JSONValue] = [
                    "points": .array(pts), "size": .double(e.brushSize), "feather": .double(e.brushFeather), "flow": .double(e.brushFlow),
                    "erase": .bool(e.brushErase), "autoMask": .bool(e.brushAutoMask),
                ]
                if let active = model.state.activeMask { p["id"] = .int(Int64(active)) }
                model.perform("mask.brushStroke", .object(p))
            }
    }
}

/// What `mask.handles` gives: normalized units per long-edge unit, the pins and handles, the
/// selected mask's outlines.
private nonisolated struct MaskHandles: Decodable {
    var long: [Double]
    var grips: [Grip]
    var outlines: [[[Double]]]

    nonisolated struct Grip: Decodable {
        var mask: UInt32
        var component: Int
        var handle: Int
        var kind: String?
        var at: [Double]
    }
}

/// A mask being dragged: what `mask.grab` took and where the drag began (image coordinates).
private struct MaskDrag {
    let mask: JSONValue
    let component: JSONValue
    let handle: JSONValue
    let shape: JSONValue
    let from: JSONValue
}
#endif
