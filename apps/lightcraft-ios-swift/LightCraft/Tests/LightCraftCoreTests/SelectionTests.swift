import Foundation
import Testing

@testable import LightCraftCore

/// The pieces of the Object selection that need no Vision: the bitmap the engine takes, the taps,
/// and the stand-in provider tours use.
struct SelectionTests {
    /// A 1-pixel-wide header is enough: only the size is read.
    private func png(_ w: Int, _ h: Int) -> Data {
        func be(_ v: Int) -> [UInt8] { [UInt8(v >> 24 & 255), UInt8(v >> 16 & 255), UInt8(v >> 8 & 255), UInt8(v & 255)] }
        return Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52] + be(w) + be(h))
    }

    @Test func aBitmapMustMatchItsSize() {
        #expect(SelectionBitmap(width: 2, height: 2, coverage: [0, 255, 255, 0]) != nil)
        #expect(SelectionBitmap(width: 2, height: 2, coverage: [0, 255, 255]) == nil)
        #expect(SelectionBitmap(width: 0, height: 2, coverage: []) == nil)
        #expect(SelectionBitmap(width: 100_000, height: 1, coverage: []) == nil)
        #expect(SelectionBitmap(width: Int.max, height: 2, coverage: []) == nil)
    }

    @Test func aBitmapIsTheEnginesParameter() throws {
        let b = try #require(SelectionBitmap(width: 2, height: 1, coverage: [255, 0]))
        #expect(b.json["width"].int == 2 && b.json["height"].int == 1)
        #expect(Data(base64Encoded: b.json["data"].string ?? "") == Data([255, 0]))
    }

    @Test func tapsRoundTripThroughTheEnginesPoints() {
        let taps = [SelectionTap(x: 0.25, y: 0.5), SelectionTap(x: 1, y: 0)]
        #expect(SelectionTap.list(SelectionTap.json(taps)) == taps)
        #expect(SelectionTap.list([["x": .double(0.25), "y": .double(0.5)]]) == [SelectionTap(x: 0.25, y: 0.5)], "a mask stores {x, y}")
        #expect(SelectionTap.list(.null).isEmpty)
        #expect(SelectionTap.list([[.double(0.5)], "x"]).isEmpty, "damaged points are skipped")
    }

    @Test func aPngSizeIsReadFromItsHeader() {
        #expect(pngSize(png(1008, 672))?.width == 1008)
        #expect(pngSize(png(1008, 672))?.height == 672)
        #expect(pngSize(Data([1, 2, 3])) == nil)
        #expect(pngSize(png(0, 5)) == nil)
    }

    @Test func theStandInSelectsAroundIncludesAndNotAroundExcludes() async throws {
        let p = EllipseSelectionProvider(radius: 0.2)
        let b = try await p.select(png: png(400, 200), include: [SelectionTap(x: 0.3, y: 0.5)], exclude: [SelectionTap(x: 0.55, y: 0.5)])
        func at(_ x: Double, _ y: Double) -> UInt8 { b.coverage[Int(y * Double(b.height)) * b.width + Int(x * Double(b.width))] }
        #expect(at(0.3, 0.5) == 255, "the tap is in")
        #expect(at(0.45, 0.5) == 0, "where the excluded tap's ellipse overlaps the included one is out")
        #expect(at(0.9, 0.9) == 0, "far away is out")
        await #expect(throws: SelectionError.nothingFound) {
            _ = try await p.select(png: png(400, 200), include: [], exclude: [])
        }
        await #expect(throws: SelectionError.self) {
            _ = try await p.select(png: Data(), include: [SelectionTap(x: 0.5, y: 0.5)], exclude: [])
        }
    }
}
