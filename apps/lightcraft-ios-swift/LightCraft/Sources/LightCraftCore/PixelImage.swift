import Foundation

/// A render's pixels in Swift memory: RGBA8, sRGB, straight alpha, `stride` bytes per row.
public struct PixelImage: Sendable, Equatable {
    public let width: Int
    public let height: Int
    public let stride: Int
    public let rgba: Data
    /// The same render as float pixels, when the display can show the range above SDR white
    /// (`render.hdrDisplay`) and the edit is an HDR one. The 8-bit picture is then Display P3.
    public var hdr: HdrImage?

    public init(width: Int, height: Int, stride: Int, rgba: Data, hdr: HdrImage? = nil) {
        self.width = width
        self.height = height
        self.stride = stride
        self.rgba = rgba
        self.hdr = hdr
    }
}

/// The HDR rendition of a render: RGBA16F (IEEE half floats, little endian, 8 bytes a pixel), Display P3
/// encoded with the sRGB curve carried on past 1, SDR white at 1.0 and highlights up to the encoding of
/// `peak` (linear light).
public struct HdrImage: Sendable, Equatable {
    public let width: Int
    public let height: Int
    public let stride: Int
    public let rgba16f: Data
    public let peak: Double

    public init(width: Int, height: Int, stride: Int, rgba16f: Data, peak: Double) {
        self.width = width
        self.height = height
        self.stride = stride
        self.rgba16f = rgba16f
        self.peak = peak
    }
}
