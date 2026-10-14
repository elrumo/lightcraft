import Combine
import SwiftUI

#if os(iOS)
import UIKit
#endif

/// LightCraft for iPhone, iPad and Mac: SwiftUI over the Rust engine (docs/ios-swift.md).
@main
struct LightCraftApp: App {
    @State private var model = AppModel.shared
    #if os(iOS)
    @Environment(\.scenePhase) private var phase
    #endif

    var body: some Scene {
        #if os(iOS)
        WindowGroup(id: RootView.id) {
            RootView()
                .environment(model)
                .task { await model.start() }
                .onReceive(NotificationCenter.default.publisher(for: UIApplication.didReceiveMemoryWarningNotification)) { _ in
                    model.lifecycle("memoryWarning")
                }
        }
        .commands { LightCraftMenus(model: model) }
        #else
        // A Mac has one main window: the model (navigation, and the engine's one selection and
        // active photo) is the app's, so a second main window could only mirror the first.
        Window("LightCraft", id: RootView.id) {
            RootView()
                .environment(model)
                .task { await model.start() }
        }
        .commands { LightCraftMenus(model: model) }
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
