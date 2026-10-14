#if os(iOS)
import LightCraftCore
import SwiftUI

/// A UI command as `host.uiCommands` lists it (egui's table, `apphost::ui_commands`).
nonisolated struct UICommand: Decodable, Hashable {
    let id: String
    let label: String
    let shortcut: String?
    let menu: String
}

/// One menu item: an engine command, or a UI command this app has, where egui's menus put it.
nonisolated struct MenuItemSpec: Hashable {
    let id: String
    let label: String
    let shortcut: String?
    let engine: Bool
    let enabled: Bool
    /// The menus it sits in, the top one first (`["Photo", "Photo Merge"]`).
    let path: [String]
}

/// The iPad's menu bar (and the keys a hardware keyboard has, listed when ⌘ is held): the engine's
/// commands with a place in the menus, and egui's UI commands that this app has, with egui's
/// default shortcuts, so both apps answer the same keys.
struct LightCraftMenus: Commands {
    let model: AppModel

    var body: some Commands {
        CommandGroup(replacing: .undoRedo) {
            Button(model.state.undo.map { String(localized: "Undo \(Bundle.main.localizedString(forKey: $0, value: $0, table: nil))") } ?? String(localized: "Undo")) { model.undo() }
                .keyboardShortcut("z", modifiers: .command)
                .disabled(model.state.undo == nil)
            Button(model.state.redo.map { String(localized: "Redo \(Bundle.main.localizedString(forKey: $0, value: $0, table: nil))") } ?? String(localized: "Redo")) { model.redo() }
                .keyboardShortcut("z", modifiers: [.command, .shift])
                .disabled(model.state.redo == nil)
        }
        CommandGroup(after: .newItem) { MenuSection(model: model, top: "File") }
        CommandGroup(after: .pasteboard) { MenuSection(model: model, top: "Edit") }
        CommandMenu("Photo") { MenuSection(model: model, top: "Photo") }
        CommandGroup(after: .sidebar) { MenuSection(model: model, top: "View") }
        CommandGroup(after: .windowArrangement) { MenuSection(model: model, top: "Window") }
        CommandGroup(replacing: .help) { MenuSection(model: model, top: "Help") }
    }
}

/// A menu's items, its submenus as menus of their own.
private struct MenuSection: View {
    let model: AppModel
    let top: String

    var body: some View {
        let items = model.menuItems(top)
        let direct = items.filter { $0.path.count == 1 }
        let subs = items.compactMap { $0.path.count > 1 ? $0.path[1] : nil }.reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }
        ForEach(direct, id: \.self) { item in MenuButton(model: model, item: item) }
        ForEach(subs, id: \.self) { sub in
            Menu(LocalizedStringKey(sub)) {
                ForEach(items.filter { $0.path.count > 1 && $0.path[1] == sub }, id: \.self) { item in MenuButton(model: model, item: item) }
            }
        }
    }
}

private struct MenuButton: View {
    let model: AppModel
    let item: MenuItemSpec

    var body: some View {
        let button = Button(LocalizedStringKey(item.label)) {
            if item.engine { model.perform(item.id) } else { model.runUI(item.id) }
        }
        .disabled(!item.enabled)
        if let key = item.shortcut.flatMap(KeyShortcut.parse) {
            button.keyboardShortcut(key)
        } else {
            button
        }
    }
}

/// egui's shortcut spellings ("Cmd+Shift+C", "G", "Left", "Escape") as SwiftUI shortcuts.
enum KeyShortcut {
    static func parse(_ text: String) -> KeyboardShortcut? {
        var parts = text.split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        // "Cmd+=" / "Cmd+-": the key itself; "Cmd++" would be "+"
        guard var key = parts.popLast() else { return nil }
        if key.isEmpty { key = "+" }
        var modifiers: EventModifiers = []
        for m in parts {
            switch m {
            case "Cmd": modifiers.insert(.command)
            case "Shift": modifiers.insert(.shift)
            case "Alt": modifiers.insert(.option)
            case "Ctrl": modifiers.insert(.control)
            default: return nil
            }
        }
        let equivalent: KeyEquivalent
        switch key {
        case "Left": equivalent = .leftArrow
        case "Right": equivalent = .rightArrow
        case "Up": equivalent = .upArrow
        case "Down": equivalent = .downArrow
        case "Escape": equivalent = .escape
        case "Enter", "Return": equivalent = .return
        case "Delete", "Backspace": equivalent = .delete
        case "Space": equivalent = .space
        case "Tab": equivalent = .tab
        default:
            // one character (F-keys and the like aren't keys SwiftUI menus can take)
            guard key.count == 1, let c = key.lowercased().first else { return nil }
            equivalent = KeyEquivalent(c)
        }
        return KeyboardShortcut(equivalent, modifiers: modifiers)
    }

    /// A shortcut as a comparable name ("⌘⇧c").
    static func name(_ k: KeyboardShortcut) -> String {
        var m = ""
        if k.modifiers.contains(.control) { m += "⌃" }
        if k.modifiers.contains(.option) { m += "⌥" }
        if k.modifiers.contains(.shift) { m += "⇧" }
        if k.modifiers.contains(.command) { m += "⌘" }
        return m + String(k.key.character).lowercased()
    }
}

extension AppModel {
    /// The engine's commands and egui's UI commands, for the menu bar (again when the state moves on,
    /// so what can run now is up to date).
    func loadMenus() async {
        guard let engine else { return }
        if uiCommands.isEmpty {
            uiCommands = (try? await engine.request("host.uiCommands").decode([UICommand].self)) ?? []
        }
        menuEngine = ((try? await engine.commands()) ?? []).filter { !$0.menu.isEmpty }
    }

    /// The items under `top` ("File", "Photo"…): the engine's, then the UI commands this app has.
    /// A key is given once: the system's own (⌘A, ⌘C…) stay the system's, and a key two items share
    /// goes to the first (UIKit refuses a menu with the same key twice).
    func menuItems(_ top: String) -> [MenuItemSpec] {
        menuPlan().filter { $0.path.first == top }
    }

    /// Every menu's items, keys shared out.
    func menuPlan() -> [MenuItemSpec] {
        let engine = menuEngine.filter { !Self.replaced.contains($0.id) && !Self.needsParams($0.params) }.map {
            MenuItemSpec(id: $0.id, label: $0.label, shortcut: $0.shortcut, engine: true, enabled: $0.enabled, path: $0.menu)
        }
        let ui = uiCommands.compactMap { c -> MenuItemSpec? in
            let path = c.menu.split(separator: ">").map(String.init)
            guard !path.isEmpty, Self.uiActions.contains(c.id) else { return nil }
            return MenuItemSpec(id: c.id, label: c.label, shortcut: c.shortcut, engine: false, enabled: uiEnabled(c.id), path: path)
        }
        var taken = Self.systemKeys
        return (engine + ui).map { item in
            guard let sc = item.shortcut, let key = KeyShortcut.parse(sc) else { return item }
            let name = KeyShortcut.name(key)
            if taken.contains(name) {
                return MenuItemSpec(id: item.id, label: item.label, shortcut: nil, engine: item.engine, enabled: item.enabled, path: item.path)
            }
            taken.insert(name)
            return item
        }
    }

    /// A command that can't run without being told something (`{paths: …}`, not `{ids?}`): a menu
    /// item has nothing to tell it.
    static func needsParams(_ params: String) -> Bool {
        guard let open = params.firstIndex(of: "{"), let close = params.firstIndex(of: "}"), open < close else { return false }
        let inside = params[params.index(after: open)..<close]
        // `name:` at the top level, without the `?` that makes it optional
        var depth = 0
        var word = ""
        for c in inside {
            switch c {
            case "[", "(", "{": depth += 1; word = ""
            case "]", ")", "}": depth -= 1; word = ""
            case ":" where depth == 0:
                if !word.isEmpty, !word.hasSuffix("?") { return true }
                word = ""
            case _ where c.isLetter || c.isNumber || c == "?" || c == "_": word.append(c)
            default: word = ""
            }
        }
        return false
    }

    /// The engine's commands the menu bar has its own items for (Undo, Redo).
    static let replaced: Set<String> = ["edit.undo", "edit.redo"]

    /// Keys the system's menus use: never given to one of ours.
    static var systemKeys: Set<String> { Set(["Cmd+A", "Cmd+C", "Cmd+V", "Cmd+X", "Cmd+Z", "Cmd+Shift+Z", "Cmd+Q", "Cmd+W", "Cmd+H", "Cmd+M",
                                              "Cmd+N", "Cmd+Shift+N", "Cmd+P", "Cmd+,", "Cmd+Tab", "Cmd+Space", "Cmd+Ctrl+F", "Cmd+Ctrl+S"]
        .compactMap { KeyShortcut.parse($0).map(KeyShortcut.name) }) }

    /// The UI commands this app does (`runUI`).
    static let uiActions: Set<String> = [
        "app.appearance.system", "app.appearance.light", "app.appearance.dark", "view.photoGrid", "view.squareGrid", "view.gridToggle",
        "view.detail", "view.compare", "view.survey", "view.people", "view.map", "view.faceBoxes", "view.reference", "photo.setReference",
        "view.filmstrip", "view.leftPanel", "view.beforeAfter", "view.beforeAfterSplit", "view.beforeAfterTopBottom",
        "view.beforeAfterSplitTopBottom", "view.clipping", "view.visualizeHdr", "view.histogram", "view.maskOverlay", "view.maskPins",
        "view.visualizeSpots", "view.cropOverlay", "view.cropOverlayOrientation", "view.back", "tool.done", "view.fullScreenPreview",
        "view.lightsOut", "view.infoOverlay", "view.navigator", "panel.edit", "panel.crop", "panel.remove", "panel.masking", "panel.redeye",
        "panel.presets", "panel.info", "panel.keywords", "panel.versions", "panel.activity", "view.syncStatus", "section.light",
        "section.color", "section.effects", "section.detail", "section.optics", "dialog.newAlbum", "dialog.newFolder", "dialog.smartAlbum",
        "dialog.newSmartAlbum", "view.slideshow", "view.gridInfo", "dialog.allMetadata", "dialog.createPreset", "dialog.autoStack",
        "dialog.copySettings", "dialog.pasteSettings", "dialog.export", "dialog.contactSheet", "dialog.mergeHdr", "dialog.mergePanorama",
        "dialog.mergeHdrPanorama", "merge.hdrLast", "merge.panoramaLast", "merge.hdrPanoramaLast", "dialog.superRes", "view.selectMode",
        "view.filterSheet", "view.tagFace", "app.settings", "app.about", "app.help", "app.discord", "app.website", "app.github",
        "app.artcraft", "app.feedback", "app.shortcuts", "dialog.rename", "dialog.labelNames", "dialog.captureTime", "app.systemInfo",
    ]

    private func uiEnabled(_ id: String) -> Bool {
        let photo = state.active != nil
        switch id {
        case _ where id.hasPrefix("panel.") || id.hasPrefix("section."): return photo
        case "view.detail", "photo.setReference", "view.lightsOut", "view.fullScreenPreview", "dialog.allMetadata",
             "dialog.createPreset", "dialog.copySettings", "dialog.superRes", "view.tagFace":
            return photo
        case "view.compare", "view.survey", "dialog.mergeHdr", "dialog.mergePanorama", "dialog.mergeHdrPanorama", "merge.hdrLast",
             "merge.panoramaLast", "merge.hdrPanoramaLast":
            return library.selected.count > 1
        case "view.reference": return reference != nil
        default: return true
        }
    }

    /// Run a UI command (the menu bar, its keys).
    func runUI(_ id: String) {
        let chosen = Array(library.selected)
        let targets = self.targets
        switch id {
        case "app.appearance.system": prefs.appearance = .system
        case "app.appearance.light": prefs.appearance = .light
        case "app.appearance.dark": prefs.appearance = .dark
        case "view.photoGrid", "view.back": path = []
        case "view.squareGrid": prefs.squareGrid.toggle()
        case "view.gridToggle": if path.isEmpty { if let a = state.active { open(a) } } else { path = [] }
        case "view.detail": if let a = state.active { open(a) }
        case "view.compare": path.append(.compare(chosen))
        case "view.survey": path.append(.survey(chosen))
        case "view.people": path.append(.people)
        case "view.map": path.append(.map)
        case "view.faceBoxes": prefs.faceBoxes.toggle()
        case "view.reference": editor.referenceView.toggle()
        case "photo.setReference":
            reference = state.active
            editor.referenceView = true
        case "view.filmstrip": prefs.filmstrip.toggle()
        case "view.navigator": prefs.navigator.toggle()
        case "view.gridInfo": prefs.gridInfo.toggle()
        case "view.leftPanel": sidebar = sidebar == .detailOnly ? .all : .detailOnly
        case "view.beforeAfter": editor.beforeAfter.toggle()
        case "view.beforeAfterSplit", "view.beforeAfterTopBottom", "view.beforeAfterSplitTopBottom":
            prefs.beforeAfterMode = ["view.beforeAfterSplit": "split", "view.beforeAfterTopBottom": "topBottom"][id] ?? "splitTopBottom"
            editor.beforeAfter = true
        case "view.clipping": prefs.showClipping.toggle()
        case "view.histogram": prefs.showHistogram.toggle()
        case "view.visualizeHdr": editor.visualizeHdr.toggle()
        case "view.maskOverlay": prefs.maskOverlay.toggle()
        case "view.maskPins": prefs.maskPins.toggle()
        case "view.visualizeSpots": editor.visualizeSpots.toggle()
        case "view.cropOverlay":
            let kinds = ["thirds", "grid", "golden", "diagonal", "triangle", "spiral", "none"]
            prefs.cropGuide = kinds[((kinds.firstIndex(of: prefs.cropGuide) ?? 0) + 1) % kinds.count]
        case "view.cropOverlayOrientation": prefs.cropGuideOrientation = (prefs.cropGuideOrientation + 1) % 4
        case "tool.done": editor.tool = nil
        case "view.fullScreenPreview", "view.lightsOut": cover = .lightsOut
        case "view.slideshow": cover = .slideshow
        case "view.infoOverlay":
            let all = Prefs.InfoOverlay.allCases
            prefs.infoOverlay = all[((all.firstIndex(of: prefs.infoOverlay) ?? 0) + 1) % all.count]
        case "panel.edit": openTool(.edit)
        case "panel.crop": openTool(.crop)
        case "panel.remove": openTool(.remove)
        case "panel.masking": openTool(.masking)
        case "panel.redeye": openTool(.redEye)
        case "panel.presets": openTool(.presets)
        case "panel.info": openTool(.info)
        case "panel.keywords": sheet = .keywords
        case "panel.versions": sheet = .history(versions: true)
        case "panel.activity": sheet = .history(versions: false)
        case "view.syncStatus": sheet = .syncStatus
        case "section.light", "section.color", "section.effects", "section.detail", "section.optics":
            prefs.editGroup = String(id.dropFirst("section.".count))
            openTool(.edit)
        case "dialog.newAlbum": sheet = .newAlbum(parent: nil, folder: false, addSelected: false)
        case "dialog.newFolder": sheet = .newAlbum(parent: nil, folder: true, addSelected: false)
        case "dialog.smartAlbum": sheet = .smartAlbum(nil, parent: nil)
        case "dialog.newSmartAlbum": sheet = .newSmartAlbumFromView
        case "dialog.allMetadata": sheet = .allMetadata
        case "dialog.createPreset": sheet = .createPreset
        case "dialog.autoStack": sheet = .autoStack
        case "dialog.copySettings": sheet = .copySettings
        case "dialog.pasteSettings": sheet = .pasteSettings(targets)
        case "dialog.export": sheet = .export(targets)
        case "dialog.contactSheet": sheet = .contactSheet(targets)
        case "dialog.mergeHdr": sheet = .merge("merge.hdr", chosen)
        case "dialog.mergePanorama": sheet = .merge("merge.panorama", chosen)
        case "dialog.mergeHdrPanorama": sheet = .merge("merge.hdrPanorama", chosen)
        case "merge.hdrLast", "merge.panoramaLast", "merge.hdrPanoramaLast":
            let command = String(id.dropLast("Last".count))
            Task { await request("host.merge.last", ["command": .string(command), "ids": AppModel.ids(chosen)]) }
        case "dialog.superRes": sheet = .superRes
        case "view.selectMode": selectMode.toggle()
        case "view.filterSheet": sheet = .filter
        case "view.tagFace":
            prefs.faceBoxes = true
            editor.tagFace = true
        case "app.settings": sheet = .settings(nil)
        case "app.about": sheet = .about
        case "app.systemInfo": sheet = .settings("systemInfo")
        case "app.help", "app.discord", "app.website", "app.github", "app.artcraft", "app.feedback":
            Task {
                guard let engine, let links = try? await engine.execute("about.links"), let url = links.array?.first(where: { $0["id"].string == id })?["url"].string,
                      let u = URL(string: url) else { return }
                await UIApplication.shared.open(u)
            }
        case "app.shortcuts": sheet = .shortcuts
        case "dialog.rename": sheet = .renamePhotos(targets)
        case "dialog.labelNames": sheet = .labelNames
        case "dialog.captureTime": sheet = .captureTime(targets)
        default: break
        }
    }

    /// Open a tool on the photo shown (the active one, opened first from the grid).
    private func openTool(_ tool: Tool) {
        if path.isEmpty, let a = state.active { open(a) }
        editor.tool = tool
    }
}

/// Help ▸ Keyboard Shortcuts: every key the menus answer, by menu.
struct ShortcutsSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                ForEach(["File", "Edit", "Photo", "View", "Window", "Help"], id: \.self) { top in
                    let items = model.menuItems(top).filter { $0.shortcut.flatMap(KeyShortcut.parse) != nil }
                    if !items.isEmpty {
                        Section(LocalizedStringKey(top)) {
                            ForEach(items, id: \.self) { item in
                                LabeledContent(LocalizedStringKey(item.label)) {
                                    Text(verbatim: Self.symbols(item.shortcut ?? "")).font(.body.monospaced()).foregroundStyle(.secondary)
                                }
                            }
                        }
                    }
                }
            }
            .navigationTitle("Keyboard Shortcuts")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .task { await model.loadMenus() }
        }
    }

    /// "Cmd+Shift+C" as the keyboard draws it: ⇧⌘C.
    static func symbols(_ s: String) -> String {
        var mods = ""
        var key = ""
        for p in s.split(separator: "+", omittingEmptySubsequences: false).map(String.init) {
            switch p {
            case "Ctrl": mods += "⌃"
            case "Alt": mods += "⌥"
            case "Shift": mods += "⇧"
            case "Cmd": mods += "⌘"
            case "Left": key = "←"
            case "Right": key = "→"
            case "Up": key = "↑"
            case "Down": key = "↓"
            case "Escape": key = "⎋"
            case "Enter", "Return": key = "↩"
            case "Delete", "Backspace": key = "⌫"
            case "": key = "+"
            default: key = p
            }
        }
        // the keyboard's order: ⌃⌥⇧⌘
        let order = "⌃⌥⇧⌘"
        let sorted = mods.sorted { (order.firstIndex(of: $0) ?? order.endIndex) < (order.firstIndex(of: $1) ?? order.endIndex) }
        return String(sorted) + key
    }
}
#endif
