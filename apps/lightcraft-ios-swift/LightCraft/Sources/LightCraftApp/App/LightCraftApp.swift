import Combine
import SwiftUI

#if os(iOS)
import UIKit
#endif

/// LightCraft for iPhone, iPad and Mac: SwiftUI over the Rust engine (docs/ios-swift.md).
@main
struct LightCraftApp: App {
    @State private var model = AppModel.shared
    /// What tells this body to run again when the menus' data moves (see `MenuChanges`).
    @ObservedObject private var menuChanges = AppModel.shared.menuChanges
    #if os(iOS)
    @Environment(\.scenePhase) private var phase
    #else
    @NSApplicationDelegateAdaptor(MacAppDelegate.self) private var delegate
    #endif

    var body: some Scene {
        // (read here: see LightCraftMenus.init(reading:); the revision is what makes this body run again)
        let menus = LightCraftMenus(reading: model, revision: menuChanges.revision)
        #if os(iOS)
        WindowGroup(id: RootView.id) {
            RootView()
                .environment(model)
                .task { await model.start() }
                .onReceive(NotificationCenter.default.publisher(for: UIApplication.didReceiveMemoryWarningNotification)) { _ in
                    model.lifecycle("memoryWarning")
                }
        }
        .commands { menus }
        #else
        // A Mac has one main window: the model (navigation, and the engine's one selection and
        // active photo) is the app's, so a second main window could only mirror the first. (A
        // WindowGroup, not a Window, for the File menu it brings; File ▸ New Window is replaced by
        // the app's own File items, in LightCraftMenus.)
        WindowGroup("LightCraft", id: RootView.id) {
            RootView()
                .environment(model)
                .frame(minWidth: 900, minHeight: 600)
                .task { await model.start() }
        }
        .defaultSize(width: 1360, height: 860)
        .commands { menus }
        Settings {
            MacSettings()
                .environment(model)
        }
        Window("Slideshow", id: CoverWindow.id) {
            CoverWindow()
                .environment(model)
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 1280, height: 800)
        // (opened by the show, not from the Window menu)
        .commandsRemoved()
        #endif
        // Window ▸ Second Window: the chosen photo large in a window of its own (one: asked again,
        // the open one comes forward)
        WindowGroup("Second Window", id: SecondWindowView.id, for: String.self) { _ in
            SecondWindowView()
                .environment(model)
        }
        #if os(iOS)
        .onChange(of: phase) { _, phase in
            switch phase {
            case .active: model.lifecycle("active")
            case .inactive: model.lifecycle("resignActive")
            case .background: model.lifecycle("background")
            @unknown default: break
            }
        }
        #endif
    }
}
