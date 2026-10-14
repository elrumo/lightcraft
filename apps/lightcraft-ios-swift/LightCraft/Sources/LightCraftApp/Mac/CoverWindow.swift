#if os(macOS)
import AppKit
import SwiftUI

/// The slideshow and Lights Out on a Mac: a window of their own that goes full screen when it
/// opens (the iPhone's full-screen cover) and closes when the show ends.
struct CoverWindow: View {
    static let id = "cover"
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            switch model.cover {
            case .slideshow: SlideshowView()
            case .lightsOut: LightsOutView()
            case nil: Color.black
            }
        }
        .background(Color.black)
        .background(FullScreenOnOpen())
        .focusable()
        .focusEffectDisabled()
        .onExitCommand { model.cover = nil }
        // (closed with its own ✕, or by tapping Lights Out: the app's side lets go of it too)
        .onDisappear { model.cover = nil }
    }
}

/// Puts the window it is in into full screen once, when it appears.
private struct FullScreenOnOpen: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView { Anchor() }
    func updateNSView(_ view: NSView, context: Context) {}

    private final class Anchor: NSView {
        override func viewDidMoveToWindow() {
            guard let window, !window.styleMask.contains(.fullScreen) else { return }
            DispatchQueue.main.async { window.toggleFullScreen(nil) }
        }
    }
}
#endif
