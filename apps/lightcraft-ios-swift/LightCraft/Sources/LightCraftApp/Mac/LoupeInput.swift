#if os(macOS)
import AppKit
import SwiftUI

/// What a loupe does with the Mac's wheel and trackpad. `ZoomablePhoto` makes one of these for the
/// photo it shows and `MacInput` calls it for the events whose pointer is over that photo.
struct LoupeHandlers {
    /// A wheel or two-finger scroll: how far it moved (points of the content's way), where the pointer
    /// is in the loupe, whether the numbers are a trackpad's (points, else a wheel's lines), and, when ⌘
    /// or ⌃ is held to zoom, how far it zooms (up is in). True when the loupe used it; false leaves
    /// it to what is under it (a fitted photo's sideways swipe is the pager's).
    var scroll: (_ delta: CGSize, _ at: CGPoint, _ precise: Bool, _ zoomDelta: CGFloat?) -> Bool = { _, _, _, _ in false }
    /// A trackpad pinch: its change since the last event (0.1 is 10 %), and where the pointer is.
    var magnify: (_ magnification: CGFloat, _ at: CGPoint) -> Void = { _, _ in }
    /// A two-finger double tap on the trackpad.
    var smartMagnify: (_ at: CGPoint) -> Void = { _ in }
}

/// An invisible view over the loupe's canvas, there to say where the loupe is on screen. It never
/// takes the pointer: `MacInput` looks at the wheel and the trackpad before the window does and
/// asks which loupe is under them.
struct LoupeAnchor: NSViewRepresentable {
    let handlers: LoupeHandlers

    func makeNSView(context: Context) -> LoupeAnchorView {
        let view = LoupeAnchorView()
        view.handlers = handlers
        LoupeAnchors.add(view)
        return view
    }

    func updateNSView(_ view: LoupeAnchorView, context: Context) {
        view.handlers = handlers
    }

    static func dismantleNSView(_ view: LoupeAnchorView, coordinator: ()) {
        LoupeAnchors.remove(view)
    }
}

final class LoupeAnchorView: NSView {
    var handlers = LoupeHandlers()

    /// (top left is the origin, as the canvas's own points are)
    override var isFlipped: Bool { true }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// The loupes on screen, the newest last (a page sliding in has its loupe before the old one goes).
@MainActor
enum LoupeAnchors {
    private final class Weak {
        weak var view: LoupeAnchorView?
        init(_ view: LoupeAnchorView) { self.view = view }
    }

    private static var all: [Weak] = []

    static func add(_ view: LoupeAnchorView) {
        all.removeAll { $0.view == nil }
        all.append(Weak(view))
    }

    static func remove(_ view: LoupeAnchorView) {
        all.removeAll { $0.view == nil || $0.view === view }
    }

    /// The newest loupe on screen (the script's).
    static var current: LoupeAnchorView? {
        all.last { $0.view?.window != nil }?.view
    }

    /// The loupe under the pointer of `event`, and the pointer in its points.
    static func target(for event: NSEvent) -> (view: LoupeAnchorView, point: CGPoint)? {
        guard let window = event.window else { return nil }
        for box in all.reversed() {
            guard let view = box.view, view.window === window, !view.isHiddenOrHasHiddenAncestor else { continue }
            let point = view.convert(event.locationInWindow, from: nil)
            if view.bounds.contains(point) { return (view, point) }
        }
        return nil
    }
}
#endif
