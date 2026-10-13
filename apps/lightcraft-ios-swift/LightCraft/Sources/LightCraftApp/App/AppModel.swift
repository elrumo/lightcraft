#if os(iOS)
import CoreGraphics
import Foundation
import LightCraftCore
import LightCraftImaging
import Observation
import SwiftUI
import UIKit

/// A screen pushed over the grid.
nonisolated enum Route: Hashable {
    case photo(UInt64)
    case commands
    case people
    case person(String)
    case map
}

/// A page that slides up over everything (`.sheet`).
nonisolated enum Sheet: Identifiable, Hashable {
    case albums
    case filter
    case settings(String?)
    case export([UInt64])
    case syncStatus
    case syncChoice
    case newAlbum(parent: UInt64?, folder: Bool, addSelected: Bool)
    case renameAlbum(UInt64, String)
    case shareLink(UInt64, String)
    case about
    case keywords
    case importReview
    case history(versions: Bool)
    case copySettings
    case pasteSettings([UInt64])

    var id: String {
        switch self {
        case .albums: "albums"
        case .filter: "filter"
        case .settings: "settings"
        case .export: "export"
        case .syncStatus: "syncStatus"
        case .syncChoice: "syncChoice"
        case .newAlbum: "newAlbum"
        case .renameAlbum: "renameAlbum"
        case .shareLink: "shareLink"
        case .about: "about"
        case .keywords: "keywords"
        case .importReview: "importReview"
        case .history: "history"
        case .copySettings: "copySettings"
        case .pasteSettings: "pasteSettings"
        }
    }
}

/// A short message over the screen.
struct Toast: Equatable {
    var text: String
    var error = false
    var id = UUID()
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
    var toast: Toast?
    var path: [Route] = []
    var sheet: Sheet?
    /// Photos are being chosen in the grid (Select).
    var selectMode = false
    /// Choosing photos to add to this album (its + ▸ LightCraft).
    var addingTo: UInt64?
    /// A dialog the engine asked for (`syncChoice`).
    var asking: String?
    /// Ask before moving these photos to Recently Deleted.
    var confirmDelete: [UInt64]?
    let prefs = Prefs()
    let images = ImageStore()
    let library = LibraryModel()
    let albums = AlbumsModel()
    let editor = EditorModel()
    let script = ScriptHost()

    /// The loupe's last render request: drafts during a slider drag go at the same size.
    private(set) var lastView: (photo: UInt64, width: Int, height: Int)?
    /// When the last draft was asked for (slider → pixels on screen: `ui.inspect` → `perf`).
    private var draftAskedAt: ContinuousClock.Instant?
    private(set) var lastDraftRoundTripMs: Double?
    private var visibleThumbs: Set<UInt64> = []
    private var retainScheduled = false
    /// Grid indexes on screen (pages there are read again when photos change).
    var visibleRange: Range<Int>?

    // MARK: start, lifecycle

    func start() async {
        guard engine == nil, startError == nil else { return }
        // the device's name on the sync server (read by the engine when signing in)
        setenv("LIGHTCRAFT_DEVICE", UIDevice.current.name, 0)
        do {
            let (engine, opened) = try await Engine.open(Self.config())
            self.engine = engine
            self.opened = opened
            if let warning = opened.warnings.first {
                show(warning)
            }
            Task { await listen(engine) }
            state = (try? await engine.request("host.state").decode(HostState.self)) ?? state
            await library.reload(engine, count: state.visibleCount)
            await albums.reload(engine)
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
            demo: nonEmpty("LIGHTCRAFT_DEMO") != nil,
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
            stateChanged(from: state, to: new)
        case .activity(let tasks):
            activity = tasks
        case .message(let text, let level):
            show(text, error: level == "error")
        case .uiRequest(let id, let method, let params):
            script.answer(id: id, method: method, params: params, model: self)
        case .copy(let text):
            UIPasteboard.general.string = text
        case .ask(let dialog, _):
            asking = dialog
            if dialog == "syncChoice" { sheet = .syncChoice }
        case .closed:
            engine = nil
        case .reply, .unknown:
            break
        }
    }

    private func stateChanged(from old: HostState, to new: HostState) {
        state = new
        guard let engine else { return }
        if new.libraryGeneration != old.libraryGeneration {
            images.forgetAll()
            path = []
            Task {
                await library.reload(engine, count: new.visibleCount)
                await albums.reload(engine)
            }
            return
        }
        if new.visibleGen != old.visibleGen {
            Task { await library.reload(engine, count: new.visibleCount) }
        } else if new.catalogRevision != old.catalogRevision {
            Task {
                await library.refresh(engine, keeping: visibleRange)
                await library.refreshState(engine)
            }
        } else if new.selectionCount != old.selectionCount || new.active != old.active {
            Task { await library.refreshState(engine) }
        }
        if new.catalogRevision != old.catalogRevision || new.active != old.active {
            if new.interaction == nil {
                Task { await editor.reload(self) }
            }
            albums.scheduleReload(engine)
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
    func requestView(_ photo: UInt64, width: Int, height: Int, draft: Bool = false, applyCrop: Bool = true) {
        guard let engine, width >= 8, height >= 8 else { return }
        lastView = (photo, width, height)
        if draft {
            draftAskedAt = .now
        }
        engine.send("render.view", ["photo": .int(Int64(clamping: photo)), "width": .int(Int64(clamping: width)), "height": .int(Int64(clamping: height)), "draft": .bool(draft), "applyCrop": .bool(applyCrop)])
    }

    /// The Before of `photo`, at the loupe's size.
    func requestBefore(_ photo: UInt64) {
        guard let engine, let v = lastView else { return }
        engine.send("render.view", ["photo": .int(Int64(clamping: photo)), "width": .int(Int64(clamping: v.width)), "height": .int(Int64(clamping: v.height)), "before": true])
    }

    /// A draft of the loupe's photo at the size it was last asked for (a slider moved).
    func requestDraft() {
        guard let v = lastView else { return }
        requestView(v.photo, width: v.width, height: v.height, draft: true)
    }

    /// Prepare the photos either side of the loupe's.
    func prefetch(_ photos: [UInt64]) {
        guard let engine, let v = lastView, !photos.isEmpty else { return }
        engine.send("render.prefetch", ["photos": .array(photos.map { .int(Int64(clamping: $0)) }), "width": .int(Int64(v.width)), "height": .int(Int64(v.height))])
    }

    // MARK: commands

    /// The loupe's photo is the one the edit commands work on.
    func select(_ photo: UInt64) async {
        await run("library.select", ["ids": [.int(Int64(clamping: photo))]])
    }

    func undo() {
        perform("edit.undo")
    }

    func redo() {
        perform("edit.redo")
    }

    /// Run an engine command; a refusal becomes the message shown.
    @discardableResult
    func run(_ command: String, _ params: JSONValue = [:]) async -> JSONValue? {
        guard let engine else { return nil }
        do {
            return try await engine.execute(command, params)
        } catch {
            show(error.localizedDescription, error: true)
            return nil
        }
    }

    /// Run an engine command without waiting for it.
    func perform(_ command: String, _ params: JSONValue = [:]) {
        Task { await run(command, params) }
    }

    /// Say `text` for a few seconds.
    func show(_ text: String, error: Bool = false) {
        toast = Toast(text: text, error: error)
    }

    /// The photos an action works on: the ones chosen, else the loupe's.
    var targets: [UInt64] {
        if selectMode { return Array(library.selected) }
        if let active = state.active { return [active] }
        return Array(library.selected)
    }

    /// Open a photo in the loupe.
    func open(_ photo: UInt64) {
        Task { await select(photo) }
        path.append(.photo(photo))
    }

    /// Which screen is showing (automation).
    var screenName: String {
        switch path.last {
        case .photo: editor.tool == nil ? "loupe" : "edit"
        case .commands: "commands"
        case .people, .person: "people"
        case .map: "map"
        case nil: sheet.map { "sheet:\($0.id)" } ?? "grid"
        }
    }

    /// The grid shows a plain album: photos can be added to and taken out of it by hand.
    var plainAlbum: UInt64? {
        guard let s = library.state?.source, s.kind == "album", let id = s.id else { return nil }
        return albums.node(id)?.isPlain == true ? id : nil
    }

    /// Ids as engine params.
    static func ids(_ ids: some Sequence<UInt64>) -> JSONValue {
        .array(ids.map { .int(Int64(clamping: $0)) })
    }
}
#endif
