#if os(macOS)
import AppKit
import LightCraftCore

/// The Mac app's windows, for the automation (`LIGHTCRAFT_SCRIPT`): which one a screenshot means,
/// and the picture of it.
enum WindowCapture {
    /// A window the script names: by part of its identifier or title (`main`, `settings`, `second`,
    /// `cover`, `sheet`: the one that is open over a window), or the key window (an unattended run has none) else the biggest one on screen.
    static func window(named name: String?) -> NSWindow? {
        let visible = NSApp.windows.filter { $0.isVisible && !($0 is NSPanel) }
        // (a sheet is the page that slid down from a window: its own window, attached to that one)
        if name?.lowercased() == "sheet" { return NSApp.windows.compactMap(\.attachedSheet).first }
        if let name = name?.lowercased(), !name.isEmpty, name != "front" {
            return visible.first { ($0.identifier?.rawValue.lowercased().contains(name) ?? false) || $0.title.lowercased().contains(name) }
        }
        return NSApp.keyWindow ?? visible.max { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }
    }

    /// Every window with what a script needs to name it (`ui.inspect`).
    static func list() -> JSONValue {
        // (the sheets open over the windows too: `sheet` names them)
        let windows = NSApp.windows.filter { !($0 is NSPanel) }
        return .array((windows + windows.compactMap(\.attachedSheet)).map { w in
            [
                "id": .string(w.identifier?.rawValue ?? (w.sheetParent != nil ? "sheet" : "")),
                "title": .string(w.title),
                "visible": .bool(w.isVisible),
                "key": .bool(w.isKeyWindow),
                "fullScreen": .bool(w.styleMask.contains(.fullScreen)),
                "size": [.double(w.frame.width), .double(w.frame.height)],
            ]
        })
    }

    /// The main menu as it is now (`ui.inspect {menus: true}`): each menu's items with their key
    /// equivalents (⌃⌥⇧⌘ and the key), and `enabled: false` when one is off.
    static func menus() -> JSONValue {
        func items(of menu: NSMenu) -> JSONValue {
            .array(menu.items.map { item in
                if item.isSeparatorItem { return .string("—") }
                var entry: [String: JSONValue] = ["title": .string(item.title)]
                if !item.keyEquivalent.isEmpty {
                    let m = item.keyEquivalentModifierMask
                    let glyphs = (m.contains(.control) ? "⌃" : "") + (m.contains(.option) ? "⌥" : "") + (m.contains(.shift) ? "⇧" : "") + (m.contains(.command) ? "⌘" : "")
                    entry["key"] = .string(glyphs + item.keyEquivalent.uppercased())
                }
                if !item.isEnabled { entry["enabled"] = .bool(false) }
                if let sub = item.submenu { entry["items"] = items(of: sub) }
                return .object(entry)
            })
        }
        // (a menu fills in right before it opens: ask for that, as AppKit does)
        for item in NSApp.mainMenu?.items ?? [] { item.submenu?.update() }
        return .array((NSApp.mainMenu?.items ?? []).map { .object(["title": .string($0.submenu?.title ?? $0.title), "items": $0.submenu.map(items(of:)) ?? .array([])]) })
    }

    /// `window` as a PNG at `path`, title bar and toolbar included: its size in pixels. As the window
    /// server composites it (glass, materials, the sidebar's table and the toolbar as they look),
    /// which an app may do to its own windows without Screen Recording permission; or, with `views`
    /// (and when that doesn't work), drawn from its views, which comes out flatter: materials light,
    /// lists and thumbnails sometimes empty.
    static func capture(_ window: NSWindow, to path: String, views: Bool = false) throws -> (Int, Int) {
        let rep: NSBitmapImageRep
        if !views, let image = windowServerImage(of: window) {
            rep = NSBitmapImageRep(cgImage: image)
        } else {
            guard let content = window.contentView else { throw ErrorBody(code: "noWindow", message: "the window has no content") }
            let view = content.superview ?? content
            guard let drawn = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
                throw ErrorBody(code: "encode", message: "the window couldn't be drawn")
            }
            view.cacheDisplay(in: view.bounds, to: drawn)
            rep = drawn
        }
        guard let png = rep.representation(using: .png, properties: [:]) else {
            throw ErrorBody(code: "encode", message: "the screenshot couldn't be encoded")
        }
        try png.write(to: URL(fileURLWithPath: path))
        return (rep.pixelsWide, rep.pixelsHigh)
    }

    /// `CGWindowListCreateImage` is obsolete in the SDK (ScreenCaptureKit replaces it, with a
    /// permission prompt) but still in the system; it is looked up by name. `nil` when it is not
    /// there or gives nothing (the window is not on a display that is awake).
    private static func windowServerImage(of window: NSWindow) -> CGImage? {
        typealias Create = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
        // RTLD_DEFAULT
        guard window.windowNumber > 0, let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else { return nil }
        let create = unsafeBitCast(symbol, to: Create.self)
        let includingWindow: UInt32 = 1 << 3  // kCGWindowListOptionIncludingWindow
        let ignoreFraming: UInt32 = 1 << 0  // kCGWindowImageBoundsIgnoreFraming
        let bestResolution: UInt32 = 1 << 3  // kCGWindowImageBestResolution
        return create(.null, includingWindow, UInt32(window.windowNumber), ignoreFraming | bestResolution)?.takeRetainedValue()
    }
}
#endif
