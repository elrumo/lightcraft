import CoreGraphics
import Foundation
import LightCraftCore
import LightCraftImaging
import Observation
import SwiftUI

/// A screen pushed over the grid.
nonisolated enum Route: Hashable {
    case photo(UInt64)
    case commands
    case people
    case person(String)
    case map
    case compare([UInt64])
    case survey([UInt64])
}

/// What a file or folder is picked for.
nonisolated enum FilePick: Hashable {
    /// A GPS track log (.gpx) to tag `ids` from.
    case tracklog([UInt64])
    /// A folder the missing photos may be in.
    case findMissing
    /// The file of a photo whose original has moved.
    case locate(UInt64)
    /// A Lightroom Classic catalog's folder (with its photos, or above them).
    case lightroom
}

/// What covers the whole screen.
nonisolated enum Cover: String, Identifiable {
    case slideshow, lightsOut, onboarding
    var id: String { rawValue }
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
    case superRes
    case createPreset
    case smartAlbum(UInt64?, parent: UInt64?)
    case newSmartAlbumFromView
    case allMetadata
    case activity
    case renamePhotos([UInt64])
    case captureTime([UInt64])
    case labelNames
    case autoStack
    case keywordSets
    case metadataPresets([UInt64])
    case contactSheet([UInt64])
    case merge(String, [UInt64])
    case shortcuts
    case editSidebar
    case cull([UInt64])
    case lightroom(URL)
    case shareFiles([String])
    case cameraImport

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
        case .superRes: "superRes"
        case .createPreset: "createPreset"
        case .smartAlbum: "smartAlbum"
        case .newSmartAlbumFromView: "newSmartAlbumFromView"
        case .allMetadata: "allMetadata"
        case .activity: "activity"
        case .renamePhotos: "renamePhotos"
        case .captureTime: "captureTime"
        case .labelNames: "labelNames"
        case .autoStack: "autoStack"
        case .keywordSets: "keywordSets"
        case .metadataPresets: "metadataPresets"
        case .contactSheet: "contactSheet"
        case .merge(let command, _): command
        case .shortcuts: "shortcuts"
        case .editSidebar: "editSidebar"
        case .cull: "cull"
        case .lightroom: "lightroom"
        case .shareFiles: "shareFiles"
        case .cameraImport: "cameraImport"
        }
    }
}

/// A short message over the screen.
nonisolated struct Toast: Equatable {
    var text: String
    var error = false
    var id = UUID()
}

/// Tells the App that what the menu bar shows has changed. A Mac's menu bar does not follow what an
/// @Observable model changes (not from a Commands body, nor from the App's), but it does follow an
/// ObservableObject the App observes: the model bumps this when the menus' data moves.
final class MenuChanges: ObservableObject {
    @Published private(set) var revision = 0
    func bump() { revision &+= 1 }
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
    /// The screen can show the range above SDR white, as the engine was last told (`render.hdrDisplay`):
    /// an HDR edit's loupe then comes as an HDR picture (`ImageStore.Picture.hdr`).
    private(set) var hdrDisplay = false
    var path: [Route] = []
    var sheet: Sheet?
    /// The App's word that the menus changed (`MenuChanges`).
    let menuChanges = MenuChanges()
    /// The page of the Mac's Settings window that is showing (`general`, `import`, `sync`…).
    var settingsTab = "general"
    /// The menu bar's commands: egui's UI commands (`host.uiCommands`) and the engine's with a menu.
    var uiCommands: [UICommand] = []
    var menuEngine: [CommandInfo] = []
    /// The photo Reference View shows beside the one being edited.
    var reference: UInt64?
    /// A file or folder the app is asking for (RootView's picker), and what it is for.
    var picker: FilePick?
    /// The slideshow or Lights Out, over everything.
    var cover: Cover?
    /// Settings ▸ Show Welcome was tapped: the settings sheet closes first, then `RootView` shows the welcome.
    var welcomeAsked = false
    /// On an iPad, whether the albums column shows beside the grid.
    var sidebar: NavigationSplitViewVisibility = .all
    /// What the app knows about its window and its hinge (`ui.inspect` → `layout`; `Duo.swift`).
    let duo = DuoFacts()
    /// The photo the loupe is showing (its pager's page). A screen the system rebuilds, as a fold or a rotation
    /// does, opens on this one, not on the photo it was first pushed for.
    var shown: UInt64?
    /// A scroll of the grid that automation asked for (`ui.set {scrollGrid}`): the library screen turns it into a jump.
    var gridJump: Int?
    /// The albums page's sections below My Photos, in order (`host.sidebar`).
    var sidebarSections: [SidebarSectionState] = SidebarSectionState.usual
    /// Photos are being chosen in the grid (Select).
    var selectMode = false
    /// The engine is being opened.
    private var starting = false
    /// Open the demo library (the way out of a library that won't open).
    private var wantsDemo = false
    /// Window ▸ Second Window was asked for (RootView opens it).
    var secondWindowAsked = false
    /// Main windows on screen (none when iPadOS brings back only the second window).
    var mainWindows = 0
    /// The app's one model: every window's, and a scene's SwiftUI doesn't make (the external display's).
    static let shared = AppModel()
    /// The grid's search field is open (⌘F, Edit ▸ Find…), and what is typed in it.
    var searching = false
    var searchText = ""
    /// The Keyword Painter's keyword: a tap on a photo in the grid gives it the keyword or takes it
    /// away (`keyword.toggle`); and whether it is being asked for.
    var painting: String?
    var askPainter = false
    /// Choosing photos to add to this album (its + ▸ LightCraft).
    var addingTo: UInt64?
    /// A dialog the engine asked for (`syncChoice`).
    var asking: String?
    /// Ask before moving these photos to Recently Deleted.
    var confirmDelete: [UInt64]?
    /// The album or folder whose Delete was chosen, waiting for a yes (the albums sheet asks).
    var albumToDelete: AlbumNode?
    /// The import review's files (`import.scan`), while it is open.
    var importReview: ImportReviewData?
    /// This build has AI Super Resolution (`enhance.model.status`).
    private(set) var superResAvailable = false
    let prefs = Prefs()
    let images = ImageStore()
    let library = LibraryModel()
    let albums = AlbumsModel()
    let editor = EditorModel()
    let script = ScriptHost()
    /// Cameras and card readers plugged in (ImageCaptureCore), for Add Photos ▸ Connected Camera/Card.
    let cameras = Cameras()

    /// The loupe's last render request: drafts during a slider drag go at the same size.
    private(set) var lastView: (photo: UInt64, width: Int, height: Int)?
    /// Its zoom: drafts keep the window there is.
    private var lastZoom: ZoomRequest?
    /// When the last draft was asked for (slider → pixels on screen: `ui.inspect` → `perf`).
    private var draftAskedAt: ContinuousClock.Instant?
    private(set) var lastDraftRoundTripMs: Double?
    private var visibleThumbs: Set<UInt64> = []
    private var retainScheduled = false
    /// Grid indexes on screen (pages there are read again when photos change).
    var visibleRange: Range<Int>?

    // MARK: start, lifecycle

    func start() async {
        // (a second window of the app starts it too)
        guard engine == nil, startError == nil, !starting else { return }
        starting = true
        defer { starting = false }
        // the device's name on the sync server (read by the engine when signing in)
        setenv("LIGHTCRAFT_DEVICE", Platform.deviceName, 0)
        let openConfig = config()
        #if os(iOS)
        // the first launch's welcome, over the opening library (not under a script or the demo: the tour's screenshots)
        if !prefs.onboarded, openConfig.script == nil, !openConfig.demo {
            cover = .onboarding
        }
        #endif
        do {
            let (engine, opened) = try await Engine.open(openConfig)
            self.engine = engine
            self.opened = opened
            if let warning = opened.warnings.first {
                show(warning)
            }
            Task { await listen(engine) }
            updateHdrDisplay()
            state = (try? await engine.request("host.state").decode(HostState.self)) ?? state
            await library.reload(engine, count: state.visibleCount)
            await albums.reload(engine)
            superResAvailable = (try? await engine.execute("enhance.model.status"))?["available"].bool == true
            // the models switched off last time stay off
            for id in prefs.modelsDisabled {
                _ = try? await engine.execute("models.setEnabled", ["id": .string(id), "enabled": false])
            }
            // a camera or card plugged in: its import (Settings ▸ Import ▸ Cameras and cards), or a word
            cameras.connected = { [weak self] device in
                guard let self else { return }
                if self.prefs.importOnConnect, self.sheet == nil, self.cover == nil, self.path.isEmpty {
                    self.sheet = .cameraImport
                } else {
                    self.show(String(localized: "\(device.name ?? "") connected"))
                }
            }
            cameras.start()
        } catch {
            startError = error.localizedDescription
        }
    }

    /// Start again after the library wouldn't open: as it was (the other LightCraft may be closed
    /// now), on another library, or on the demo library (nothing is saved there).
    func retryStart(library: String? = nil, demo: Bool = false) {
        if let library { prefs.libraryPath = library }
        wantsDemo = demo
        startError = nil
        Task { await start() }
    }

    private func config() -> OpenConfig {
        let env = ProcessInfo.processInfo.environment
        let nonEmpty = { (key: String) in env[key].flatMap { $0.isEmpty ? nil : $0 } }
        return OpenConfig(
            home: NSHomeDirectory(),
            tmp: Platform.tmpFolder,
            // (the environment first, then the library chosen when the default one wouldn't open)
            library: nonEmpty("LIGHTCRAFT_LIBRARY") ?? prefs.libraryPath,
            demo: nonEmpty("LIGHTCRAFT_DEMO") != nil || wantsDemo,
            deviceName: Platform.deviceName,
            script: nonEmpty("LIGHTCRAFT_SCRIPT"),
            profile: Platform.profile
        )
    }

    /// The app became active, went to the background… (`host.lifecycle`: save, pause the GPU, free memory).
    func lifecycle(_ event: String) {
        engine?.send("host.lifecycle", ["event": .string(event)])
        // (another screen, or a setting that changed meanwhile)
        if event == "active" {
            updateHdrDisplay()
            #if os(iOS)
            Task { await importShared() }
            #endif
        }
    }

    /// Tell the engine whether the screen has HDR headroom (EDR: iPhone 12 and later, recent iPads), so that
    /// an HDR edit's loupe comes as an HDR picture; asked again when the app becomes active. The
    /// environment variable `LIGHTCRAFT_HDR_DISPLAY` (`1` or `0`) decides for the simulator and scripts.
    func updateHdrDisplay() {
        let forced = ProcessInfo.processInfo.environment["LIGHTCRAFT_HDR_DISPLAY"]
        #if os(iOS)
        // (the scene on screen: an iPhone Duo's scene moves between its two displays as it folds and unfolds, and
        // the display it is on is the one to ask)
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let screen = (scenes.first { $0.activationState == .foregroundActive } ?? scenes.first)?.screen
        let headroom = screen?.potentialEDRHeadroom ?? 1
        #else
        // (a Mac's built-in display and its XDR displays have it)
        let headroom = NSScreen.main?.maximumPotentialExtendedDynamicRangeColorComponentValue ?? 1
        #endif
        let on = forced.map { $0 == "1" } ?? (headroom > 1)
        // (not before the engine is there: `start()` asks again once it is, and what is not sent is not recorded)
        guard let engine, on != hdrDisplay else { return }
        hdrDisplay = on
        engine.send("render.hdrDisplay", ["on": .bool(on)])
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
            images.put(info, image, hdr: pixels.hdr?.cgImage())
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
            Platform.copy(text)
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
        // (Undo and Redo name the step)
        if old.undo != new.undo || old.redo != new.redo { menuChanges.bump() }
        // what isn't drawn live during a drag (menus, the grid under the photo, lists, previews)
        // catches up when it ends: at every step it would keep the engine from the drafts
        if new.interaction == nil, settledRevision != new.catalogRevision {
            settledRevision = new.catalogRevision
        }
        guard let engine else { return }
        if new.libraryGeneration != old.libraryGeneration {
            // another library (signing out emptied this one): nothing on screen may name the old one's photos
            images.forgetAll()
            path = []
            resetForNewLibrary()
            Task {
                await library.reload(engine, count: new.visibleCount, forget: true)
                await albums.reload(engine)
                await editor.reload(self)
            }
            return
        }
        // a drag (slider, crop, brush) changes the catalog at every step: the grid follows once it ends
        let changed = new.catalogRevision != old.catalogRevision || (old.interaction != nil && new.interaction == nil)
        if new.catalogRevision != old.catalogRevision, new.interaction == nil {
            images.dropRegion()
        }
        if new.visibleGen != old.visibleGen {
            Task { await library.reload(engine, count: new.visibleCount) }
        } else if changed && new.interaction == nil {
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
            if new.interaction == nil { albums.scheduleReload(engine) }
        }
    }

    /// What holds the old library's photo, album or person ids goes: pages and covers over the grid, the choice of photos,
    /// a search being typed, the questions waiting for a yes. (Settings and the sync pages stay: they are where a sign-out
    /// is made, and they show no photos.)
    private func resetForNewLibrary() {
        switch sheet {
        case .none, .settings, .about, .shortcuts, .syncStatus, .syncChoice, .activity, .cameraImport: break
        default: sheet = nil
        }
        if cover != .onboarding { cover = nil }
        selectMode = false
        addingTo = nil
        painting = nil
        askPainter = false
        searching = false
        searchText = ""
        confirmDelete = nil
        albumToDelete = nil
        importReview = nil
        reference = nil
        picker = nil
    }

    /// The catalog's revision, held while a drag (a slider, the crop, a brush) is under way.
    private(set) var settledRevision: UInt64 = 0

    // MARK: pictures

    /// Thumbnails asked for and retain lists sent (automation: `ui.inspect`).
    private(set) var thumbRequests = 0
    private(set) var lastRetain: [UInt64] = []

    /// Ask for a grid thumbnail (cheap to repeat: the engine answers only what changed).
    func requestThumb(_ photo: UInt64, edge: Int) {
        thumbRequests += 1
        engine?.send("render.thumb", ["photo": .int(Int64(clamping: photo)), "edge": .int(Int64(clamping: edge))])
    }

    /// A thumbnail on screen outside the grid (a cover, a marker…): kept by `render.retain`, asked for.
    func wantThumb(_ photo: UInt64, edge: Int) {
        thumbAppeared(photo)
        requestThumb(photo, edge: edge)
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
            lastRetain = Array(visibleThumbs)
            engine?.send("render.retain", ["thumbs": .array(visibleThumbs.map { .int(Int64(clamping: $0)) })])
            images.trimThumbs(keeping: visibleThumbs)
        }
    }

    /// Ask for the loupe's picture of `photo` at `width` × `height` pixels (the photo fits inside).
    func requestView(_ photo: UInt64, width: Int, height: Int, draft: Bool = false, applyCrop: Bool = true, zoom: ZoomRequest? = nil) {
        guard let engine, width >= 8, height >= 8 else { return }
        if !draft {
            lastView = (photo, width, height)
            lastZoom = zoom
        }
        if draft {
            draftAskedAt = .now
        }
        var p: [String: JSONValue] = ["photo": .int(Int64(clamping: photo)), "width": .int(Int64(clamping: width)), "height": .int(Int64(clamping: height)), "draft": .bool(draft), "applyCrop": .bool(applyCrop)]
        if let viewOverlay { p["overlay"] = viewOverlay }
        if let viewProof { p["proof"] = viewProof }
        // zoomed in: the window on screen comes sharp, at the zoom scale (`region`)
        if let zoom = draft ? lastZoom : zoom {
            p["zoom"] = ["drawn": .int(Int64(zoom.drawn)), "visible": .array(zoom.visible.map { .double(Double($0) / 1000) })]
        }
        engine.send("render.view", .object(p))
    }

    /// Soft proofing (`render.view {proof}`).
    var viewProof: JSONValue? {
        editor.softProof ? ["space": .string(editor.proofSpace), "destWarning": .bool(editor.proofWarning)] : nil
    }

    /// What the loupe draws over the photo: in Masking, the selected mask (Show Overlay); in Remove,
    /// Visualize Spots.
    var viewOverlay: JSONValue? {
        if editor.tool == .remove, editor.visualizeSpots {
            return ["kind": "spots", "threshold": .double(editor.spotsThreshold)]
        }
        if editor.tool == .edit, editor.visualizeHdr, editor.hdr {
            return ["kind": "hdr"]
        }
        guard editor.tool == .masking else { return nil }
        if editor.maskOverlayAll {
            return [
                "kind": "allMasks", "view": .string(prefs.maskOverlayMode),
                "color": .array(prefs.maskOverlayColor.map { .int(Int64($0)) }), "opacity": .double(prefs.maskOverlayOpacity),
            ]
        }
        let shown = prefs.maskOverlayVisibility == "always" || (prefs.maskOverlayVisibility == "auto" && !editor.maskAdjusted)
        guard shown, let mask = state.activeMask else { return nil }
        return [
            "kind": "mask", "id": .int(Int64(mask)), "view": .string(prefs.maskOverlayMode),
            "color": .array(prefs.maskOverlayColor.map { .int(Int64($0)) }), "opacity": .double(prefs.maskOverlayOpacity),
        ]
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

    /// A preview of the active photo with a preset, a profile or a version (`render.variant`): the
    /// slot its picture comes in.
    func requestVariant(_ which: JSONValue, edge: Int = 160) async -> String? {
        guard let engine else { return nil }
        var p = which
        p["edge"] = .int(Int64(edge))
        return try? await engine.request("render.variant", p)["slot"].string
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
        buzz(.light)
    }

    func redo() {
        perform("edit.redo")
        buzz(.light)
    }

    /// The haptic for what just happened, which RootView plays (egui's `haptics::tap` sites).
    private(set) var haptic = Haptic()

    func buzz(_ kind: Haptic.Kind) {
        haptic = Haptic(kind: kind, seq: haptic.seq &+ 1)
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

    /// Ask the host (`render.*`, `host.*`); a refusal becomes the message shown.
    @discardableResult
    func request(_ method: String, _ params: JSONValue = [:]) async -> JSONValue? {
        guard let engine else { return nil }
        do {
            return try await engine.request(method, params)
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
        shown = photo
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
        case .compare: "compare"
        case .survey: "survey"
        case nil: sheet.map { "sheet:\($0.id)" } ?? "grid"
        }
    }

    /// The grid shows a plain album: photos can be added to and taken out of it by hand.
    var plainAlbum: UInt64? {
        guard let s = library.state?.source, s.kind == "album", let id = s.id else { return nil }
        return albums.node(id)?.isPlain == true ? id : nil
    }

    // MARK: import

    /// Scan `paths` (staged copies of what was picked) and open the review.
    func reviewImport(_ paths: [String]) async {
        guard let engine else { return }
        importReview = nil
        sheet = .importReview
        do {
            let r = try await engine.request("import.scan", ["paths": .array(paths.map { .string($0) })])
            let candidates = r["candidates"].array ?? []
            if candidates.isEmpty {
                sheet = nil
                show(String(localized: "No photos found"))
            } else {
                importReview = ImportReviewData(candidates: candidates)
            }
        } catch {
            sheet = nil
            show(error.localizedDescription, error: true)
        }
    }

    /// Import what the review chose (`import.start`), in the background; say how it went.
    func importChosen(_ params: JSONValue) async {
        guard let engine else { return }
        do {
            let r = try await engine.request("import.start", params)
            let n = { (k: String) in Int(r[k].int ?? 0) }
            var parts: [String] = []
            if r["cancelled"].bool == true {
                parts.append(String(localized: "Import cancelled · \(n("imported")) added"))
            } else if n("imported") == 0, n("restored") > 0 {
                parts.append(String(localized: "Restored \(n("restored")) from Recently Deleted"))
            } else {
                parts.append(String(localized: "Imported \(n("imported")) photos"))
            }
            if n("duplicates") > 0 { parts.append(String(localized: "\(n("duplicates")) duplicates skipped")) }
            if n("failed") > 0 { parts.append(String(localized: "\(n("failed")) not readable")) }
            show(parts.joined(separator: " · "), error: n("failed") > 0 && n("imported") == 0)
        } catch {
            show(error.localizedDescription, error: true)
        }
    }

    /// Ids as engine params.
    static func ids(_ ids: some Sequence<UInt64>) -> JSONValue {
        .array(ids.map { .int(Int64(clamping: $0)) })
    }
}
/// A haptic to play: its kind, and a count so the same kind twice plays twice.
nonisolated struct Haptic: Equatable {
    enum Kind { case selection, light, medium, warning }
    var kind = Kind.light
    var seq = 0

    var feedback: SensoryFeedback {
        switch kind {
        case .selection: .selection
        case .light: .impact(weight: .light)
        case .medium: .impact(weight: .medium)
        case .warning: .warning
        }
    }
}
