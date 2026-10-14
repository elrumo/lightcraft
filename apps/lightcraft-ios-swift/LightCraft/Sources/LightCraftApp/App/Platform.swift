import LightCraftCore
import SwiftUI

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

// The few places iOS and macOS differ, so a call site changes by one token instead of an `#if`
// block (docs/ios-swift.md → macOS). Presentation only: nothing here knows the engine. Whole
// screens that differ live under Mac/, not here.

extension View {
    /// A small title in the navigation bar (iOS). The window toolbar of a Mac has no such modes.
    @ViewBuilder func inlineTitle() -> some View {
        #if os(iOS)
        navigationBarTitleDisplayMode(.inline)
        #else
        self
        #endif
    }

    /// No automatic capital letter (iOS keyboards).
    @ViewBuilder func noAutocap() -> some View {
        #if os(iOS)
        textInputAutocapitalization(.never)
        #else
        self
        #endif
    }

    /// The keyboard a field wants on a phone; a Mac has one keyboard.
    @ViewBuilder func lcKeyboard(_ kind: LCKeyboard) -> some View {
        #if os(iOS)
        keyboardType(kind.uiKeyboardType)
        #else
        self
        #endif
    }

    /// A menu item that leaves its menu open (Undo/Redo in the photo's menu, to tap again). A Mac's
    /// menus always close.
    @ViewBuilder func keepsMenuOpen() -> some View {
        #if os(iOS)
        menuActionDismissBehavior(.disabled)
        #else
        self
        #endif
    }

    /// A photo opening out of its tile (iOS's zoom transition); a Mac window pushes it plainly.
    @ViewBuilder func lcZoomTransition<ID: Hashable>(sourceID: ID, in namespace: Namespace.ID) -> some View {
        #if os(iOS)
        navigationTransition(.zoom(sourceID: sourceID, in: namespace))
        #else
        self
        #endif
    }

    /// Hides the status bar (iOS); a Mac window has none.
    @ViewBuilder func lcStatusBarHidden(_ hidden: Bool = true) -> some View {
        #if os(iOS)
        statusBarHidden(hidden)
        #else
        self
        #endif
    }

    /// The pointer lifting a tile on an iPad. (A Mac's hover states are drawn by the views.)
    @ViewBuilder func lcHoverLift() -> some View {
        #if os(iOS)
        hoverEffect(.lift)
        #else
        self
        #endif
    }
}

enum LCKeyboard {
    case url, number, normal

    #if os(iOS)
    var uiKeyboardType: UIKeyboardType {
        switch self {
        case .url: .URL
        case .number: .numbersAndPunctuation
        case .normal: .default
        }
    }
    #endif
}

extension ToolbarItemPlacement {
    /// The leading edge of the navigation bar (iOS) or window toolbar (macOS).
    static var lcLeading: ToolbarItemPlacement {
        #if os(iOS)
        return .topBarLeading
        #else
        return .navigation
        #endif
    }

    /// The trailing edge of the navigation bar or window toolbar.
    static var lcTrailing: ToolbarItemPlacement {
        #if os(iOS)
        return .topBarTrailing
        #else
        return .primaryAction
        #endif
    }

    /// The bottom bar of a phone. A Mac window has none: the system places the item.
    static var lcBottom: ToolbarItemPlacement {
        #if os(iOS)
        return .bottomBar
        #else
        return .automatic
        #endif
    }
}

extension Color {
    /// The system's grouped-background greys, by the names iOS gives them; a Mac's nearest.
    static var lcTertiaryFill: Color {
        #if os(iOS)
        return Color(uiColor: .tertiarySystemFill)
        #else
        return Color(nsColor: .tertiarySystemFill)
        #endif
    }

    static var lcFill: Color {
        #if os(iOS)
        return Color(uiColor: .systemFill)
        #else
        return Color(nsColor: .systemFill)
        #endif
    }

    static var lcSecondaryBackground: Color {
        #if os(iOS)
        return Color(uiColor: .secondarySystemBackground)
        #else
        return Color(nsColor: .controlBackgroundColor)
        #endif
    }

    static var lcBackground: Color {
        #if os(iOS)
        return Color(uiColor: .systemBackground)
        #else
        return Color(nsColor: .windowBackgroundColor)
        #endif
    }
}

/// What the app asks of the device and the system, by one name on both platforms.
enum Platform {
    /// Whose start-up rules the engine follows (`OpenConfig.profile`): `nil` is a phone's, the default.
    /// A Mac keeps its library in Pictures and says why when it can't open it, instead of a demo.
    static var profile: Profile? {
        #if os(iOS)
        return nil
        #else
        return .desktop
        #endif
    }

    /// Whether plugging in a camera or a card opens its import by default. A phone's user means it; a
    /// Mac's has an iPhone on a cable now and then.
    static var importsOnConnect: Bool {
        #if os(iOS)
        return true
        #else
        return false
        #endif
    }

    /// The app's tmp folder: staging for picks and exports, the automation script. A phone's
    /// `NSTemporaryDirectory()` is the app's own; a Mac's is shared with every other app, and the
    /// engine empties `Import` and `Exports` in it at each launch, so a Mac gets a folder of its own.
    static var tmpFolder: String {
        #if os(iOS)
        return NSTemporaryDirectory()
        #else
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first ?? URL(fileURLWithPath: NSTemporaryDirectory())
        let dir = caches.appendingPathComponent(Bundle.main.bundleIdentifier ?? "ai.storyteller.lightcraft.swift").appendingPathComponent("tmp")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.path
        #endif
    }

    /// The device's name, as sync shows it ("Ann's iPhone", "Ann's MacBook Pro").
    static var deviceName: String {
        #if os(iOS)
        return UIDevice.current.name
        #else
        return Host.current().localizedName ?? ProcessInfo.processInfo.hostName
        #endif
    }

    /// "iPhone, iOS 26.5" / "Mac, Version 27.2 (Build …)" for About.
    static var deviceDescription: String {
        #if os(iOS)
        let d = UIDevice.current
        return "\(d.model), \(d.systemName) \(d.systemVersion)"
        #else
        return "Mac, \(ProcessInfo.processInfo.operatingSystemVersionString)"
        #endif
    }

    /// Whether the app can show a second window (a Mac always can; an iPad only in iPadOS's windowed modes).
    static var supportsMultipleWindows: Bool {
        #if os(iOS)
        return UIApplication.shared.supportsMultipleScenes
        #else
        return true
        #endif
    }

    /// Puts text on the system clipboard.
    static func copy(_ text: String) {
        #if os(iOS)
        UIPasteboard.general.string = text
        #else
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        #endif
    }

    /// Opens a web address (or any URL) in the app that handles it.
    static func open(_ url: URL) {
        #if os(iOS)
        UIApplication.shared.open(url)
        #else
        NSWorkspace.shared.open(url)
        #endif
    }

    #if os(macOS)
    private static var awake: NSObjectProtocol?
    #endif

    /// Keeps the screen on while a slideshow runs (the idle timer on iOS, a power assertion on a Mac).
    static func keepAwake(_ on: Bool) {
        #if os(iOS)
        UIApplication.shared.isIdleTimerDisabled = on
        #else
        if on, awake == nil {
            awake = ProcessInfo.processInfo.beginActivity(options: [.idleDisplaySleepDisabled, .userInitiated], reason: "Slideshow")
        } else if !on, let token = awake {
            ProcessInfo.processInfo.endActivity(token)
            awake = nil
        }
        #endif
    }
}
