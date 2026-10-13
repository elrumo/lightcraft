#if os(iOS)
import Foundation
import LightCraftCore
import SwiftUI
import UIKit

/// The UI's half of automation (`LIGHTCRAFT_SCRIPT`, docs/ios-swift.md → Driving the app): the
/// engine answers `engine.*`, `render.*` and `host.*` itself and asks the app for the rest.
/// - `ui.screenshot {path}`: the window as a PNG.
/// - `ui.inspect`: the screen, the photo, the state and timings (`perf`).
/// - `ui.set {screen: grid | photo | edit | commands, photo?}`: go there.
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
                "photos": .int(Int64(clamping: model.library.photos.count)),
                "state": (try? JSONValue.encode(model.state)) ?? .null,
                "perf": perf,
            ]
        case "ui.set":
            switch params["screen"].string {
            case "grid":
                model.editing = false
                model.path = []
            case "photo", "edit":
                let id = params["photo"].int.flatMap { UInt64(exactly: $0) } ?? model.library.photos.first?.id
                guard let id else { throw ErrorBody(code: "badParams", message: "no photo") }
                model.path = [.photo(id)]
                model.editing = params["screen"].string == "edit"
            case "commands":
                model.editing = false
                model.path = [.commands]
            default:
                throw ErrorBody(code: "badParams", message: "`screen`: grid, photo, edit or commands")
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
