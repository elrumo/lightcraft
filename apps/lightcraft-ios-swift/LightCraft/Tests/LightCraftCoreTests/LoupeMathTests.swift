import Foundation
import Testing

@testable import LightCraftCore

/// The loupe's zoom and pan without a window: the point under the pointer stays under it.
struct LoupeMathTests {
    private let width = 1000.0
    private let height = 800.0

    /// Where the photo point under the canvas point (x, y) is, in the fitted unscaled photo.
    private func photoPoint(_ v: LoupeView, x: Double, y: Double) -> (Double, Double) {
        ((x - width / 2 - v.panX) / v.zoom, (y - height / 2 - v.panY) / v.zoom)
    }

    @Test func zoomingAboutAPointKeepsThePhotoUnderIt() {
        for (x, y) in [(500.0, 400.0), (700.0, 300.0), (120.0, 90.0), (990.0, 790.0)] {
            var v = LoupeView()
            let before = photoPoint(v, x: x, y: y)
            v.zoomAbout(x: x, y: y, to: 2, canvasWidth: width, canvasHeight: height, limit: 16)
            let after = photoPoint(v, x: x, y: y)
            #expect(abs(before.0 - after.0) < 1e-9 && abs(before.1 - after.1) < 1e-9, "(\(x), \(y))")
            #expect(v.zoom == 2)
        }
    }

    @Test func zoomingTwiceAboutAMovingPointStaysUnderIt() {
        var v = LoupeView()
        v.zoomAbout(x: 800, y: 200, to: 3, canvasWidth: width, canvasHeight: height, limit: 16)
        let before = photoPoint(v, x: 300, y: 600)
        v.zoomAbout(x: 300, y: 600, to: 5, canvasWidth: width, canvasHeight: height, limit: 16)
        let after = photoPoint(v, x: 300, y: 600)
        #expect(abs(before.0 - after.0) < 1e-9 && abs(before.1 - after.1) < 1e-9)
    }

    @Test func theCentreZoomsWithoutMoving() {
        var v = LoupeView()
        v.zoomAbout(x: width / 2, y: height / 2, to: 4, canvasWidth: width, canvasHeight: height, limit: 16)
        #expect(v == LoupeView(zoom: 4))
    }

    @Test func zoomStaysBetweenFitAndTheLimit() {
        var v = LoupeView()
        v.zoomAbout(x: 10, y: 10, to: 100, canvasWidth: width, canvasHeight: height, limit: 16)
        #expect(v.zoom == 16)
        v.zoomAbout(x: 10, y: 10, to: 0.2, canvasWidth: width, canvasHeight: height, limit: 16)
        #expect(v == LoupeView(), "all the way out: fitted and centred")
    }

    @Test func theZoomedPhotoStaysOverTheCanvas() {
        var v = LoupeView()
        // the corner of the canvas: the photo's corner comes to it and no further
        v.zoomAbout(x: 0, y: 0, to: 2, canvasWidth: width, canvasHeight: height, limit: 16)
        #expect(v.panX == width / 2 && v.panY == height / 2, "the photo's top left corner stays at the canvas's")
        v.pan(dx: 5000, dy: 5000, canvasWidth: width, canvasHeight: height)
        #expect(v.panX == width / 2 && v.panY == height / 2)
        v.pan(dx: -5000, dy: -5000, canvasWidth: width, canvasHeight: height)
        #expect(v.panX == -width / 2 && v.panY == -height / 2)
    }

    @Test func aFittedPhotoDoesNotPan() {
        var v = LoupeView()
        v.pan(dx: 40, dy: -30, canvasWidth: width, canvasHeight: height)
        #expect(v == LoupeView())
    }

    @Test func scrollingFollowsTheFingers() {
        var v = LoupeView(zoom: 3)
        v.pan(dx: 20, dy: -10, canvasWidth: width, canvasHeight: height)
        #expect(v.panX == 20 && v.panY == -10)
    }

    @Test func pinchAndScrollStepsAreSymmetric() {
        #expect(abs(LoupeView.pinchFactor(0.1) - 1.1) < 1e-12)
        #expect(LoupeView.pinchFactor(-5) == 0.5, "a wild event does not collapse the photo")
        #expect(LoupeView.pinchFactor(9) == 2)
        for precise in [true, false] {
            #expect(abs(LoupeView.scrollFactor(7, precise: precise) * LoupeView.scrollFactor(-7, precise: precise) - 1) < 1e-12)
            #expect(LoupeView.scrollFactor(0, precise: precise) == 1)
            #expect(LoupeView.scrollFactor(1e9, precise: precise) <= exp(1) + 1e-9, "one event is never a huge jump")
        }
    }

    @Test func hostileNumbersChangeNothing() {
        let start = LoupeView(zoom: 2, panX: 10, panY: 20)
        var v = start
        for bad in [Double.nan, .infinity, -.infinity] {
            v.zoomAbout(x: bad, y: 0, to: 3, canvasWidth: width, canvasHeight: height, limit: 16)
            v.zoomAbout(x: 0, y: bad, to: 3, canvasWidth: width, canvasHeight: height, limit: 16)
            v.zoomAbout(x: 0, y: 0, to: bad, canvasWidth: width, canvasHeight: height, limit: 16)
            v.zoomAbout(x: 0, y: 0, to: 3, canvasWidth: width, canvasHeight: height, limit: bad)
            v.pan(dx: bad, dy: 0, canvasWidth: width, canvasHeight: height)
            v.pan(dx: 0, dy: bad, canvasWidth: width, canvasHeight: height)
            #expect(v == start)
            #expect(LoupeView.pinchFactor(bad) == 1)
            #expect(LoupeView.scrollFactor(bad, precise: true) == 1)
        }
        v.zoomAbout(x: 0, y: 0, to: 3, canvasWidth: 0, canvasHeight: height, limit: 16)
        v.zoomAbout(x: 0, y: 0, to: 3, canvasWidth: width, canvasHeight: -1, limit: 16)
        #expect(v == start, "no canvas, no zoom")
        var broken = LoupeView(zoom: 2, panX: .nan, panY: .infinity)
        broken.clamp(canvasWidth: width, canvasHeight: height)
        #expect(broken.panX == 0 && broken.panY == 0, "a pan that is not a number is put back")
    }
}
