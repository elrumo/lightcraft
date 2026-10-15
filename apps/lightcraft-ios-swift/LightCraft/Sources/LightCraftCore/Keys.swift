import Foundation

// The keyboard of a host that has real keys (the Mac app): a key press as a shortcut spells it, the
// map from keys to commands (the commands' own shortcuts, the UI commands', and the fixed keys the
// engine host lists: `host.keyBindings`, `apphost::keys`), what a key means in the screen and tool
// that are open, and how far the arrow keys move in a grid. Pure values: no AppKit, tested by
// `swift test`.

/// A key press as a shortcut spells it: `Cmd+Shift+Z`, `Right`, `Shift+{`, `Alt+1`, `F6`, `Space`.
/// The key is lowercased and shifted symbols are their plain key with Shift (`{` is Shift+`[`), so
/// a spelling and the press it describes are equal.
public struct KeyChord: Hashable, Sendable, CustomStringConvertible {
    /// `a`, `5`, `[`, `left`, `space`, `return`, `escape`, `delete`, `tab`, `f6`…
    public var key: String
    public var command: Bool
    public var shift: Bool
    public var option: Bool
    public var control: Bool

    public init(_ key: String, command: Bool = false, shift: Bool = false, option: Bool = false, control: Bool = false) {
        var key = key.lowercased()
        var shift = shift
        if let base = Self.shifted[key] {
            key = base
            shift = true
        }
        self.key = Self.names[key] ?? key
        self.command = command
        self.shift = shift
        self.option = option
        self.control = control
    }

    /// A shortcut's spelling; `nil` for one that names no key or has a modifier it doesn't know.
    public init?(shortcut: String) {
        var parts = shortcut.split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        // "Cmd+=" is the key `=`; a trailing empty part ("Cmd++") is the key `+`
        guard var key = parts.popLast() else { return nil }
        if key.isEmpty {
            // the plus key itself ("Cmd++" splits into "Cmd", "", ""); an empty spelling names nothing
            guard !parts.isEmpty else { return nil }
            key = "+"
            if parts.last == "" { parts.removeLast() }
        }
        var (command, shift, option, control) = (false, false, false, false)
        for modifier in parts {
            switch modifier {
            case "Cmd": command = true
            case "Shift": shift = true
            case "Alt": option = true
            case "Ctrl": control = true
            default: return nil
            }
        }
        self.init(key, command: command, shift: shift, option: option, control: control)
        // a key with nothing to name it
        if self.key.isEmpty { return nil }
    }

    /// Cmd or Ctrl is down: the menus' kind of shortcut, which a text field never wants.
    public var isMenuKey: Bool { command || control }

    /// Whether it is one of the function keys, which type nothing.
    public var isFunctionKey: Bool { key.count >= 2 && key.hasPrefix("f") && Int(key.dropFirst()) != nil }

    /// An arrow key.
    public var isArrow: Bool { Self.arrows.contains(key) }

    /// The keys that move around in, or press, the control that has the focus: a list, a slider, a menu.
    public var isNavigation: Bool { isArrow || Self.moving.contains(key) }

    /// A key that should go on while it is held: the arrows step through photos, [ and ] size the brush.
    public var repeats: Bool { (isArrow || key == "[" || key == "]") && !command && !control && !option }

    /// A character key with nothing held but Shift: what typing is made of.
    public var isTyping: Bool { !command && !control && !option && key.count == 1 }

    /// The egui spelling, modifiers in its order (Cmd, Ctrl, Alt, Shift).
    public var description: String {
        var s = ""
        if command { s += "Cmd+" }
        if control { s += "Ctrl+" }
        if option { s += "Alt+" }
        if shift { s += "Shift+" }
        return s + (Self.spelled[key] ?? (key.count == 1 ? key.uppercased() : key.capitalized))
    }

    private static let arrows: Set<String> = ["left", "right", "up", "down"]
    private static let moving: Set<String> = ["home", "end", "pageup", "pagedown", "space", "return", "tab", "escape"]
    private static let shifted = ["{": "[", "}": "]", "?": "/", "<": ",", ">": ".", ":": ";", "\"": "'", "|": "\\", "~": "`", "_": "-", "+": "="]
    private static let names = ["enter": "return", "backspace": "delete", "esc": "escape"]
    private static let spelled = [
        "left": "Left", "right": "Right", "up": "Up", "down": "Down", "escape": "Escape", "return": "Enter", "delete": "Delete",
        "space": "Space", "tab": "Tab", "home": "Home", "end": "End", "pageup": "PageUp", "pagedown": "PageDown",
    ]
}

/// What a key does: an engine command with its parameters, or one of the UI commands (which the app
/// does itself).
public struct KeyBinding: Hashable, Sendable {
    public enum Origin: Sendable { case ui, engine }
    public var command: String
    public var params: JSONValue
    public var origin: Origin

    public init(command: String, params: JSONValue = [:], origin: Origin) {
        self.command = command
        self.params = params
        self.origin = origin
    }
}

/// One of `host.keyBindings`' fixed keys.
public struct FixedKey: Decodable, Sendable, Hashable {
    public var shortcut: String
    public var command: String
    public var params: JSONValue

    public init(shortcut: String, command: String, params: JSONValue = [:]) {
        self.shortcut = shortcut
        self.command = command
        self.params = params
    }
}

extension FixedKey {
    /// `host.keyBindings`' answer: the fixed keys and the pairs of commands that share a key.
    public struct Table: Decodable, Sendable {
        public var fixed: [FixedKey]
        public var contextual: [[String]]
    }
}

/// The keys of the commands the host lists, and what each one is bound to. A key can have several
/// bindings (X rejects a photo, and in the Crop tool swaps the crop's aspect): [`KeyRules`] picks.
public struct KeyMap: Sendable {
    private var table: [KeyChord: [KeyBinding]] = [:]

    /// UI commands first, then the engine's (one that has a UI command's key gives it up, as egui's
    /// keymap does), then the fixed keys (a UI command or an engine one, as the lists say).
    public init(ui: [(id: String, shortcut: String?)], engine: [(id: String, shortcut: String?)], fixed: [FixedKey]) {
        let uiIDs = Set(ui.map(\.id))
        var uiKeys: Set<KeyChord> = []
        for command in ui {
            guard let chord = command.shortcut.flatMap({ KeyChord(shortcut: $0) }) else { continue }
            uiKeys.insert(chord)
            add(chord, KeyBinding(command: command.id, origin: .ui))
        }
        for command in engine {
            guard let chord = command.shortcut.flatMap({ KeyChord(shortcut: $0) }), !uiKeys.contains(chord) else { continue }
            add(chord, KeyBinding(command: command.id, origin: .engine))
        }
        for key in fixed {
            guard let chord = KeyChord(shortcut: key.shortcut) else { continue }
            add(chord, KeyBinding(command: key.command, params: key.params, origin: uiIDs.contains(key.command) ? .ui : .engine))
        }
    }

    private mutating func add(_ chord: KeyChord, _ binding: KeyBinding) {
        if !(table[chord]?.contains(binding) ?? false) { table[chord, default: []].append(binding) }
    }

    /// What `chord` is bound to, in the order they were added.
    public func bindings(for chord: KeyChord) -> [KeyBinding] { table[chord] ?? [] }

    /// How many keys are bound.
    public var count: Int { table.count }
}

/// Where the user is, as far as a key's meaning goes.
public struct KeyContext: Sendable, Equatable {
    public enum Screen: Sendable { case grid, loupe, compare, survey, other }
    public var screen: Screen
    /// The open tool (`edit`, `crop`, `masking`, `remove`, `redEye`, `presets`, `info`), if any.
    public var tool: String?
    /// The grid shows a plain album: Backspace takes the photos out of it.
    public var inAlbum: Bool
    public var activeMask: Bool
    public var activeSpot: Bool

    public init(screen: Screen, tool: String? = nil, inAlbum: Bool = false, activeMask: Bool = false, activeSpot: Bool = false) {
        self.screen = screen
        self.tool = tool
        self.inAlbum = inAlbum
        self.activeMask = activeMask
        self.activeSpot = activeSpot
    }
}

/// What a key means in a context. These are egui's rules (`ui-egui/src/shortcuts.rs`, `handle`), the
/// ones that depend on where you are and not on how the window is drawn: a key shared by two
/// commands, a command that does something else in the grid or while a tool is open.
public enum KeyRules {
    public enum Ruling: Equatable, Sendable {
        /// Do this (a UI command is `ui: true`).
        case run(String, JSONValue, ui: Bool)
        /// The key means nothing here, and is used up (Delete in Red Eye).
        case ignore
        /// Not this binding: try the next one.
        case skip
    }

    /// The first binding of `bindings` that means something in `context`, as what to do.
    public static func ruling(_ bindings: [KeyBinding], in context: KeyContext) -> Ruling? {
        for binding in bindings {
            switch ruling(binding, in: context) {
            case .skip: continue
            case let r: return r
            }
        }
        return nil
    }

    /// Delete acts on what is being edited, never the photo while retouching.
    private static func deleting(_ c: KeyContext) -> Ruling {
        switch c.tool {
        case "masking": c.activeMask ? .run("mask.delete", [:], ui: false) : .ignore
        case "remove": c.activeSpot ? .run("spot.delete", [:], ui: false) : .ignore
        case "redEye": .ignore
        default: .run("photo.delete", [:], ui: false)
        }
    }

    static func ruling(_ binding: KeyBinding, in c: KeyContext) -> Ruling {
        let grid = c.screen == .grid
        let run = { (id: String, params: JSONValue, ui: Bool) -> Ruling in .run(id, params, ui: ui) }
        switch binding.command {
        // X rejects, and in the Crop tool swaps the crop's aspect
        case "photo.reject" where c.tool == "crop": return run("crop.rotateAspect", [:], false)
        case "crop.rotateAspect" where c.tool != "crop": return .skip
        // ⌫ takes the photos out of the album its grid shows; anywhere else it deletes, as ⌘⌫ always does
        case "album.removePhotos": return grid && c.inAlbum ? binding.runs() : deleting(c)
        case "photo.delete": return deleting(c)
        // [ and ] rate down and up in the grid (they size the brush elsewhere)
        case "brush.smaller" where grid: return run("photo.decreaseRating", [:], false)
        case "brush.larger" where grid: return run("photo.increaseRating", [:], false)
        // B adds to the target album in the grid (the brush while editing)
        case "tool.brush" where grid: return run("album.toggleTarget", [:], false)
        // S expands and collapses stacks in the grid (soft proofing elsewhere)
        case "view.softProof" where grid: return run("stack.toggle", [:], false)
        // Shift+P picks and goes on in the grid (the Presets panel elsewhere)
        case "panel.presets" where grid: return run("photo.flag", ["flag": "pick", "advance": true], false)
        // / refreshes the source of the spot chosen in Remove (the filmstrip elsewhere)
        case "view.filmstrip" where c.tool == "remove" && c.activeSpot: return run("spot.refreshSource", [:], false)
        // O and Shift+O: the crop's guides while cropping, the mask overlay's colour while masking
        case "view.cropOverlay" where c.tool == "masking": return run("view.maskOverlayColor", [:], true)
        case "view.maskOverlay" where c.tool == "crop": return run("view.cropOverlay", [:], true)
        case "view.cropOverlay" where c.tool == "crop": return run("view.cropOverlayOrientation", [:], true)
        // A locks the crop's aspect while cropping
        case "view.visualizeSpots" where c.tool == "crop": return run("crop.aspect", ["aspect": "toggle"], false)
        default: return binding.runs()
        }
    }
}

extension KeyBinding {
    fileprivate func runs() -> KeyRules.Ruling { .run(command, params, ui: origin == .ui) }
}

/// How far the arrow keys move in a grid of square cells, `across` to a row, under date headers
/// that each start a row of their own.
public enum GridNavigation {
    /// How many photos to move from `index` to the cell below (`down`) or above, keeping the column:
    /// `across` within a group; across a group's edge, to the same column of the next group's first
    /// row (or the previous group's last); a short last row's cell is the group's last photo. `0`
    /// when there is nowhere to go (the grid's first or last row).
    public static func rowDistance(from index: Int, down: Bool, across: Int, groups: [DateGroup], count: Int) -> Int {
        let across = max(across, 1)
        guard index >= 0, index < count else { return 0 }
        // one run when the sort makes no headers (or the headers have not caught up with the grid)
        let whole = [DateGroup(key: "", label: "", start: 0, count: count)]
        let found = groups.firstIndex { $0.indices(in: count).contains(index) }
        let runs = found == nil ? whole : groups
        let g = found ?? 0
        // (the group cut to the grid: a header list that is out of date can't send the cursor anywhere odd)
        let group = runs[g].indices(in: count)
        let p = index - group.lowerBound
        let (row, column) = (p / across, p % across)
        if down {
            if (row + 1) * across + column < group.count { return across }
            // the next row exists but is shorter than this column: its last cell
            if (row + 1) * across < group.count { return group.upperBound - 1 - index }
            guard g + 1 < runs.count else { return 0 }
            let next = runs[g + 1].indices(in: count)
            guard !next.isEmpty else { return 0 }
            return next.lowerBound + min(column, next.count - 1) - index
        }
        if row > 0 { return across }
        guard g > 0 else { return 0 }
        let before = runs[g - 1].indices(in: count)
        guard !before.isEmpty else { return 0 }
        let lastRow = (before.count - 1) / across
        return index - (before.lowerBound + min(lastRow * across + column, before.count - 1))
    }
}
