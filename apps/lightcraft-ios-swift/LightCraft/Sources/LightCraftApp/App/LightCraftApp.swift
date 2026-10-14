#if os(iOS)
import Combine
import SwiftUI
import UIKit

/// LightCraft for iPhone and iPad: SwiftUI over the Rust engine (docs/ios-swift.md).
@main
struct LightCraftApp: App {
    @State private var model = AppModel.shared
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup(id: RootView.id) {
            RootView()
                .environment(model)
                .task { await model.start() }
                .onReceive(NotificationCenter.default.publisher(for: UIApplication.didReceiveMemoryWarningNotification)) { _ in
                    model.lifecycle("memoryWarning")
                }
        }
        .commands { LightCraftMenus(model: model) }
        // Window ▸ Second Window: the chosen photo large in a window of its own (one: asked again,
        // the open one comes forward)
        WindowGroup("Second Window", id: SecondWindowView.id, for: String.self) { _ in
            SecondWindowView()
                .environment(model)
        }
        .onChange(of: phase) { _, phase in
            switch phase {
            case .active: model.lifecycle("active")
            case .inactive: model.lifecycle("resignActive")
            case .background: model.lifecycle("background")
            @unknown default: break
            }
        }
    }
}
#endif
