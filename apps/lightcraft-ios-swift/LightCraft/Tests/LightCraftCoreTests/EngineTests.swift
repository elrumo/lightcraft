import Foundation
import Testing

import CoreGraphics
@testable import LightCraftCore
@testable import LightCraftImaging

/// The real engine (the Rust library built for this Mac by `../run.sh --test`), driven as the app
/// drives it.
@Suite(.serialized)
struct EngineTests {
    private func demo() async throws -> Engine {
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("lc-swift-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        let (engine, opened) = try await Engine.open(OpenConfig(tmp: tmp.path, demo: true, renderThreads: 2))
        #expect(opened.demo && opened.photos > 0)
        return engine
    }

    /// The first event `match` accepts, within 30 s.
    private func next<T: Sendable>(_ engine: Engine, _ match: @escaping @Sendable (EngineEvent) -> T?) async throws -> T {
        try await withThrowingTaskGroup(of: T?.self) { group in
            group.addTask {
                for await event in engine.events {
                    if let found = match(event) { return found }
                }
                return nil
            }
            group.addTask {
                try await Task.sleep(for: .seconds(30))
                return nil
            }
            let first = try await group.next() ?? nil
            group.cancelAll()
            return try #require(first)
        }
    }

    @Test func theLibraryMatchesTheHeader() {
        #expect(Bridge.abiVersion == Bridge.expectedAbiVersion)
    }

    /// A Mac host refuses a library another LightCraft has open (the egui app locks it) instead of
    /// swapping in the demo library, which would let the user import into one that isn't theirs.
    @Test func aDesktopHostRefusesALibraryOpenElsewhere() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("lc-swift-\(UUID().uuidString)")
        let tmp = dir.appendingPathComponent("tmp")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let config = OpenConfig(tmp: tmp.path, library: dir.appendingPathComponent("lib").path, renderThreads: 1, profile: .desktop)
        let (first, opened) = try await Engine.open(config)
        #expect(!opened.demo)
        do {
            _ = try await Engine.open(config)
            Issue.record("a second host opened a library the first one has open")
        } catch let error as ErrorBody {
            #expect(error.code == "openFailed" && error.message.contains("already open"), "\(error)")
        }
        await first.close()
    }

    @Test func commandsAnswerAndFailAsErrors() async throws {
        let engine = try await demo()
        let page = try await engine.execute("catalog.query", ["limit": 5], as: PhotoPage.self)
        #expect(page.photos.count == 5 && page.total >= 5)
        await #expect(throws: ErrorBody.self) { try await engine.execute("no.such.command") }
        let commands = try await engine.commands()
        let ids = Set(commands.map(\.id))
        #expect(ids.contains("develop.set") && ids.contains("app.export") && ids.contains("edit.undo"))
        let light = try await engine.controls(section: "light")
        #expect(light.contains { $0.id == "light.exposure" && $0.min < $0.max })
        await engine.close()
    }

    @Test func aThumbnailArrivesWithItsPixels() async throws {
        let engine = try await demo()
        let page = try await engine.execute("catalog.query", ["limit": 1], as: PhotoPage.self)
        let photo = try #require(page.photos.first).id
        engine.send("render.thumb", ["photo": .int(Int64(clamping: photo)), "edge": 128])
        let pixels = try await next(engine) { event -> PixelImage? in
            if case .image(let info, let pixels) = event, info.slot == "thumb:\(photo)" { return pixels }
            return nil
        }
        #expect(pixels.width > 0 && pixels.width <= 128 && pixels.stride == pixels.width * 4)
        #expect(pixels.rgba.count == pixels.stride * pixels.height)
        await engine.close()
    }

    /// An HDR edit on a screen that can show it comes with its float picture, as a lease beside the 8-bit one.
    @Test func anHdrEditOnAnHdrScreenComesWithItsFloatPicture() async throws {
        let engine = try await demo()
        let page = try await engine.execute("catalog.query", ["limit": 1], as: PhotoPage.self)
        let photo = try #require(page.photos.first).id
        try await engine.execute("library.select", ["ids": [.int(Int64(clamping: photo))]])
        try await engine.execute("develop.hdr", ["enabled": true, "maxEv": 3])
        try await engine.execute("develop.set", ["control": "light.exposure", "value": 1.0])
        let view: JSONValue = ["width": 160, "height": 120]
        // a screen without headroom: the SDR rendition, 8 bits
        engine.send("render.view", view)
        let sdr = try await next(engine) { event -> PixelImage? in
            if case .image(let info, let pixels) = event, info.slot == "main", !info.draft { return pixels }
            return nil
        }
        #expect(sdr.hdr == nil)
        // a screen with: the same edit again, with float pixels
        _ = try await engine.request("render.hdrDisplay", ["on": true])
        engine.send("render.view", view)
        let pixels = try await next(engine) { event -> PixelImage? in
            if case .image(let info, let pixels) = event, info.slot == "main", !info.draft, info.hdr != nil { return pixels }
            return nil
        }
        let hdr = try #require(pixels.hdr)
        #expect(hdr.width == pixels.width && hdr.height == pixels.height && hdr.stride == hdr.width * 8)
        #expect(hdr.rgba16f.count == hdr.stride * hdr.height && hdr.peak == 8)
        // half floats: some component of an exposure-lifted demo scene is above SDR white (1.0 is 0x3C00)
        let above = hdr.rgba16f.withUnsafeBytes { raw in
            raw.bindMemory(to: UInt16.self).enumerated().contains { $0.offset % 4 < 3 && $0.element > 0x3C00 && $0.element < 0x7C00 }
        }
        #expect(above)
        // the 8-bit picture of an HDR render is Display P3: the converted image says so
        #expect((pixels.cgImage()?.colorSpace?.name as String?) == (CGColorSpace.displayP3 as String))
        await engine.close()
    }

    @Test func aDragIsOneUndoStep() async throws {
        let engine = try await demo()
        let page = try await engine.execute("catalog.query", ["limit": 1], as: PhotoPage.self)
        let photo = try #require(page.photos.first).id
        try await engine.execute("library.select", ["ids": [.int(Int64(clamping: photo))]])
        try await engine.execute("develop.beginInteraction", ["label": "Exposure"])
        for v in [0.25, 0.5, 0.75, 1.0] {
            try await engine.execute("develop.set", ["control": "light.exposure", "value": .double(v)])
        }
        try await engine.execute("develop.endInteraction")
        let state = try await engine.request("host.state").decode(HostState.self)
        #expect(state.undo?.contains("Exposure") == true && state.interaction == nil)
        let exposure = try await engine.controls(section: "light").first { $0.id == "light.exposure" }?.value
        #expect(abs((exposure ?? 0) - 1.0) < 1e-6)
        try await engine.execute("edit.undo")
        let undone = try await engine.controls(section: "light").first { $0.id == "light.exposure" }?.value
        #expect(abs(undone ?? 1) < 1e-6)
        await engine.close()
    }

    @Test func manyRequestsAtOnceEachGetTheirOwnAnswer() async throws {
        let engine = try await demo()
        try await withThrowingTaskGroup(of: (Int, Int).self) { group in
            for n in 1...200 {
                group.addTask {
                    let page = try await engine.execute("catalog.query", ["limit": .int(Int64(n % 7 + 1))], as: PhotoPage.self)
                    return (n, page.photos.count)
                }
            }
            for try await (n, count) in group {
                #expect(count == n % 7 + 1)
            }
        }
        await engine.close()
    }

    @Test func aClosedEngineRefuses() async throws {
        let engine = try await demo()
        await engine.close()
        await #expect(throws: ErrorBody.self) { try await engine.request("host.state") }
    }
}
