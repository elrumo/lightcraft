#if os(iOS)
import LightCraftCore
import SwiftUI
import UIKit

/// Window ▸ Second Window (Lightroom Classic's, as a Loupe): the photo chosen in the main window,
/// large, in a window of its own that follows the choice and the edits. An iPad shows it beside the
/// app or, under Stage Manager, on an external display; a display that only mirrors gets it from
/// `ExternalDisplayDelegate`.
struct SecondWindowView: View {
    static let id = "second"
    /// What it shows (the one value its windows present, so there is one window).
    static let loupe = "loupe"
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            if let photo = model.state.active {
                PhotoPicture(photo: photo)
                    .padding(24)
                    .id(photo)
                    .transition(.opacity)
            } else {
                ContentUnavailableView("No Photo", systemImage: "photo", description: Text("Choose a photo in the main window."))
            }
        }
        .animation(.easeInOut(duration: 0.25), value: model.state.active)
        .overlay(alignment: .bottomLeading) { caption }
        .preferredColorScheme(.dark)
        // iPadOS may bring back this window alone after a relaunch: the library comes up too
        .task {
            AppModel.current = model
            await model.start()
            if model.mainWindows == 0 { openWindow(id: RootView.id) }
        }
    }

    /// The photo's name and stars, small in a corner.
    @ViewBuilder private var caption: some View {
        if let id = model.state.active, let photo = model.library.index(of: id).flatMap({ model.library.photo(at: $0, model.engine) }) {
            HStack(spacing: 8) {
                Text(photo.title ?? photo.fileName)
                if let r = photo.rating, r > 0 {
                    Text(String(repeating: "★", count: min(r, 5))).accessibilityLabel(Text("\(r) stars"))
                }
            }
            .font(.footnote)
            .foregroundStyle(.white.opacity(0.75))
            .padding(16)
        }
    }
}

/// A screen that only shows what the app gives it (a mirroring external display, AirPlay): the
/// second window's photo, full screen. Named in Info.plist for that role; SwiftUI makes the app's
/// own windows.
@objc(LCExternalDisplayDelegate)
final class ExternalDisplayDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?

    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
        guard let scene = scene as? UIWindowScene, let model = AppModel.current else { return }
        let window = UIWindow(windowScene: scene)
        window.rootViewController = UIHostingController(rootView: SecondWindowView().environment(model))
        window.isHidden = false
        self.window = window
    }
}
#endif
