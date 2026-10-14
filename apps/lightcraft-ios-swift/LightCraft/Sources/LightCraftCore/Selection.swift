import Foundation

/// A picture of coverage over the whole photo, 0 (out) to 255 (in), `width × height` bytes: what a
/// selection provider answers with, and the `bitmap` that the engine's `mask.add` and
/// `mask.setObject` take. It lines up with the picture `mask.inputImage` gave: the engine turns it
/// into the stored mask, so nothing here knows about crops, orientation or the stored format.
public struct SelectionBitmap: Sendable, Equatable {
    public let width: Int
    public let height: Int
    public let coverage: [UInt8]

    /// `nil` when the size is empty, absurd, or doesn't match the data.
    public init?(width: Int, height: Int, coverage: [UInt8]) {
        let (count, overflow) = width.multipliedReportingOverflow(by: height)
        guard width > 0, height > 0, width <= 8192, height <= 8192, !overflow, count == coverage.count else { return nil }
        self.width = width
        self.height = height
        self.coverage = coverage
    }

    /// The engine's `bitmap` parameter.
    public var json: JSONValue {
        .object(["width": .int(Int64(width)), "height": .int(Int64(height)), "data": .string(Data(coverage).base64EncodedString())])
    }
}

/// A tap on the photo, in the photo's normalized coordinates (0…1, origin top left; what the
/// engine's masks store as `points` and `exclude`).
public struct SelectionTap: Sendable, Equatable {
    public var x: Double
    public var y: Double

    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }

    /// Taps as the engine reports them: a mask's `hint` and `exclude` are `[{x, y}, …]`; `[[x, y], …]`
    /// (what the commands take) reads too.
    public static func list(_ json: JSONValue) -> [SelectionTap] {
        (json.array ?? []).compactMap { p in
            guard let x = p["x"].double ?? p[0].double, let y = p["y"].double ?? p[1].double else { return nil }
            return SelectionTap(x: x, y: y)
        }
    }

    public static func json(_ taps: [SelectionTap]) -> JSONValue {
        .array(taps.map { .array([.double($0.x), .double($0.y)]) })
    }
}

public enum SelectionError: Error, LocalizedError, Equatable {
    /// The provider can't run here (the OS is too old, the model isn't there, the simulator lacks it).
    case unavailable(String)
    /// It ran and found nothing at the taps.
    case nothingFound
    /// The picture or the answer was unusable.
    case failed(String)

    /// (English: the app shows `.nothingFound` with its own translated text.)
    public var errorDescription: String? {
        switch self {
        case .unavailable(let why): why
        case .nothingFound: "Nothing to select there. Tap the object itself."
        case .failed(let why): why
        }
    }
}

/// Something that selects an object from taps. `png` is the picture `mask.inputImage` gave (the
/// photo as a model sees it); the answer covers the same picture. At least one `include` tap.
public protocol SelectionProvider: Sendable {
    /// Names the provider in the AI Models list (`apple.vision`, `apple.foreground`, `fake`).
    var id: String { get }
    func select(png: Data, include: [SelectionTap], exclude: [SelectionTap]) async throws -> SelectionBitmap
}

/// The size of a PNG's picture, read from its header.
public func pngSize(_ png: Data) -> (width: Int, height: Int)? {
    let b = [UInt8](png.prefix(24))
    guard b.count == 24, Array(b[1...3]) == [0x50, 0x4E, 0x47] else { return nil }
    func be(_ i: Int) -> Int { Int(b[i]) << 24 | Int(b[i + 1]) << 16 | Int(b[i + 2]) << 8 | Int(b[i + 3]) }
    let (w, h) = (be(16), be(20))
    return w > 0 && h > 0 ? (w, h) : nil
}

/// A stand-in that needs no model: an ellipse around each included tap, minus one around each
/// excluded tap. Tours and tests use it (`LIGHTCRAFT_FAKE_SEGMENTER=1`) so a selection is the same
/// everywhere.
public struct EllipseSelectionProvider: SelectionProvider {
    public let id = "fake"
    /// Half the ellipse's width and height as a fraction of the photo's.
    public var radius: Double

    public init(radius: Double = 0.2) {
        self.radius = radius
    }

    public func select(png: Data, include: [SelectionTap], exclude: [SelectionTap]) async throws -> SelectionBitmap {
        guard let size = pngSize(png) else { throw SelectionError.failed("not a PNG") }
        guard !include.isEmpty else { throw SelectionError.nothingFound }
        // a coarse grid is enough: the engine resamples it anyway
        let w = min(size.width, 128), h = min(size.height, 128)
        func inside(_ t: SelectionTap, _ x: Double, _ y: Double) -> Bool {
            let dx = (x - t.x) / radius, dy = (y - t.y) / radius
            return dx * dx + dy * dy <= 1
        }
        let cov: [UInt8] = (0..<w * h).map { i in
            let x = (Double(i % w) + 0.5) / Double(w), y = (Double(i / w) + 0.5) / Double(h)
            let hit = include.contains { inside($0, x, y) } && !exclude.contains { inside($0, x, y) }
            return hit ? 255 : 0
        }
        guard let bitmap = SelectionBitmap(width: w, height: h, coverage: cov) else { throw SelectionError.failed("bad size") }
        return bitmap
    }
}
