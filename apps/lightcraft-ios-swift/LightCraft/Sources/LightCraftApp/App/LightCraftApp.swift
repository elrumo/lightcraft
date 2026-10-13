#if os(iOS)
import Combine
import SwiftUI
import UIKit

/// LightCraft for iPhone and iPad: SwiftUI over the Rust engine (docs/ios-swift.md).
@main
struct LightCraftApp: App {
    @State private var model = AppModel()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .task { await model.start() }
                .onReceive(NotificationCenter.default.publisher(for: UIApplication.didReceiveMemoryWarningNotification)) { _ in
                    model.lifecycle("memoryWarning")
                }
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
