import Foundation

// The loupe's zoom and pan as plain numbers, so the arithmetic that keeps the photo under the pointer
// is tested without a window. The photo is fitted in the canvas, scaled about the canvas centre by
// `zoom` (1 is fitted) and moved by the pan, all in points of the canvas.

public struct LoupeView: Equatable, Sendable {
    public var zoom: Double
    public var panX: Double
    public var panY: Double

    public init(zoom: Double = 1, panX: Double = 0, panY: Double = 0) {
        self.zoom = zoom
        self.panX = panX
        self.panY = panY
    }

    /// Zoom to `target` (kept within 1 and `limit`) about the canvas point (`x`, `y`): the part of the
    /// photo under that point stays under it, so a pinch or a wheel zooms where the pointer is. A
    /// number that is not a number, or a canvas with no size, changes nothing.
    public mutating func zoomAbout(x: Double, y: Double, to target: Double, canvasWidth: Double, canvasHeight: Double, limit: Double) {
        guard target.isFinite, x.isFinite, y.isFinite, limit.isFinite, zoom.isFinite, zoom > 0, canvasWidth > 0, canvasHeight > 0 else { return }
        let next = min(max(target, 1), max(limit, 1))
        guard next != zoom else { return }
        // (x - centre - pan) / zoom is the photo's point under the pointer, in the unscaled fitted photo;
        // the new pan puts the same point back under it
        let k = 1 - next / zoom
        panX += (x - canvasWidth / 2 - panX) * k
        panY += (y - canvasHeight / 2 - panY) * k
        zoom = next
        clamp(canvasWidth: canvasWidth, canvasHeight: canvasHeight)
    }

    /// Move the photo by (`dx`, `dy`) points, keeping it over the canvas.
    public mutating func pan(dx: Double, dy: Double, canvasWidth: Double, canvasHeight: Double) {
        guard dx.isFinite, dy.isFinite else { return }
        panX += dx
        panY += dy
        clamp(canvasWidth: canvasWidth, canvasHeight: canvasHeight)
    }

    /// Keep the zoomed photo over the canvas: it can be moved as far as its edge reaches the canvas's.
    public mutating func clamp(canvasWidth: Double, canvasHeight: Double) {
        guard zoom.isFinite, canvasWidth.isFinite, canvasHeight.isFinite else { return }
        let limitX = max(canvasWidth * (zoom - 1) / 2, 0)
        let limitY = max(canvasHeight * (zoom - 1) / 2, 0)
        panX = panX.isFinite ? min(max(panX, -limitX), limitX) : 0
        panY = panY.isFinite ? min(max(panY, -limitY), limitY) : 0
    }

    /// What a pinch of `magnification` (the trackpad's change since its last event, 0.1 being 10 %)
    /// multiplies the zoom by.
    public static func pinchFactor(_ magnification: Double) -> Double {
        guard magnification.isFinite else { return 1 }
        return min(max(1 + magnification, 0.5), 2)
    }

    /// What a scroll of `delta` points (a trackpad's, `precise`) or lines (a wheel's) with ⌘ or ⌃ held
    /// multiplies the zoom by: the same step either way, in and out the same size.
    public static func scrollFactor(_ delta: Double, precise: Bool) -> Double {
        guard delta.isFinite else { return 1 }
        let step = precise ? 0.01 : 0.15
        return exp(min(max(delta * step, -1), 1))
    }
}
