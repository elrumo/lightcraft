import Foundation
import LightCraftCore

/// The keyboard (docs/ios-swift.md → Keyboard). A key means what `KeyMap` says: the commands' own
/// shortcuts, the UI commands' and the fixed keys the host lists (`host.keyBindings`, shared with the
/// egui app), and what it does where the user is (`KeyRules`, also egui's). The Mac's input monitor
/// (`Mac/MacInput.swift`) and the script's `ui.key` both come here.
extension AppModel {
    /// Keys only this app has: the panels away (Tab), the first and the last photo (Home, End).
    static let localKeys: [(id: String, shortcut: String?)] = [("view.panels", "Tab"), ("nav.first", "Home"), ("nav.last", "End")]

    /// The key map, from the commands as they are listed now.
    func rebuildKeyMap() {
        let ui = uiCommands.filter { Self.uiActions.contains($0.id) && !Self.macOwned.contains($0.id) }.map { (id: $0.id, shortcut: $0.shortcut) } + Self.localKeys
        let engine = engineCommands.filter { !Self.replaced.contains($0.id) && !Self.needsParams($0.params) }.map { (id: $0.id, shortcut: $0.shortcut) }
        keyMap = KeyMap(ui: ui, engine: engine, fixed: keyTable?.fixed ?? [])
    }

    /// The photos take the keyboard (a click on one): a list, a field or a slider that had it lets go.
    func claimKeyboard() {
        arrowsTaken = nil
        Platform.claimKeyboard()
    }

    /// The loupe is the screen on top.
    var inLoupe: Bool {
        if case .photo = path.last { return true }
        return false
    }

    /// Compare or Survey is the screen on top: ratings and flags go to the active photo only.
    var cullScreen: Bool {
        switch path.last {
        case .compare, .survey: true
        default: false
        }
    }

    /// Where the keys are, as far as what they mean goes.
    var keyContext: KeyContext {
        let screen: KeyContext.Screen = switch path.last {
        case nil: .grid
        case .photo: .loupe
        case .compare: .compare
        case .survey: .survey
        default: .other
        }
        return KeyContext(screen: screen, tool: screen == .loupe ? editor.tool?.rawValue : nil, inAlbum: plainAlbum != nil,
                          activeMask: state.activeMask != nil, activeSpot: state.activeSpot != nil)
    }

    /// A key pressed in the window in front. True when it means something here (so the window leaves
    /// it alone), false when it is for something else.
    @discardableResult
    func press(_ chord: KeyChord) -> Bool {
        guard engine != nil, let ruling = KeyRules.ruling(keyMap.bindings(for: chord), in: keyContext) else { return false }
        switch ruling {
        case .ignore:
            return true
        case .skip:
            return false
        case .run(let id, let params, let ui):
            if ui {
                if uiEnabled(id) { runUI(id) }
            } else {
                runCommand(id, params, announce: true)
            }
            return true
        }
    }

    /// Run an engine command for a key or a menu item. It works on the photos chosen (in Compare and
    /// Survey on the active photo only), asks before deleting, goes on to the next photo after a
    /// rating or a flag when the key says so or Auto Advance is on, and (`announce`) says what it
    /// did. A command that can't run now, or fails, is left alone: a key press that doesn't apply is
    /// not an error.
    func runCommand(_ id: String, _ params: JSONValue = [:], announce: Bool = false) {
        guard let engine else { return }
        // (not checked against the list's `enabled`, which is as old as the last menu refresh: the engine says no itself)
        var params = params
        var goOn = false
        switch id {
        case "photo.delete":
            let ids = cullScreen ? state.active.map { [$0] } ?? [] : targets
            if !ids.isEmpty { confirmDelete = ids }
            return
        case "album.removePhotos":
            guard let album = plainAlbum else { return }
            params["id"] = .int(Int64(clamping: album))
        case "library.up", "library.down", "library.next", "library.previous":
            let forward = id == "library.down" || id == "library.next"
            if cullScreen {
                cullStep?(forward ? 1 : -1)
                return
            }
            // in the grid, Up and Down go to the cell above and below
            if path.isEmpty, id == "library.up" || id == "library.down" {
                guard let active = state.active, let at = library.index(of: active) else { break }
                let rows = GridNavigation.rowDistance(from: at, down: forward, across: gridAcross, groups: library.groups, count: library.count)
                if rows == 0 { return }
                params["rows"] = .int(Int64(rows))
            }
        case "photo.rate", "photo.flag", "photo.label", "photo.pick", "photo.reject", "photo.unflag", "photo.increaseRating", "photo.decreaseRating":
            if cullScreen, params["ids"].isNull, let active = state.active { params["ids"] = [.int(Int64(clamping: active))] }
            let advance = params["advance"].bool == true || prefs.autoAdvance
            // (the engine moves on itself, except in Compare and Survey, where the next candidate is this app's)
            params["advance"] = .bool(advance && !cullScreen)
            goOn = advance && cullScreen
        default:
            break
        }
        let say = announce ? Self.announcement(id, params) : nil
        Task {
            guard (try? await engine.execute(id, params)) != nil else { return }
            if goOn { cullStep?(1) }
            if let say { show(say) }
        }
    }

    /// What a rating or a flag says about itself, as egui does.
    private static func announcement(_ id: String, _ params: JSONValue) -> String? {
        switch id {
        case "photo.rate":
            let n = Int(params["rating"].int ?? 0)
            return n <= 0 ? String(localized: "Rating cleared") : String(localized: "Rated \(String(repeating: "★", count: min(n, 5)))")
        case "photo.pick": return String(localized: "Flagged as Pick")
        case "photo.reject": return String(localized: "Flagged as Reject")
        case "photo.unflag": return String(localized: "Unflagged")
        case "photo.flag":
            switch params["flag"].string {
            case "pick": return String(localized: "Flagged as Pick")
            case "reject": return String(localized: "Flagged as Reject")
            default: return String(localized: "Unflagged")
            }
        default: return nil
        }
    }

    /// Home and End: the first or the last photo of the grid, chosen.
    func goToEnd(last: Bool) {
        guard let engine, library.count > 0 else { return }
        let at = last ? library.count - 1 : 0
        Task {
            let page = try? await engine.execute("catalog.query", ["offset": .int(Int64(at)), "limit": 1], as: PhotoPage.self)
            if let photo = page?.photos.first { await select(photo.id) }
        }
    }

    /// A tool's own screen (Crop, Masking, Remove, Red Eye) cancelled: back to how the photo was when it opened.
    func cancelToolScreen() {
        Task {
            if let base = editor.screenBase { await run("edit.undoTo", ["depth": .int(Int64(base))]) }
            editor.tool = .edit
        }
    }

    // MARK: Help ▸ Keyboard Shortcuts

    /// The keys that move around, which no menu or table lists.
    var movingKeyRows: [KeyRow] {
        let section = String(localized: "Moving Around")
        return [
            KeyRow(section: section, label: String(localized: "Previous Photo"), shortcut: "Left"),
            KeyRow(section: section, label: String(localized: "Next Photo"), shortcut: "Right"),
            KeyRow(section: section, label: String(localized: "Photo Above"), shortcut: "Up"),
            KeyRow(section: section, label: String(localized: "Photo Below"), shortcut: "Down"),
            KeyRow(section: section, label: String(localized: "First Photo"), shortcut: "Home"),
            KeyRow(section: section, label: String(localized: "Last Photo"), shortcut: "End"),
            KeyRow(section: section, label: String(localized: "Open Photo"), shortcut: "Enter"),
            KeyRow(section: section, label: String(localized: "Back"), shortcut: "Escape"),
            KeyRow(section: section, label: String(localized: "Hide or Show Panels"), shortcut: "Tab"),
        ]
    }

    /// The keys that no menu item lists (rating, labels and flags, the second keys of commands that
    /// have one), as rows: the section, what they do, the shortcut.
    func fixedKeyRows() -> [KeyRow] {
        let culling = String(localized: "Rating, Flags and Labels")
        let other = String(localized: "More Keys")
        let rows = (keyTable?.fixed ?? []).map { key in
            // (Shift with a rating, a flag or a label goes on to the next photo)
            let goOn = { (what: String) in key.params["advance"].bool == true ? String(localized: "\(what) and Go On") : what }
            switch key.command {
            case "photo.rate":
                let n = Int(key.params["rating"].int ?? 0)
                let what = n == 0 ? String(localized: "Clear Rating") : n == 1 ? String(localized: "Rate 1 Star") : String(localized: "Rate \(n) Stars")
                return KeyRow(section: culling, label: goOn(what), shortcut: key.shortcut)
            case "photo.label":
                return KeyRow(section: culling, label: goOn(String(localized: "\(library.labelName(key.params["label"].string ?? "")) Label")), shortcut: key.shortcut)
            case "photo.flag":
                let flag = key.params["flag"].string ?? "none"
                return KeyRow(section: culling, label: goOn(flag == "pick" ? String(localized: "Pick") : flag == "reject" ? String(localized: "Reject") : String(localized: "Unflag")), shortcut: key.shortcut)
            default:
                let label = uiCommands.first { $0.id == key.command }?.label ?? engineCommands.first { $0.id == key.command }?.label ?? key.command
                // (the keyword set's nine keys are one command: which of the set it is is the number)
                let which = key.params["index"].int.map { " \($0)" } ?? ""
                return KeyRow(section: other, label: Bundle.main.localizedString(forKey: label, value: label, table: nil) + which, shortcut: key.shortcut)
            }
        }
        // rating, flags and labels first: they are what most people look for
        return rows.filter { $0.section == culling } + rows.filter { $0.section != culling }
    }
}

/// A key that no menu lists, for Help ▸ Keyboard Shortcuts.
struct KeyRow: Identifiable {
    let section: String
    let label: String
    let shortcut: String
    var id: String { shortcut }
}
