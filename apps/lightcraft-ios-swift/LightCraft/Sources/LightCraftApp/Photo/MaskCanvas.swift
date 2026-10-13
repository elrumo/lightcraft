#if os(iOS)
import LightCraftCore
import SwiftUI

/// The Masking tool on the photo: with the brush in hand, paint (or erase) the mask
/// (`mask.brushStroke`); with Color Range, a tap samples the colour to select (`mask.sampleColor`).
struct MaskCanvas: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    /// Where the shown photo is drawn.
    let rect: CGRect
    @State private var frame: PhotoFrame?
    @State private var trail: [CGPoint] = []

    var body: some View {
        let tool = model.editor.maskTool
        Canvas { ctx, _ in
            guard trail.count > 0 else { return }
            var p = Path()
            p.addLines(trail)
            let color: Color = model.editor.brushErase ? .red : .accentColor
            ctx.stroke(p, with: .color(color.opacity(0.4)), style: StrokeStyle(lineWidth: brushRadius * 2, lineCap: .round, lineJoin: .round))
        }
        .allowsHitTesting(false)
        .overlay {
            if tool == "brush" {
                Color.clear.contentShape(Rectangle()).gesture(paint)
            } else if tool == "colorRange" {
                Color.clear.contentShape(Rectangle()).onTapGesture { location in
                    guard let n = norm(location) else { return }
                    model.perform("mask.sampleColor", ["x": .double(n.x), "y": .double(n.y)])
                }
            }
        }
        .task(id: model.state.catalogRevision) {
            guard let engine = model.engine else { return }
            frame = try? await engine.execute("view.frame", ["id": .int(Int64(clamping: photo))], as: PhotoFrame.self)
        }
    }

    private var brushRadius: CGFloat {
        CGFloat(model.editor.brushSize) * max(rect.width, rect.height)
    }

    private func norm(_ p: CGPoint) -> (x: Double, y: Double)? {
        guard let frame, rect.width > 0, rect.height > 0 else { return nil }
        return frame.norm(u: (p.x - rect.minX) / rect.width, v: (p.y - rect.minY) / rect.height)
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
                if let active = model.library.state?.activeMask, active != .null { p["id"] = active }
                model.perform("mask.brushStroke", .object(p))
            }
    }
}
#endif
