import CoreGraphics
import Foundation
import ImageIO
import LightCraftCore
import SwiftUI
import UniformTypeIdentifiers
#if canImport(ImagePlayground)
import ImagePlayground
#endif

/// Generative Expand by Apple Image Playground (`appleCloud`), the one way to paint the new area that
/// is not on this device: Apple's sheet makes it in Apple's cloud (Private Cloud Compute), so the app
/// asks every time and never picks it by itself. The engine gives the canvas as a picture
/// (`crop.expandCanvas`) and takes the picture the sheet made (`crop.applyExpandPicture`): the photo
/// stays as it is, the new area comes from the picture in the photo's tones. Apple's API takes no mask
/// and, in the sheet, the user works the picture over, so what comes back is whatever the sheet made.
nonisolated enum PlaygroundExpand {
    /// The id the fill is stored with (and the Fill With choice is kept as).
    static let id = "appleCloud"

    /// What the sheet is asked for.
    static let prompt = "Continue this photograph past its edges: extend the scene naturally into the plain or blurred areas around it. Keep the existing photo as it is, in the same light, colours, lens and grain."

    /// `LIGHTCRAFT_FAKE_PLAYGROUND=1` stands the sheet in with a tinted copy of the canvas, so tours and
    /// tests (and a simulator, which has no Apple Intelligence) can run the rest of the flow.
    static var isFake: Bool { ProcessInfo.processInfo.environment["LIGHTCRAFT_FAKE_PLAYGROUND"] == "1" }

    /// Whether this device can use the sheet: iOS or macOS 27, Apple Intelligence on, a supported language and region.
    @MainActor static var isAvailable: Bool {
        if isFake { return true }
        #if canImport(ImagePlayground)
        if #available(iOS 27.0, macOS 27.0, *) { return ImagePlaygroundViewController.isAvailable }
        #endif
        return false
    }

    /// A PNG as a picture.
    static func image(from png: Data) -> CGImage? {
        CGImageSourceCreateWithData(png as CFData, nil).flatMap { CGImageSourceCreateImageAtIndex($0, 0, nil) }
    }

    /// What the stand-in sheet answers: the canvas, bluer, with pale bands, as a PNG.
    static func fakeAnswer(for png: Data) -> Data? {
        guard let canvas = image(from: png), let space = CGColorSpace(name: CGColorSpace.sRGB),
              let ctx = CGContext(data: nil, width: canvas.width, height: canvas.height, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        let all = CGRect(x: 0, y: 0, width: canvas.width, height: canvas.height)
        ctx.draw(canvas, in: all)
        ctx.setFillColor(CGColor(red: 0.1, green: 0.3, blue: 1, alpha: 0.35))
        ctx.fill(all)
        ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 0.18))
        for x in stride(from: 0, to: canvas.width, by: 96) {
            ctx.fill(CGRect(x: x, y: 0, width: 36, height: canvas.height))
        }
        guard let out = ctx.makeImage() else { return nil }
        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(dest, out, nil)
        return CGImageDestinationFinalize(dest) ? data as Data : nil
    }
}

/// What the sheet is given for one Generate: the canvas, and the key naming the crop it is for.
struct PlaygroundRun: Identifiable {
    let id = UUID()
    let key: Int64
    let png: Data
    let image: CGImage
    /// The canvas's size in pixels.
    let size: CGSize
}

extension AppModel {
    /// The canvas for the sheet (`crop.expandCanvas`); `nil` after a word why (Expand off, nothing past the photo).
    func playgroundCanvas() async -> PlaygroundRun? {
        guard let r = await run("crop.expandCanvas", ["maxEdge": 1024]) else { return nil }
        guard let b64 = r["png"].string, let png = Data(base64Encoded: b64), let key = r["key"].int, let image = PlaygroundExpand.image(from: png) else {
            show(String(localized: "Couldn't prepare the picture for Apple Image Playground."), error: true)
            return nil
        }
        return PlaygroundRun(key: key, png: png, image: image, size: CGSize(width: r["width"].double ?? 0, height: r["height"].double ?? 0))
    }

    /// The sheet made `url`: it becomes the fill (`crop.applyExpandPicture`, one undo step). The file is
    /// read at once; the sheet's temporary file goes with it.
    func playgroundFinish(file url: URL, key: Int64) async {
        if await run("crop.applyExpandPicture", ["key": .int(key), "path": .string(url.path), "model": .string(PlaygroundExpand.id)]) != nil {
            buzz(.light)
        }
    }

    /// The whole flow with the stand-in sheet (tours): the canvas, a tinted copy of it as the answer, the fill.
    func playgroundExpandWithStandIn() async {
        guard let canvas = await playgroundCanvas() else { return }
        guard let answer = PlaygroundExpand.fakeAnswer(for: canvas.png) else { return }
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("playground-\(UUID().uuidString).png")
        defer { try? FileManager.default.removeItem(at: file) }
        guard (try? answer.write(to: file)) != nil else { return }
        await playgroundFinish(file: file, key: canvas.key)
    }
}

extension View {
    /// Apple's Image Playground sheet for a Generative Expand, shown while `run` is set (iOS and macOS 27; elsewhere nothing
    /// is shown). `done` hears the picture's file and the run's key; the file is gone when it returns.
    @ViewBuilder func playgroundExpandSheet(_ run: Binding<PlaygroundRun?>, done: @escaping (URL, Int64) -> Void) -> some View {
        #if canImport(ImagePlayground)
        if #available(iOS 27.0, macOS 27.0, *) {
            modifier(PlaygroundSheet(run: run, done: done))
        } else {
            self
        }
        #else
        self
        #endif
    }
}

#if canImport(ImagePlayground)
@available(iOS 27.0, macOS 27.0, *)
private struct PlaygroundSheet: ViewModifier {
    @Binding var run: PlaygroundRun?
    let done: (URL, Int64) -> Void

    func body(content: Content) -> some View {
        // (an edit of the picture given, as the sheet's own size steps allow: the canvas's)
        var options = ImagePlaygroundOptions()
        options.creationStrategy = .editExisting
        options.personalization = .disabled
        if let size = run?.size { options.sizeSpecification = .closest(to: size) }
        let key = run?.key ?? 0
        return content
            .imagePlaygroundSheet(
                isPresented: Binding(get: { run != nil }, set: { if !$0 { run = nil } }),
                concepts: [.text(PlaygroundExpand.prompt)],
                sourceImage: run.map { Image(decorative: $0.image, scale: 1) },
                onCompletion: { url in done(url, key) },
                onCancellation: { run = nil }
            )
            .imagePlaygroundOptions(options)
            .imagePlaygroundGenerationStyle(.any, in: ImagePlaygroundStyle.all)
    }
}
#endif
