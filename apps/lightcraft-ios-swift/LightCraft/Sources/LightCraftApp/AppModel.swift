#if os(iOS)
import CoreGraphics
import Foundation
import LightCraftCore
import LightCraftImaging
import Observation
import SwiftUI
import UIKit

/// Where the app is.
nonisolated enum Route: Hashable {
    case photo(UInt64)
    case commands
}

/// The app's state: the engine, what it last reported, and the pictures it sent. Everything the
/// views show comes from here; everything they do goes to the engine as a request.
@Observable
final class AppModel {
    private(set) var engine: Engine?
    private(set) var opened: Opened?
    private(set) var startError: String?
    private(set) var state = HostState()
    private(set) var activity: [ActivityTask] = []
    /// A message to show briefly (an error answer, a warning at start).
    var message: String?
    var path: [Route] = []
    /// The edit sheet is open over the loupe.
    var editing = false
    let images = ImageStore()
    let library = LibraryModel()
    let script = ScriptHost()

    /// The loupe's last render request: drafts during a slider drag go at the same size.
    private(set) var lastView: (photo: UInt64, width: Int, height: Int)?
    /// When the last draft was asked for (slider → pixels on screen: `ui.inspect` → `perf`).
    private var draftAskedAt: ContinuousClock.Instant?
    private(set) var lastDraftRoundTripMs: Double?
    private var visibleThumbs: Set<UInt64> = []
    private var retainScheduled = false

    // MARK: start, lifecycle

    func start() async {
        guard engine == nil, startError == nil else { return }
        do {
            let (engine, opened) = try await Engine.open(Self.config())
            self.engine = engine
            self.opened = opened
            message = opened.warnings.first
            Task { await listen(engine) }
            await library.reload(engine)
        } catch {
            startError = error.localizedDescription
        }
    }

    private static func config() -> OpenConfig {
        let env = ProcessInfo.processInfo.environment
        let nonEmpty = { (key: String) in env[key].flatMap { $0.isEmpty ? nil : $0 } }
        return OpenConfig(
            home: NSHomeDirectory(),
            tmp: NSTemporaryDirectory(),
            library: nonEmpty("LIGHTCRAFT_LIBRARY"),
            deviceName: UIDevice.current.name,
            script: nonEmpty("LIGHTCRAFT_SCRIPT")
        )
    }

    /// The app became active, went to the background… (`host.lifecycle`: save, pause the GPU, free memory).
    func lifecycle(_ event: String) {
        engine?.send("host.lifecycle", ["event": .string(event)])
    }

    // MARK: events

    private func listen(_ engine: Engine) async {
        for await event in engine.events {
            handle(event)
        }
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case .image(let info, let pixels):
            guard let pixels, let image = pixels.cgImage() else { return }
            images.put(info, image)
            if info.slot == "main", info.draft, let asked = draftAskedAt {
                let elapsed = ContinuousClock.now - asked
                lastDraftRoundTripMs = Double(elapsed.components.seconds) * 1000 + Double(elapsed.components.attoseconds) / 1e15
                draftAskedAt = nil
            }
        case .renderFailed(let slot, _, let error):
            images.failed[slot] = error
        case .state(let new):
            let old = state
            state = new
            if new.visibleGen != old.visibleGen || new.libraryGeneration != old.libraryGeneration, let engine {
                if new.libraryGeneration != old.libraryGeneration {
                    images.forgetAll()
                }
                Task { await library.reload(engine) }
            }
        case .activity(let tasks):
            activity = tasks
        case .message(let text, _):
            message = text
        case .uiRequest(let id, let method, let params):
            script.answer(id: id, method: method, params: params, model: self)
        case .closed:
            engine = nil
        case .reply, .unknown:
            break
        }
    }

    // MARK: pictures

    /// Ask for a grid thumbnail (cheap to repeat: the engine answers only what changed).
    func requestThumb(_ photo: UInt64, edge: Int) {
        engine?.send("render.thumb", ["photo": .int(Int64(clamping: photo)), "edge": .int(Int64(clamping: edge))])
    }

    func thumbAppeared(_ photo: UInt64) {
        visibleThumbs.insert(photo)
        scheduleRetain()
    }

    func thumbDisappeared(_ photo: UInt64) {
        visibleThumbs.remove(photo)
        scheduleRetain()
    }

    /// Tell the engine which thumbnails are on screen (it drops queued renders of the others) and
    /// let pictures far from the screen go.
    private func scheduleRetain() {
        guard !retainScheduled else { return }
        retainScheduled = true
        Task {
            try? await Task.sleep(for: .milliseconds(250))
            retainScheduled = false
            engine?.send("render.retain", ["thumbs": .array(visibleThumbs.map { .int(Int64(clamping: $0)) })])
            images.trimThumbs(keeping: visibleThumbs)
        }
    }

    /// Ask for the loupe's picture of `photo` at `width` × `height` pixels (the photo fits inside).
    /// A slider drag asks for drafts itself.
    func requestView(_ photo: UInt64, width: Int, height: Int, draft: Bool = false) {
        guard let engine, width >= 8, height >= 8 else { return }
        lastView = (photo, width, height)
        if draft {
            draftAskedAt = .now
        }
        engine.send("render.view", ["photo": .int(Int64(clamping: photo)), "width": .int(Int64(clamping: width)), "height": .int(Int64(clamping: height)), "draft": .bool(draft)])
    }

    /// A draft of the loupe's photo at the size it was last asked for (a slider moved).
    func requestDraft() {
        guard let v = lastView else { return }
        requestView(v.photo, width: v.width, height: v.height, draft: true)
    }

    // MARK: commands

    /// The loupe's photo is the one the edit commands work on.
    func select(_ photo: UInt64) async {
        await run("library.select", ["ids": [.int(Int64(clamping: photo))]])
    }

    func undo() {
        Task { await run("edit.undo") }
    }

    func redo() {
        Task { await run("edit.redo") }
    }

    /// Run an engine command; a refusal becomes the message shown.
    @discardableResult
    func run(_ command: String, _ params: JSONValue = [:]) async -> JSONValue? {
        guard let engine else { return nil }
        do {
            return try await engine.execute(command, params)
        } catch {
            message = error.localizedDescription
            return nil
        }
    }

    /// Which screen is showing (automation).
    var screenName: String {
        switch path.last {
        case .photo: editing ? "edit" : "loupe"
        case .commands: "commands"
        case nil: "grid"
        }
    }
}

/// The pictures the engine sent, by slot.
@Observable
final class ImageStore {
    struct Picture {
        let image: CGImage
        let photo: UInt64
        let key: UInt64
        let isFinal: Bool
        let draft: Bool
        let ms: Double
        let histogram: Histogram?
    }

    private(set) var pictures: [String: Picture] = [:]
    var failed: [String: String] = [:]
    /// Thumbnails kept in memory beyond those on screen.
    private let maxThumbs = 400

    func put(_ info: ImageEvent, _ image: CGImage) {
        pictures[info.slot] = Picture(image: image, photo: info.photo, key: info.key, isFinal: info.isFinal, draft: info.draft, ms: info.ms, histogram: info.histogram)
        failed[info.slot] = nil
        // the real thumbnail replaces its stand-in
        if info.slot.hasPrefix("thumb:") {
            pictures["thumbQuick:\(info.photo)"] = nil
        }
    }

    /// A grid cell's picture: the thumbnail, else its stand-in.
    func thumb(_ photo: UInt64) -> Picture? {
        pictures["thumb:\(photo)"] ?? pictures["thumbQuick:\(photo)"]
    }

    /// The loupe's picture of `photo`: its render, else the stand-in, else the thumbnail.
    func view(for photo: UInt64) -> Picture? {
        if let main = pictures["main"], main.photo == photo { return main }
        if let preview = pictures["preview"], preview.photo == photo { return preview }
        return thumb(photo)
    }

    /// Let thumbnails go, beyond `maxThumbs`, that aren't on screen.
    func trimThumbs(keeping visible: Set<UInt64>) {
        let thumbs = pictures.keys.filter { $0.hasPrefix("thumb") }
        guard thumbs.count > maxThumbs else { return }
        for slot in thumbs where !visible.contains(pictures[slot]?.photo ?? 0) {
            pictures[slot] = nil
        }
    }

    /// Another library: photo ids mean something else now.
    func forgetAll() {
        pictures.removeAll()
        failed.removeAll()
    }
}

/// The grid's photos (the engine's current source, filter and sort).
@Observable
final class LibraryModel {
    private(set) var photos: [PhotoSummary] = []
    private(set) var total = 0
    private(set) var loading = false
    private var generation = 0

    func reload(_ engine: Engine) async {
        generation += 1
        let mine = generation
        loading = true
        defer { if mine == generation { loading = false } }
        var all: [PhotoSummary] = []
        var offset = 0
        while true {
            guard let page = try? await engine.execute("catalog.query", ["offset": .int(Int64(offset)), "limit": 500], as: PhotoPage.self) else { break }
            guard mine == generation else { return }
            all += page.photos
            total = page.total
            offset += page.photos.count
            if page.photos.isEmpty || offset >= page.total { break }
        }
        photos = all
    }
}
#endif
