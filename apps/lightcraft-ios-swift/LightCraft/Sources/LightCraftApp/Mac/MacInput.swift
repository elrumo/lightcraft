#if os(macOS)
import AppKit
import LightCraftCore

/// The Mac's keyboard, wheel and trackpad, seen before AppKit routes them (docs/ios-swift.md →
/// Keyboard). One monitor on the app's events:
/// - a key goes to `AppModel.press` (the key map and its rules) unless something else should have
///   it: a field being typed in, a list, slider or menu that has the focus and wants its arrows, a
///   sheet or alert on the window, another window;
/// - a scroll, pinch or two-finger double tap over a loupe goes to that loupe (`LoupeInput.swift`).
/// An event that is used here is not passed on, so the menus and the views never see it twice.
@MainActor
final class MacInput {
    static let shared = MacInput()
    private var monitor: Any?
    private var model: AppModel?

    func install(_ model: AppModel) {
        self.model = model
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .scrollWheel, .magnify, .smartMagnify]) { event in
            // (the answer crosses the actor boundary as a Bool: an NSEvent may not)
            let used = MainActor.assumeIsolated { MacInput.shared.handle(event) }
            return used ? nil : event
        }
    }

    /// True when the event was used here.
    private func handle(_ event: NSEvent) -> Bool {
        guard let model, let window = event.window, Self.isMain(window), window.attachedSheet == nil, NSApp.modalWindow == nil else { return false }
        switch event.type {
        case .keyDown: return key(event, window, model)
        case .scrollWheel: return scrolled(event)
        case .magnify:
            guard let (anchor, point) = LoupeAnchors.target(for: event) else { return false }
            anchor.handlers.magnify(event.magnification, point)
            return true
        case .smartMagnify:
            guard let (anchor, point) = LoupeAnchors.target(for: event) else { return false }
            anchor.handlers.smartMagnify(point)
            return true
        default: return false
        }
    }

    // MARK: keys

    private func key(_ event: NSEvent, _ window: NSWindow, _ model: AppModel) -> Bool {
        guard model.sheet == nil, model.cover == nil, model.confirmDelete == nil, let chord = Self.chord(of: event) else { return false }
        if let responder = window.firstResponder {
            // typing in a field: the keys are the field's (⌘ keys still reach the menus)
            if Self.edits(responder) { return false }
            // a list, a slider or a menu with the focus keeps the keys that move it
            if chord.isNavigation, Self.moves(responder) { return false }
        }
        // Tab moves the focus from control to control for someone using Full Keyboard Access
        if chord.key == "tab", NSApp.isFullKeyboardAccessEnabled { return false }
        // a slider of the edit panel with the focus keeps the arrows too (SwiftUI's focus, not AppKit's)
        if chord.isArrow, model.arrowsTaken != nil { return false }
        // held down, only the keys that should go on do
        if event.isARepeat, !chord.repeats { return true }
        if model.press(chord) { return true }
        // a letter, a digit or an arrow that means nothing here is used up too, so the window doesn't beep
        return chord.isTyping || chord.isArrow
    }

    /// A field editor, or a text view, that can be typed in.
    private static func edits(_ responder: NSResponder) -> Bool {
        (responder as? NSText)?.isEditable == true
    }

    /// A control whose arrow, Return and Space keys do something of its own.
    private static func moves(_ responder: NSResponder) -> Bool {
        responder is NSTableView || responder is NSSlider || responder is NSButton || responder is NSSegmentedControl
            || responder is NSStepper || responder is NSBrowser || responder is NSCollectionView
    }

    /// The window the photos are in (not Settings, the second window or a show's).
    static func isMain(_ window: NSWindow) -> Bool {
        window.identifier?.rawValue.hasPrefix(RootView.id) == true
    }

    // MARK: wheel and trackpad

    private func scrolled(_ event: NSEvent) -> Bool {
        guard let (anchor, point) = LoupeAnchors.target(for: event) else { return false }
        let zooming = event.modifierFlags.contains(.command) || event.modifierFlags.contains(.control)
        // the way the wheel or the fingers went, whatever "natural scrolling" does to the numbers: away from
        // the user zooms in
        let away = event.isDirectionInvertedFromDevice ? -event.scrollingDeltaY : event.scrollingDeltaY
        // (a trackpad's numbers are points; a wheel's are lines, about twelve points each)
        let points: CGFloat = event.hasPreciseScrollingDeltas ? 1 : 12
        return anchor.handlers.scroll(CGSize(width: event.scrollingDeltaX * points, height: event.scrollingDeltaY * points), point,
                                      event.hasPreciseScrollingDeltas, zooming ? away : nil)
    }

    // MARK: key events and chords

    /// The keys that have a code of their own: what AppKit calls them, the key code and the character it types.
    private static let named: [(name: String, code: UInt16, character: String)] = [
        ("left", 123, "\u{F702}"), ("right", 124, "\u{F703}"), ("down", 125, "\u{F701}"), ("up", 126, "\u{F700}"),
        ("return", 36, "\r"), ("escape", 53, "\u{1B}"), ("delete", 51, "\u{7F}"), ("space", 49, " "), ("tab", 48, "\t"),
        ("home", 115, "\u{F729}"), ("end", 119, "\u{F72B}"), ("pageup", 116, "\u{F72C}"), ("pagedown", 121, "\u{F72D}"),
        ("f1", 122, "\u{F704}"), ("f2", 120, "\u{F705}"), ("f3", 99, "\u{F706}"), ("f4", 118, "\u{F707}"), ("f5", 96, "\u{F708}"),
        ("f6", 97, "\u{F709}"), ("f7", 98, "\u{F70A}"), ("f8", 100, "\u{F70B}"), ("f9", 101, "\u{F70C}"), ("f10", 109, "\u{F70D}"),
        ("f11", 103, "\u{F70E}"), ("f12", 111, "\u{F70F}"),
        // the number row, whatever Shift or Option make of it
        ("0", 29, "0"), ("1", 18, "1"), ("2", 19, "2"), ("3", 20, "3"), ("4", 21, "4"),
        ("5", 23, "5"), ("6", 22, "6"), ("7", 26, "7"), ("8", 28, "8"), ("9", 25, "9"),
    ]

    /// Key codes that mean the same as another key: the keypad's Enter, forward delete, its digits.
    private static let alike: [UInt16: String] = [
        76: "return", 117: "delete",
        82: "0", 83: "1", 84: "2", 85: "3", 86: "4", 87: "5", 88: "6", 89: "7", 91: "8", 92: "9",
    ]

    private static let names: [UInt16: String] = {
        var names = alike
        for key in named { names[key.code] = key.name }
        return names
    }()

    /// The key an event is, as a shortcut spells it.
    static func chord(of event: NSEvent) -> KeyChord? {
        let flags = event.modifierFlags
        let make = { (key: String) in
            KeyChord(key, command: flags.contains(.command), shift: flags.contains(.shift), option: flags.contains(.option), control: flags.contains(.control))
        }
        if let name = names[event.keyCode] { return make(name) }
        // (a letter as the layout types it: Shift is the chord's own, the other modifiers change nothing)
        guard let typed = event.charactersIgnoringModifiers, typed.count == 1 else { return nil }
        return make(typed)
    }

    /// A key press as the keyboard would send it, for the script (`ui.rawKey`).
    static func keyEvent(_ chord: KeyChord, in window: NSWindow) -> NSEvent? {
        var flags: NSEvent.ModifierFlags = []
        if chord.command { flags.insert(.command) }
        if chord.shift { flags.insert(.shift) }
        if chord.option { flags.insert(.option) }
        if chord.control { flags.insert(.control) }
        let special = named.first { $0.name == chord.key }
        let character = special?.character ?? (chord.shift ? chord.key.uppercased() : chord.key)
        return NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, characters: character, charactersIgnoringModifiers: character,
            isARepeat: false, keyCode: special?.code ?? 0)
    }
}
#endif
