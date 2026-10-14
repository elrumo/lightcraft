#if os(iOS)
import SwiftUI

/// Drawing a brushed stroke the tools share: its shape as a path, its outline, and the glow shown
/// while the engine works out what it does (an auto mask, an AI fill).
enum StrokeGlow {
    /// A stroke's centre line (a single point is a dot: a round cap on no length).
    static func path(_ points: [CGPoint], offset: CGSize = .zero) -> Path {
        var p = Path()
        guard let first = points.first else { return p }
        func at(_ q: CGPoint) -> CGPoint { CGPoint(x: q.x + offset.width, y: q.y + offset.height) }
        p.move(to: at(first))
        if points.count == 1 {
            p.addLine(to: at(first))
        } else {
            for q in points.dropFirst() { p.addLine(to: at(q)) }
        }
        return p
    }

    static func style(_ radius: CGFloat, extra: CGFloat = 0) -> StrokeStyle {
        StrokeStyle(lineWidth: max(radius * 2 + extra, 0.5), lineCap: .round, lineJoin: .round)
    }

    /// Only the edge of what the brush covers (the stroke's area, with its inside cut out), dark
    /// under light so it reads on any photo.
    static func outline(_ ctx: GraphicsContext, _ path: Path, radius: CGFloat, color: Color = .white) {
        ring(ctx, path, radius, color: .black.opacity(0.35), width: 4)
        ring(ctx, path, radius, color: color, width: 1.5)
    }

    private static func ring(_ ctx: GraphicsContext, _ path: Path, _ radius: CGFloat, color: Color, width: CGFloat) {
        ctx.drawLayer { layer in
            layer.stroke(path, with: .color(color), style: style(radius, extra: width))
            layer.blendMode = .destinationOut
            layer.stroke(path, with: .color(.black), style: style(radius, extra: -width))
        }
    }

    /// The soft pastel shimmer around what's being worked on (as Photos' Clean Up has): the stroke
    /// lightly filled, a blurred halo of `colors` turning round it, a crisp edge. `alpha` fades it.
    static let shimmer: [Color] = [.cyan, .blue, .purple, .pink, .orange, .yellow, .mint]

    static func glow(_ ctx: GraphicsContext, _ path: Path, radius: CGFloat, colors: [Color], time: Double, alpha: Double, body: Color = .white) {
        guard alpha > 0, let first = colors.first else { return }
        let box = path.boundingRect.insetBy(dx: -radius, dy: -radius)
        let shading = GraphicsContext.Shading.conicGradient(
            Gradient(colors: colors + [first]), center: CGPoint(x: box.midX, y: box.midY), angle: .radians(time * 1.8))
        let pulse = 0.7 + 0.3 * sin(time * 4)
        ctx.stroke(path, with: .color(body.opacity(0.22 * alpha)), style: style(radius))
        ctx.drawLayer { layer in
            layer.opacity = alpha * pulse
            layer.addFilter(.blur(radius: max(6, radius * 0.45)))
            layer.stroke(path, with: shading, style: style(radius, extra: 14))
        }
        ctx.drawLayer { layer in
            layer.opacity = alpha
            layer.stroke(path, with: shading, style: style(radius, extra: 2.5))
            layer.blendMode = .destinationOut
            layer.stroke(path, with: .color(.black), style: style(radius, extra: -2.5))
        }
    }
}
#endif
