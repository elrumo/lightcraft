import Foundation

/// A render's pixels in Swift memory: RGBA8, sRGB, straight alpha, `stride` bytes per row.
public struct PixelImage: Sendable, Equatable {
    public let width: Int
    public let height: Int
    public let stride: Int
    public let rgba: Data

    public init(width: Int, height: Int, stride: Int, rgba: Data) {
        self.width = width
        self.height = height
        self.stride = stride
        self.rgba = rgba
    }
}
