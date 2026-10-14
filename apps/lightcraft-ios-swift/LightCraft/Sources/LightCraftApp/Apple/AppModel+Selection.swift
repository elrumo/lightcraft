#if os(iOS)
import Foundation
import LightCraftCore

/// Which provider selects objects, and the flow from a tap to a stored mask: the engine renders the
/// photo as a selection model sees it (`mask.inputImage`), a provider answers with a coverage
/// picture, and the engine stores it as the mask (`mask.add`, `mask.addComponent`,
/// `mask.setObject`). Swift does no coordinate or image math: the picture lines up with the one the
/// engine gave.
extension AppModel {
    /// What a selection tap or Select Subject runs on, by the Selection setting; `nil` for Classic
    /// (LightCraft's own guess, no model). `tap`: a provider that takes taps (Select Subject takes
    /// none, so it only asks for the foreground one).
    func selectionProvider(taps: Bool) async -> (any SelectionProvider)? {
        if ProcessInfo.processInfo.environment["LIGHTCRAFT_FAKE_SEGMENTER"] == "1" { return EllipseSelectionProvider() }
        if prefs.selectionEngine == "classic" { return nil }
        if taps, #available(iOS 27.0, *), await VisionIterativeProvider.isReady() { return VisionIterativeProvider() }
        return VisionForegroundProvider()
    }

    /// The photo for a selection model: the PNG `mask.inputImage` renders.
    private func selectionInput() async -> Data? {
        guard let reply = await run("mask.inputImage"), let b64 = reply["png"].string, let png = Data(base64Encoded: b64) else { return nil }
        return png
    }

    /// Ask a provider, showing what went wrong. `nil` when nothing came back.
    private func ask(_ provider: any SelectionProvider, png: Data, include: [SelectionTap], exclude: [SelectionTap]) async -> SelectionBitmap? {
        editor.selecting = true
        defer { editor.selecting = false }
        do {
            return try await provider.select(png: png, include: include, exclude: exclude)
        } catch SelectionError.nothingFound {
            show(String(localized: "Nothing to select there. Tap the object itself."), error: true)
        } catch {
            show(error.localizedDescription, error: true)
        }
        return nil
    }

    /// Select Subject: a mask of what stands out, from Apple's foreground model. Classic, or a photo
    /// with no foreground, makes LightCraft's own guess instead.
    func selectSubject() async {
        if let provider = await selectionProvider(taps: false), let png = await selectionInput() {
            editor.selecting = true
            let bitmap = try? await provider.select(png: png, include: [], exclude: [])
            editor.selecting = false
            if let bitmap {
                await run("mask.add", ["kind": "object", "name": .string(String(localized: "Subject")), "bitmap": bitmap.json])
                return
            }
        }
        await run("mask.add", ["kind": "subject"])
    }

    /// A tap on the photo with the Object tool: the object there joins the selection (or leaves it,
    /// with Subtract). The taps so far are kept in the mask, so Undo and sync carry them.
    func selectObject(at n: (x: Double, y: Double)) async {
        guard !editor.selecting else { return }
        let tap = SelectionTap(x: n.x, y: n.y)
        // the Object part the taps refine: the selected mask's last one, unless the next tap starts a part
        let mask = MaskList.selected(self)
        let part = editor.objectOp == nil ? mask?["components"].array?.lastIndex { $0["shape"]["kind"] == "object" } : nil
        let shape = part.flatMap { mask?["components"][$0]["shape"] }
        var include = SelectionTap.list(shape?["hint"] ?? .null)
        var exclude = SelectionTap.list(shape?["exclude"] ?? .null)
        if editor.objectSubtract { exclude.append(tap) } else { include.append(tap) }
        guard !include.isEmpty else {
            show(String(localized: "First tap the object you want to select."))
            return
        }
        guard let provider = await selectionProvider(taps: true), let png = await selectionInput() else {
            show(String(localized: "Object selection is off while Selection is set to Classic. Change it in Settings ▸ AI Models."), error: true)
            return
        }
        guard let bitmap = await ask(provider, png: png, include: include, exclude: exclude) else { return }
        var p: [String: JSONValue] = ["bitmap": bitmap.json, "points": SelectionTap.json(include), "exclude": SelectionTap.json(exclude)]
        if let part, let mask {
            p["id"] = mask["id"]
            p["component"] = .int(Int64(part))
            await run("mask.setObject", .object(p))
        } else if let op = editor.objectOp, let mask {
            p["kind"] = "object"
            p["op"] = .string(op)
            p["id"] = mask["id"]
            await run("mask.addComponent", .object(p))
        } else {
            p["kind"] = "object"
            p["name"] = .string(String(localized: "Object"))
            await run("mask.add", .object(p))
        }
        editor.objectOp = nil
    }
}
#endif
