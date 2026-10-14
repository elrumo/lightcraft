#if canImport(CoreGraphics)
import CoreGraphics
import Foundation
import Testing

@testable import LightCraftCore
@testable import LightCraftImaging

/// Pictures as CoreGraphics images.
struct ImagingTests {
    /// 2 × 1 pixels of RGBA16F: (0.5, 1, 4, 1) and (0, 8, 0.25, 1).
    private func hdr() -> HdrImage {
        let halves: [UInt16] = [0x3800, 0x3C00, 0x4400, 0x3C00, 0x0000, 0x4800, 0x3400, 0x3C00]
        let data = halves.withUnsafeBufferPointer { Data(buffer: $0) }
        return HdrImage(width: 2, height: 1, stride: 16, rgba16f: data, peak: 8)
    }

    /// The float pictures are 16-bit float, in the extended colour space that lets the system draw past
    /// SDR white.
    @Test func anHdrPictureIsA16BitFloatImageInExtendedDisplayP3() throws {
        let image = try #require(hdr().cgImage())
        #expect((image.width, image.height, image.bitsPerComponent, image.bitsPerPixel) == (2, 1, 16, 64))
        #expect(image.bitmapInfo.contains(.floatComponents))
        #expect((image.colorSpace?.name as String?) == (CGColorSpace.extendedDisplayP3 as String))
        #expect(image.colorSpace?.supportsOutput == true)
    }

    /// The pixels read back are the half floats, not clamped to 1.
    @Test func itsPixelsAreNotClampedToSdrWhite() throws {
        let image = try #require(hdr().cgImage())
        let data = try #require(image.dataProvider?.data) as Data
        let halves = data.withUnsafeBytes { Array($0.bindMemory(to: UInt16.self)) }
        #expect(halves.prefix(8) == [0x3800, 0x3C00, 0x4400, 0x3C00, 0x0000, 0x4800, 0x3400, 0x3C00])
    }

    @Test func aBadPictureIsNoImage() {
        #expect(HdrImage(width: 2, height: 1, stride: 8, rgba16f: Data(count: 8), peak: 8).cgImage() == nil, "stride below a row")
        #expect(HdrImage(width: 2, height: 1, stride: 16, rgba16f: Data(count: 4), peak: 8).cgImage() == nil, "fewer bytes than rows")
        #expect(HdrImage(width: 0, height: 1, stride: 0, rgba16f: Data(), peak: 8).cgImage() == nil)
    }

    /// An 8-bit picture is sRGB, unless it has an HDR rendition: then it is Display P3 like it.
    @Test func theEightBitPictureOfAnHdrRenderIsDisplayP3() {
        let rgba = Data(count: 8)
        let plain = PixelImage(width: 2, height: 1, stride: 8, rgba: rgba)
        let withHdr = PixelImage(width: 2, height: 1, stride: 8, rgba: rgba, hdr: hdr())
        #expect((plain.cgImage()?.colorSpace?.name as String?) == (CGColorSpace.sRGB as String))
        #expect((withHdr.cgImage()?.colorSpace?.name as String?) == (CGColorSpace.displayP3 as String))
    }
}
#endif
