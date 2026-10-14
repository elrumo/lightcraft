import Foundation
import Testing

@testable import LightCraftCore

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
