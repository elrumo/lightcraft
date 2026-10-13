#if canImport(CoreGraphics)
import CoreGraphics
import Foundation
import LightCraftCore

extension PixelImage {
    /// The pixels as a CoreGraphics image (no copy: it wraps `rgba`).
    public func cgImage() -> CGImage? {
        guard width > 0, height > 0, stride >= width * 4, rgba.count >= stride * height,
            let provider = CGDataProvider(data: rgba as CFData)
        else { return nil }
        let space = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
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
#endif
