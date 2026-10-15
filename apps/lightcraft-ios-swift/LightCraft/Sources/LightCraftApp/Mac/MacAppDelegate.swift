#if os(macOS)
import AppKit
import LightCraftCore

/// What AppKit asks of an app that SwiftUI's scenes don't answer: the app coming and going, memory
/// pressure, pictures opened from Finder or dropped on the Dock icon, and quitting.
final class MacAppDelegate: NSObject, NSApplicationDelegate {
    private var observers: [NSObjectProtocol] = []
    private var memoryPressure: DispatchSourceMemoryPressure?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // One main window: no tab bar, no View ▸ Show Tab Bar
        NSWindow.allowsAutomaticWindowTabbing = false
        let model = AppModel.shared
        let center = NotificationCenter.default
        // Active and resigned only. Rust's `background` pauses the GPU and frees decoded photos: a Mac
        // that is merely behind other windows (an export under way, a second display) must not do that
        // every time the user switches apps.
        observers = [
            center.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
                MainActor.assumeIsolated { model.lifecycle("active") }
            },
            center.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main) { _ in
                MainActor.assumeIsolated { model.lifecycle("resignActive") }
            },
        ]
        let pressure = DispatchSource.makeMemoryPressureSource(eventMask: [.warning, .critical], queue: .main)
        pressure.setEventHandler { MainActor.assumeIsolated { model.lifecycle("memoryWarning") } }
        pressure.resume()
        memoryPressure = pressure
    }

    /// Closing the window leaves the app running (an export, a sync or an import may be under way);
    /// the Dock icon brings the window back.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool { true }

    /// Quit: ask first when it would cost something, then save, close the library and stop the engine
    /// (until then the library stays locked for the egui app).
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        let model = AppModel.shared
        guard model.engine != nil else { return .terminateNow }
        if let warning = model.quitWarning {
            let alert = NSAlert()
            alert.messageText = String(localized: "Quit LightCraft?")
            alert.informativeText = warning + " " + String(localized: "Quitting stops it.")
            alert.addButton(withTitle: String(localized: "Quit"))
            alert.addButton(withTitle: String(localized: "Cancel"))
            if alert.runModal() != .alertFirstButtonReturn { return .terminateCancel }
        }
        Task {
            await model.terminate()
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }

    /// Open With… and files dropped on the Dock icon.
    func application(_ application: NSApplication, open urls: [URL]) {
        AppModel.shared.importFromFinder(urls)
    }
}
#endif
