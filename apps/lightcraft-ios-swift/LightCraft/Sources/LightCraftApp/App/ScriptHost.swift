#if os(iOS)
import Foundation
import LightCraftCore
import SwiftUI
import UIKit

/// The UI's half of automation (`LIGHTCRAFT_SCRIPT`, docs/ios-swift.md → Driving the app): the
/// engine answers `engine.*`, `render.*` and `host.*` itself and asks the app for the rest.
/// - `ui.screenshot {path}`: the window as a PNG.
/// - `ui.inspect`: the screen, the photo, the state and timings (`perf`).
/// - `ui.set {screen?: grid | photo | edit | crop | masking | remove | redEye | presets | info | commands |
///   people | map, photo?, span?: all | months | years, group?: light | color…, sheet?: albums |
///   filter | settings | export | sync | history | versions | keywords | about | superRes | createPreset | smartAlbum (id?) | allMetadata | none, select?,
///   faceBoxes?, tagFace?, beforeAfter?, visualizeSpots?, removeAI?, histogram?, cropGuide?}`: go there.
@Observable
final class ScriptHost {
    func answer(id: UInt64, method: String, params: JSONValue, model: AppModel) {
        Task {
            var reply: JSONValue = ["id": .int(Int64(clamping: id))]
            do {
                reply["result"] = try await perform(method, params, model)
            } catch {
                reply["error"] = .string(error.localizedDescription)
            }
            model.engine?.send("ui.reply", reply)
        }
    }

    private func perform(_ method: String, _ params: JSONValue, _ model: AppModel) async throws -> JSONValue {
        switch method {
        case "ui.screenshot":
            let path = params["path"].string ?? (NSTemporaryDirectory() as NSString).appendingPathComponent("screenshot.png")
            // let the last change reach the screen first
            try? await Task.sleep(for: .milliseconds(100))
            let (w, h) = try screenshot(to: path)
            return ["path": .string(path), "width": .int(Int64(clamping: w)), "height": .int(Int64(clamping: h))]
        case "ui.inspect":
            var photo: JSONValue = .null
            if case .photo(let id) = model.path.last {
                photo = .int(Int64(clamping: id))
            }
            var perf: JSONValue = [:]
            if let main = model.images.pictures["main"] {
                perf["lastRenderMs"] = .double(main.ms)
            }
            if let trip = model.lastDraftRoundTripMs {
                perf["draftRoundTripMs"] = .double(trip)
            }
            return [
                "screen": .string(model.screenName),
                "photo": photo,
                "photos": .int(Int64(clamping: model.library.count)),
                "state": (try? JSONValue.encode(model.state)) ?? .null,
                "perf": perf,
                "images": [
                    "thumbs": .int(Int64(model.images.pictures.keys.filter { $0.hasPrefix("thumb") }.count)),
                    "requested": .int(Int64(model.thumbRequests)),
                    "retained": .array(model.lastRetain.sorted().map { .int(Int64(clamping: $0)) }),
                    "slots": .array(model.images.pictures.keys.sorted().prefix(40).map { .string($0) }),
                    "failed": .object(model.images.failed.mapValues { .string($0) }),
                ],
            ]
        case "ui.set":
            if let span = params["span"].string, let s = Prefs.GridSpan(rawValue: span) {
                model.prefs.gridSpan = s
            }
            if let group = params["group"].string {
                model.prefs.editGroup = group
            }
            if let style = params["editStyle"].string, let s = Prefs.EditStyle(rawValue: style) {
                model.prefs.editStyle = s
            }
            if let select = params["select"].bool {
                model.selectMode = select
            }
            if let on = params["faceBoxes"].bool { model.prefs.faceBoxes = on }
            if let on = params["tagFace"].bool { model.editor.tagFace = on }
            if let on = params["beforeAfter"].bool { model.editor.beforeAfter = on }
            if let on = params["visualizeSpots"].bool { model.editor.visualizeSpots = on }
            if let on = params["removeAI"].bool { model.prefs.removeAI = on }
            if let on = params["histogram"].bool { model.prefs.showHistogram = on }
            if let guide = params["cropGuide"].string { model.prefs.cropGuide = guide }
            if let draw = params["cropDraw"].string { model.editor.cropDraw = draw == "none" ? nil : draw }
            if let on = params["filmstrip"].bool { model.prefs.filmstrip = on }
            if let on = params["navigator"].bool { model.prefs.navigator = on }
            if let on = params["gridInfo"].bool { model.prefs.gridInfo = on }
            if let mode = params["beforeAfterMode"].string { model.prefs.beforeAfterMode = mode }
            if let ref = params["reference"].int { model.reference = UInt64(clamping: ref) }
            if let on = params["referenceView"].bool { model.editor.referenceView = on }
            if let on = params["softProof"].bool { model.editor.softProof = on }
            if let on = params["searching"].bool { model.searching = on }
            if params["secondWindow"].bool == true { model.secondWindowAsked = true }
            if let k = params["painting"].string { model.painting = k.isEmpty ? nil : k }
            if let text = params["search"].string { model.searchText = text }
            if let command = params["sidebarCommand"].string { await model.arrangeSidebar(command, params["sidebarParams"]) }
            if let space = params["proofSpace"].string { model.editor.proofSpace = space }
            if let on = params["proofWarning"].bool { model.editor.proofWarning = on }
            if let cover = params["cover"].string { model.cover = Cover(rawValue: cover) }
            if let o = params["orientation"].string, let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first {
                // (the app turns itself: no rotating the simulator from outside)
                scene.requestGeometryUpdate(.iOS(interfaceOrientations: o == "landscape" ? .landscapeRight : .portrait)) { _ in }
            }
            switch params["screen"].string {
            case nil:
                break
            case "grid":
                model.sheet = nil
                model.editor.tool = nil
                model.path = []
            case "photo", "edit", "crop", "masking", "remove", "presets", "info", "redEye":
                let id = params["photo"].int.flatMap { UInt64(exactly: $0) } ?? model.library.loaded(at: 0)?.id
                guard let id else { throw ErrorBody(code: "badParams", message: "no photo") }
                model.sheet = nil
                if case .photo(let shown) = model.path.last, shown == id {
                } else {
                    model.path = [.photo(id)]
                    await model.select(id)
                }
                let screen = params["screen"].string ?? "photo"
                model.editor.tool = screen == "photo" ? nil : Tool(rawValue: screen)
            case "commands":
                model.path = [.commands]
            case "people":
                model.path = [.people] + (params["person"].string.map { [.person($0)] } ?? [])
            case "map":
                model.path = [.map]
            case "compare", "survey":
                let ids = params["ids"].array?.compactMap { $0.int.map { UInt64(clamping: $0) } } ?? Array(model.library.selected)
                model.path = [params["screen"].string == "compare" ? .compare(ids) : .survey(ids)]
            default:
                throw ErrorBody(code: "badParams", message: "`screen`: grid, photo, edit, crop, masking, remove, redEye, presets, info, commands, people or map")
            }
            if let sheet = params["sheet"].string {
                switch sheet {
                case "albums": model.sheet = .albums
                case "filter": model.sheet = .filter
                case "settings": model.sheet = .settings(params["section"].string)
                case "export": model.sheet = .export(model.state.active.map { [$0] } ?? [])
                case "sync": model.sheet = .syncStatus
                case "history": model.sheet = .history(versions: false)
                case "versions": model.sheet = .history(versions: true)
                case "keywords": model.sheet = .keywords
                case "about": model.sheet = .about
                case "superRes": model.sheet = .superRes
                case "createPreset": model.sheet = .createPreset
                case "smartAlbum": model.sheet = .smartAlbum(params["id"].int.map { UInt64(clamping: $0) }, parent: nil)
                case "allMetadata": model.sheet = .allMetadata
                case "renamePhotos": model.sheet = .renamePhotos(model.targets)
                case "captureTime": model.sheet = .captureTime(model.targets)
                case "labelNames": model.sheet = .labelNames
                case "autoStack": model.sheet = .autoStack
                case "keywordSets": model.sheet = .keywordSets
                case "metadataPresets": model.sheet = .metadataPresets(model.targets)
                case "contactSheet": model.sheet = .contactSheet(model.targets)
                case "shortcuts": model.sheet = .shortcuts
                case "editSidebar": model.sheet = .editSidebar
                case "lightroom":
                    // (a relative path is in the app's tmp folder, beside the script)
                    let p = params["path"].string ?? ""
                    model.sheet = .lightroom(p.hasPrefix("/") ? URL(fileURLWithPath: p) : URL(fileURLWithPath: NSTemporaryDirectory()).appending(path: p))
                case "cull": model.sheet = .cull(params["ids"].array?.compactMap { $0.int.map { UInt64(clamping: $0) } } ?? model.targets)
                case "merge.hdr", "merge.panorama", "merge.hdrPanorama":
                    let ids = params["ids"].array?.compactMap { $0.int.map { UInt64(clamping: $0) } } ?? model.targets
                    model.sheet = .merge(sheet, ids)
                default: model.sheet = nil
                }
            }
            return [:]
        default:
            throw ErrorBody(code: "unknownMethod", message: "`\(method)` isn't answered by the SwiftUI app yet")
        }
    }

    /// The key window as a PNG at `path`; its size in pixels.
    private func screenshot(to path: String) throws -> (Int, Int) {
        // the window in front (with a second window open, the main one may be behind it)
        let all = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let front = all.filter { $0.activationState == .foregroundActive }
        let scenes = front.isEmpty ? all : front
        guard let window = scenes.flatMap(\.windows).first(where: \.isKeyWindow) ?? scenes.first?.windows.first else {
            throw ErrorBody(code: "noWindow", message: "no window to capture")
        }
        let renderer = UIGraphicsImageRenderer(bounds: window.bounds)
        let image = renderer.image { _ in
            _ = window.drawHierarchy(in: window.bounds, afterScreenUpdates: true)
        }
        guard let png = image.pngData() else {
            throw ErrorBody(code: "encode", message: "the screenshot couldn't be encoded")
        }
        try png.write(to: URL(fileURLWithPath: path))
        return (Int(image.size.width * image.scale), Int(image.size.height * image.scale))
    }
}
#endif
