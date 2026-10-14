#if os(iOS)
import Foundation
import LightCraftCore
import SwiftUI
import UIKit

/// The UI's half of automation (`LIGHTCRAFT_SCRIPT`, docs/ios-swift.md → Driving the app): the
/// engine answers `engine.*`, `render.*` and `host.*` itself and asks the app for the rest.
/// - `ui.screenshot {path}`: the window as a PNG.
/// - `ui.inspect`: the screen, the photo, the state and timings (`perf`).
/// - `ui.set {screen?: grid | photo | edit | crop | masking | remove | presets | info | commands |
///   people | map, photo?, span?: all | months | years, group?: light | color…, sheet?: albums |
///   filter | settings | export | sync | history | versions | keywords | about | superRes | createPreset | none, select?,
///   faceBoxes?, tagFace?, beforeAfter?, visualizeSpots?, removeAI?}`: go there.
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
            if let select = params["select"].bool {
                model.selectMode = select
            }
            if let on = params["faceBoxes"].bool { model.prefs.faceBoxes = on }
            if let on = params["tagFace"].bool { model.editor.tagFace = on }
            if let on = params["beforeAfter"].bool { model.editor.beforeAfter = on }
            if let on = params["visualizeSpots"].bool { model.editor.visualizeSpots = on }
            if let on = params["removeAI"].bool { model.prefs.removeAI = on }
            switch params["screen"].string {
            case nil:
                break
            case "grid":
                model.sheet = nil
                model.editor.tool = nil
                model.path = []
            case "photo", "edit", "crop", "masking", "remove", "presets", "info":
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
                model.path = [.people]
            case "map":
                model.path = [.map]
            default:
                throw ErrorBody(code: "badParams", message: "`screen`: grid, photo, edit, crop, masking, remove, presets, info, commands, people or map")
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
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
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
