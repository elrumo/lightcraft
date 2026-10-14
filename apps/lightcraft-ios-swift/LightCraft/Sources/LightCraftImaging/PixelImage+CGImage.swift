#if canImport(CoreGraphics)
import CoreGraphics
import Foundation
import LightCraftCore

extension PixelImage {
    /// The pixels as a CoreGraphics image (no copy: it wraps `rgba`). Display P3 when the picture has
    /// an HDR rendition (`hdr`: that render is in P3), else sRGB.
    public func cgImage() -> CGImage? {
        guard width > 0, height > 0, stride >= width * 4, rgba.count >= stride * height,
            let provider = CGDataProvider(data: rgba as CFData)
        else { return nil }
        let space = CGColorSpace(name: hdr == nil ? CGColorSpace.sRGB : CGColorSpace.displayP3) ?? CGColorSpaceCreateDeviceRGB()
        return CGImage(
            width: width,
            height: height,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: stride,
            space: space,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider,
            decode: nil,
            shouldInterpolate: true,
            intent: .defaultIntent
        )
    }
}

extension HdrImage {
    /// The float pixels as a CoreGraphics image (no copy: it wraps `rgba16f`): 16-bit float components
    /// in extended Display P3 (the sRGB curve carried on past 1), which the system draws brighter than
    /// white where the display has headroom (`Image.allowedDynamicRange(.high)`).
    public func cgImage() -> CGImage? {
        guard width > 0, height > 0, stride >= width * 8, rgba16f.count >= stride * height,
            let provider = CGDataProvider(data: rgba16f as CFData),
            let space = CGColorSpace(name: CGColorSpace.extendedDisplayP3)
        else { return nil }
        let info = CGImageAlphaInfo.noneSkipLast.rawValue | CGBitmapInfo.floatComponents.rawValue | CGImageByteOrderInfo.order16Little.rawValue
        return CGImage(
            width: width,
            height: height,
            bitsPerComponent: 16,
            bitsPerPixel: 64,
            bytesPerRow: stride,
            space: space,
            bitmapInfo: CGBitmapInfo(rawValue: info),
            provider: provider,
            decode: nil,
            shouldInterpolate: true,
            intent: .defaultIntent
        )
    }
}
#endif
