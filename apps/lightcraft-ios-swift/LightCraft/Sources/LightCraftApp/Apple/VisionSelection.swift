#if os(iOS)
import CoreGraphics
import CoreVideo
import Foundation
import ImageIO
import LightCraftCore
import Vision

/// Apple's own selection models, as [`SelectionProvider`]s. They run on this device through the
/// Vision framework; nothing leaves it. What they answer is a coverage picture of the photo the
/// engine showed them (`mask.inputImage`): the engine turns it into the stored mask.
nonisolated enum VisionSelection {
    /// The picture the engine gave (a PNG).
    static func image(_ png: Data) throws -> CGImage {
        guard let source = CGImageSourceCreateWithData(png as CFData, nil), let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
            throw SelectionError.failed("The photo could not be read for selection.")
        }
        return image
    }

    /// Run a Vision call, turning the system's "can't run here" failures (no inference engine: the
    /// simulator; an unsupported compute device) into one to show.
    static func run<T>(_ body: () async throws -> T) async throws -> T {
        do {
            return try await body()
        } catch {
            let ns = error as NSError
            // VNErrorCode internalError (9), unsupportedComputeStage (21), unsupportedComputeDevice (22)
            if ns.domain == "com.apple.Vision", [9, 21, 22].contains(ns.code) {
                throw SelectionError.unavailable(String(localized: "Apple's selection model couldn't start on this device."))
            }
            throw error
        }
    }

    /// A tap as Vision wants it: normalized with the origin at the bottom left.
    static func point(_ t: SelectionTap) -> NormalizedPoint {
        NormalizedPoint(x: t.x, y: 1 - t.y)
    }

    /// The coverage in a one-channel mask buffer (8-bit, or 16- or 32-bit float: 0…1).
    static func bitmap(_ buffer: CVPixelBuffer) throws -> SelectionBitmap {
        let (w, h) = (CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer))
        let stride = CVPixelBufferGetBytesPerRow(buffer)
        let format = CVPixelBufferGetPixelFormatType(buffer)
        CVPixelBufferLockBaseAddress(buffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(buffer), w > 0, h > 0, stride >= w else {
            throw SelectionError.failed("Apple's selection model gave an empty mask.")
        }
        var out = [UInt8](repeating: 0, count: w * h)
        func unit(_ v: Float) -> UInt8 { v.isFinite ? UInt8(max(0, min(255, (v * 255).rounded()))) : 0 }
        for y in 0..<h {
            let row = base.advanced(by: y * stride)
            switch format {
            case kCVPixelFormatType_OneComponent8:
                let p = row.assumingMemoryBound(to: UInt8.self)
                for x in 0..<w { out[y * w + x] = p[x] }
            case kCVPixelFormatType_OneComponent32Float:
                guard stride >= w * 4 else { throw SelectionError.failed("Unexpected mask layout.") }
                let p = row.assumingMemoryBound(to: Float.self)
                for x in 0..<w { out[y * w + x] = unit(p[x]) }
            #if arch(arm64)
            case kCVPixelFormatType_OneComponent16Half:
                guard stride >= w * 2 else { throw SelectionError.failed("Unexpected mask layout.") }
                let p = row.assumingMemoryBound(to: Float16.self)
                for x in 0..<w { out[y * w + x] = unit(Float(p[x])) }
            #endif
            default:
                throw SelectionError.failed("Apple's selection model gave a mask in an unexpected format.")
            }
        }
        guard let bitmap = SelectionBitmap(width: w, height: h, coverage: out) else { throw SelectionError.failed("Unexpected mask size.") }
        return bitmap
    }
}

/// Tap an object and Vision lifts it (the model that lifts a subject in Photos). iOS 18 and later:
/// a tap picks the foreground instance under it; no taps at all selects every foreground instance.
nonisolated struct VisionForegroundProvider: SelectionProvider {
    let id = "apple.foreground"

    func select(png: Data, include: [SelectionTap], exclude: [SelectionTap]) async throws -> SelectionBitmap {
        let handler = ImageRequestHandler(try VisionSelection.image(png))
        guard let found = try await VisionSelection.run({ try await handler.perform(GenerateForegroundInstanceMaskRequest()) }) else {
            throw SelectionError.nothingFound
        }
        // (instance 0 is the background)
        var picked = include.isEmpty ? found.allInstances : IndexSet()
        for t in include { picked.formUnion(found.instanceAtPoint(VisionSelection.point(t))) }
        for t in exclude { picked.subtract(found.instanceAtPoint(VisionSelection.point(t))) }
        picked.remove(0)
        guard !picked.isEmpty else { throw SelectionError.nothingFound }
        return try VisionSelection.bitmap(try await VisionSelection.run { try found.generateScaledMask(for: picked, scaledToImageFrom: handler) })
    }
}

/// Tap-to-segment (iOS 27): include and exclude taps refine one object's mask, any object, not only
/// a foreground one. Its model is Apple's, downloaded by the system on request.
@available(iOS 27.0, *)
nonisolated struct VisionIterativeProvider: SelectionProvider {
    let id = "apple.vision"

    func select(png: Data, include: [SelectionTap], exclude: [SelectionTap]) async throws -> SelectionBitmap {
        guard let seed = include.first else { throw SelectionError.nothingFound }
        let request = GenerateIterativeSegmentationRequest(seedPoint: VisionSelection.point(seed))
        for t in include.dropFirst() { try request.addIncludedPoint(VisionSelection.point(t)) }
        for t in exclude { try request.addExcludedPoint(VisionSelection.point(t)) }
        guard case .ready = await request.assetStatus else { throw SelectionError.unavailable("Apple's selection model is not downloaded yet.") }
        let handler = ImageRequestHandler(try VisionSelection.image(png))
        guard let found = try await VisionSelection.run({ try await handler.perform(request) }) else { throw SelectionError.nothingFound }
        return try found.pixelBuffer.withUnsafeBuffer { try VisionSelection.bitmap($0) }
    }

    /// Whether the system already has the model.
    static func isReady() async -> Bool {
        let request = GenerateIterativeSegmentationRequest(seedPoint: NormalizedPoint(x: 0.5, y: 0.5))
        if case .ready = await request.assetStatus { return true }
        return false
    }

    /// Ask the system to download the model; `progress` is told how far it is (0…1).
    static func download(progress: @escaping @Sendable (Double) -> Void) async throws {
        let request = GenerateIterativeSegmentationRequest(seedPoint: NormalizedPoint(x: 0.5, y: 0.5))
        let total = Progress(totalUnitCount: 1000)
        let watcher = Task {
            while !Task.isCancelled {
                progress(total.fractionCompleted)
                try? await Task.sleep(for: .milliseconds(250))
            }
        }
        defer { watcher.cancel() }
        try await request.downloadAssets(progress: total.subprogress(assigningCount: 1000))
        progress(1)
    }
}
#endif
