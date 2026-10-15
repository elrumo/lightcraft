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
    /// What the menus show, as the app's body read it (`init(reading:)`).
    let plan: [MenuItemSpec]
    /// There is no window to act on (a Mac's, closed).
    let off: Bool
    let sidebarSections: [SidebarSectionState]
    let undo: String?
    let redo: String?
    /// `MenuChanges.revision` as it was read: what makes the App's body run again.
    let revision: Int

    /// The menus as the model is now. Made by the App's body, which SwiftUI re-runs when the model
    /// moves: a Mac's menu bar does not follow what a Commands body or the views inside a CommandMenu
    /// read from an @Observable model, so the menus get their data as plain values.
    init(reading model: AppModel, revision: Int) {
        self.model = model
        self.revision = revision
        plan = model.menuPlan()
        #if os(macOS)
        off = model.mainWindows == 0
        #else
        off = false
        #endif
        sidebarSections = model.sidebarHere
        undo = model.state.undo
        redo = model.state.redo
    }

    var body: some Commands {
        CommandGroup(replacing: .undoRedo) {
            Button(undo.map { String(localized: "Undo \(Bundle.main.localizedString(forKey: $0, value: $0, table: nil))") } ?? String(localized: "Undo")) {
                #if os(macOS)
                // (typing in a text field: ⌘Z is the field's)
                if let field = Self.editedText { field.undoManager?.undo(); return }
                #endif
                model.undo()
            }
                .keyboardShortcut("z", modifiers: .command)
                .disabled(undoDisabled(undo))
            Button(redo.map { String(localized: "Redo \(Bundle.main.localizedString(forKey: $0, value: $0, table: nil))") } ?? String(localized: "Redo")) {
                #if os(macOS)
                if let field = Self.editedText { field.undoManager?.redo(); return }
                #endif
                model.redo()
            }
                .keyboardShortcut("z", modifiers: [.command, .shift])
                .disabled(undoDisabled(redo))
        }
        #if os(macOS)
        // About in the app menu (Settings… is the Settings scene's, ⌘,)
        CommandGroup(replacing: .appInfo) {
            Button("About LightCraft") { model.sheet = .about }
        }
        SidebarCommands()
        #endif
        #if os(macOS)
        // (no New Window: there is one main window. The first item is always there: a top-level menu
        // that is empty when the app starts is never created, however full it gets later)
        CommandGroup(replacing: .newItem) {
            Button("Import Photos…") { model.importPhotosPanel() }
                .keyboardShortcut("i", modifiers: [.command, .shift])
                .disabled(off)
            Divider()
            section(plan, "File", off)
        }
        #else
        CommandGroup(after: .newItem) { section(plan, "File", off) }
        #endif
        CommandGroup(after: .pasteboard) { section(plan, "Edit", off) }
        CommandMenu("Photo") { section(plan, "Photo", off) }
        #if os(macOS)
        // the tools and panels egui lists under Window: a Mac's Window menu is the system's
        CommandMenu("Tools") { section(plan, "Tools", off) }
        #endif
        CommandGroup(after: .sidebar) {
            section(plan, "View", off)
            SidebarSectionsMenu(model: model, sections: sidebarSections)
        }
        CommandGroup(after: .windowArrangement) { section(plan, "Window", off) }
        CommandGroup(replacing: .help) { section(plan, "Help", off) }
    }

    private func section(_ plan: [MenuItemSpec], _ top: String, _ off: Bool) -> MenuSection {
        MenuSection(model: model, items: plan.filter { $0.path.first == top }, off: off)
    }

    /// Undo or Redo has nothing to do (a Mac's text field has its own, so they stay on while one is edited).
    private func undoDisabled(_ step: String?) -> Bool {
        #if os(macOS)
        if Self.editedText != nil { return false }
        #endif
        return step == nil
    }

    #if os(macOS)
    /// The text being edited in the key window (a field's editor), if any.
    private static var editedText: NSText? { NSApp.keyWindow?.firstResponder as? NSText }
    #endif
}

/// A menu's items, its submenus as menus of their own.
private struct MenuSection: View {
    let model: AppModel
    let items: [MenuItemSpec]
    let off: Bool

    var body: some View {
        let direct = items.filter { $0.path.count == 1 }
        let subs = items.compactMap { $0.path.count > 1 ? $0.path[1] : nil }.reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }
        ForEach(direct, id: \.self) { item in MenuButton(model: model, item: item, off: off) }
        ForEach(subs, id: \.self) { sub in
            Menu(LocalizedStringKey(sub)) {
                ForEach(items.filter { $0.path.count > 1 && $0.path[1] == sub }, id: \.self) { item in MenuButton(model: model, item: item, off: off) }
            }
        }
    }
}

private struct MenuButton: View {
    let model: AppModel
    let item: MenuItemSpec
    /// There is no window to act on (a Mac's, closed).
    let off: Bool

    var body: some View {
        let button = Button(LocalizedStringKey(item.label)) {
            if item.engine { model.runCommand(item.id) } else { model.runUI(item.id) }
        }
        .disabled(!item.enabled || off)
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
            #if os(macOS)
            if key.count >= 2, key.first == "F", let n = Int(key.dropFirst()), (1...12).contains(n),
               let function = UnicodeScalar(0xF704 + UInt32(n - 1)) {
                // F1…F12: AppKit's function-key characters (NSF1FunctionKey…)
                return KeyboardShortcut(KeyEquivalent(Character(function)), modifiers: modifiers)
            }
            #endif
            // one character (an iPad's menus can't take F-keys)
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
        if keyTable == nil {
            keyTable = try? await engine.request("host.keyBindings").decode(FixedKey.Table.self)
        }
        engineCommands = (try? await engine.commands()) ?? []
        menuEngine = engineCommands.filter { !$0.menu.isEmpty }
        rebuildKeyMap()
        menuChanges.bump()
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
            #if os(macOS)
            // (the app menu has Settings… and About; the Window menu is the system's: the tools get one of their own)
            if Self.macOwned.contains(c.id) { return nil }
            if path.first == "Window", c.id != "view.secondWindow" {
                let rest = path.dropFirst().first == "Tools" ? Array(path.dropFirst(2)) : Array(path.dropFirst())
                return MenuItemSpec(id: c.id, label: c.label, shortcut: c.shortcut, engine: false, enabled: uiEnabled(c.id), path: ["Tools"] + rest)
            }
            #endif
            return MenuItemSpec(id: c.id, label: c.label, shortcut: c.shortcut, engine: false, enabled: uiEnabled(c.id), path: path)
        }
        // An engine command and a UI command of the same name ("New Album", "New Album…"): the UI one
        // is the dialog, the engine's takes no input and would only be a second item for it
        let uiNames = Set(ui.map { Self.plainLabel($0.label) + "/" + ($0.path.first ?? "") })
        let engineOnly = engine.filter { !uiNames.contains(Self.plainLabel($0.label) + "/" + ($0.path.first ?? "")) }
        var taken = Self.systemKeys
        return (engineOnly + ui).map { item in
            guard let sc = item.shortcut, let key = KeyShortcut.parse(sc) else { return item }
            let name = KeyShortcut.name(key)
            if taken.contains(name) {
                return MenuItemSpec(id: item.id, label: item.label, shortcut: nil, engine: item.engine, enabled: item.enabled, path: item.path)
            }
            taken.insert(name)
            return item
        }
    }

    /// A label without its trailing ellipsis, for telling two commands of one name apart.
    private static func plainLabel(_ label: String) -> String {
        label.replacingOccurrences(of: "…", with: "").replacingOccurrences(of: "...", with: "").trimmingCharacters(in: .whitespaces).lowercased()
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

    /// What the Mac's own menus already have (Settings… is the Settings scene's, About is in the app menu).
    static let macOwned: Set<String> = ["app.settings", "app.about"]

    /// Keys the system's menus use: never given to one of ours.
    static var systemKeys: Set<String> {
        var keys = ["Cmd+A", "Cmd+C", "Cmd+V", "Cmd+X", "Cmd+Z", "Cmd+Shift+Z", "Cmd+Q", "Cmd+W", "Cmd+H", "Cmd+M",
                    "Cmd+P", "Cmd+,", "Cmd+Tab", "Cmd+Space", "Cmd+Ctrl+F", "Cmd+Ctrl+S"]
        #if os(iOS)
        // (an iPad's New Window; a Mac has one main window: ⌘N is New Album, as in the egui app)
        keys += ["Cmd+N", "Cmd+Shift+N"]
        #endif
        return Set(keys.compactMap { KeyShortcut.parse($0).map(KeyShortcut.name) })
    }

    /// The UI commands this app does (`runUI`).
    static let uiActions: Set<String> = [
        "app.appearance.system", "app.appearance.light", "app.appearance.dark", "view.photoGrid", "view.squareGrid", "view.gridToggle",
        "view.detail", "view.compare", "view.survey", "view.people", "view.map", "view.faceBoxes", "view.reference", "photo.setReference",
        "view.filmstrip", "view.leftPanel", "view.beforeAfter", "view.beforeAfterSplit", "view.beforeAfterTopBottom",
        "view.beforeAfterSplitTopBottom", "view.clipping", "view.visualizeHdr", "view.histogram", "view.maskOverlay", "view.maskOverlayAll", "view.maskPins",
        "view.visualizeSpots", "view.cropOverlay", "view.cropOverlayOrientation", "view.back", "tool.done", "view.fullScreenPreview",
        "view.lightsOut", "view.infoOverlay", "view.navigator", "panel.edit", "panel.crop", "panel.remove", "panel.masking", "panel.redeye",
        "panel.presets", "panel.info", "panel.keywords", "panel.versions", "panel.activity", "view.syncStatus", "section.light",
        "section.color", "section.effects", "section.detail", "section.optics", "dialog.newAlbum", "dialog.newFolder", "dialog.smartAlbum",
        "dialog.newSmartAlbum", "view.slideshow", "view.gridInfo", "dialog.allMetadata", "dialog.createPreset", "dialog.autoStack",
        "dialog.copySettings", "dialog.pasteSettings", "dialog.export", "dialog.contactSheet", "dialog.mergeHdr", "dialog.mergePanorama",
        "dialog.mergeHdrPanorama", "merge.hdrLast", "merge.panoramaLast", "merge.hdrPanoramaLast", "dialog.superRes", "view.selectMode",
        "view.filterSheet", "view.tagFace", "app.settings", "app.about", "app.help", "app.discord", "app.website", "app.github",
        "app.artcraft", "app.feedback", "app.shortcuts", "dialog.rename", "dialog.labelNames", "dialog.captureTime", "app.systemInfo",
        "dialog.cull", "photo.tagFromTracklog", "file.findMissing", "photo.locate", "app.exportPrevious", "view.softProof", "file.importLightroom",
        "view.focusSearch", "view.secondWindow", "view.zoomToggle", "view.zoomFit", "view.zoom100", "view.zoomIn", "view.zoomOut",
        "view.maskOverlayColor", "tool.brush", "tool.linear", "tool.radial", "tool.wbPicker", "brush.smaller", "brush.larger",
        "brush.featherLess", "brush.featherMore",
    ]

    func uiEnabled(_ id: String) -> Bool {
        let photo = state.active != nil
        switch id {
        case "view.zoomFit", "view.zoom100", "view.zoomIn", "view.zoomOut": return inLoupe
        case _ where id.hasPrefix("panel.") || id.hasPrefix("section."): return photo
        case "view.detail", "photo.setReference", "view.lightsOut", "view.fullScreenPreview", "dialog.allMetadata",
             "dialog.createPreset", "dialog.copySettings", "dialog.superRes", "view.tagFace", "tool.brush", "tool.linear", "tool.radial",
             "tool.wbPicker":
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
        case "view.photoGrid": path = []
        case "view.back":
            // (Esc: the Keyword Painter is put down, a tool's own screen is cancelled, a person's page goes back to the people)
            if painting != nil { painting = nil } else if inLoupe, editor.tool?.hasScreen == true { cancelToolScreen() } else if case .person = path.last { path.removeLast() } else { path = [] }
        case "view.squareGrid": prefs.squareGrid.toggle()
        case "view.gridToggle":
            // (G: the grid from any other screen, as in Lightroom; in the grid itself, egui's switch between its two grids)
            if path.isEmpty { prefs.squareGrid.toggle() } else { path = [] }
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
        case "view.maskOverlay": prefs.maskOverlayVisibility = ["auto": "always", "always": "never"][prefs.maskOverlayVisibility] ?? "auto"
        case "view.maskOverlayAll": editor.maskOverlayAll.toggle()
        case "view.maskPins": prefs.maskPins.toggle()
        case "view.visualizeSpots": editor.visualizeSpots.toggle()
        case "view.cropOverlay":
            let kinds = ["thirds", "grid", "golden", "diagonal", "triangle", "spiral", "none"]
            prefs.cropGuide = kinds[((kinds.firstIndex(of: prefs.cropGuide) ?? 0) + 1) % kinds.count]
        case "view.cropOverlayOrientation": prefs.cropGuideOrientation = (prefs.cropGuideOrientation + 1) % 4
        case "tool.done":
            if inLoupe, editor.tool?.hasScreen == true {
                perform("develop.endInteraction")
                editor.tool = .edit
            } else if !inLoupe {
                // (Return: the photo that is chosen, large)
                if path.isEmpty || cullScreen, let a = state.active { open(a) }
            } else {
                #if os(iOS)
                editor.tool = nil
                #endif
            }
        case "view.zoomToggle":
            // (Space and Z: in the loupe the zoom a click gives, and back; in the grid the photo, large)
            if inLoupe { editor.zoomAsk = editor.zoomedIn ? .fit : .fitted(prefs.clickZoom) } else if path.isEmpty || cullScreen, let a = state.active { open(a) }
        case "view.zoomFit": editor.zoomAsk = .fit
        case "view.zoom100": editor.zoomAsk = .actual(1)
        case "view.zoomIn": editor.zoomAsk = .by(1.25)
        case "view.zoomOut": editor.zoomAsk = .by(0.8)
        case "view.panels": editor.review.toggle()
        case "tool.brush", "tool.linear", "tool.radial":
            // (B, L and R start a mask in the Masking tool)
            openTool(.masking)
            createMask(["tool.brush": "brush", "tool.linear": "linear", "tool.radial": "radial"][id] ?? "brush")
        case "tool.wbPicker":
            openTool(.edit)
            editor.arm(.whiteBalance, self)
        case "brush.smaller", "brush.larger", "brush.featherLess", "brush.featherMore":
            // ([ ] size the brush a fifth at a time, Shift with them its feather ten at a time: egui's steps)
            let grow = id == "brush.larger" ? 1.2 : id == "brush.smaller" ? 1 / 1.2 : 1
            let feather = id == "brush.featherMore" ? 10.0 : id == "brush.featherLess" ? -10 : 0
            switch editor.tool {
            case .masking:
                editor.brushSize = min(max(editor.brushSize * grow, 1 / 400), 100 / 400)
                editor.brushFeather = min(max(editor.brushFeather + feather, 0), 100)
            case .remove:
                editor.removeSize = min(max(editor.removeSize * grow, 0.001), 0.25)
                editor.removeFeather = min(max(editor.removeFeather + feather, 0), 100)
            default: break
            }
        case "nav.first", "nav.last": goToEnd(last: id == "nav.last")
        case "view.maskOverlayColor":
            let colors: [[Int]] = [[230, 30, 40], [0, 200, 80], [60, 120, 255], [255, 210, 0], [255, 0, 200]]
            prefs.maskOverlayColor = colors[((colors.firstIndex(of: prefs.maskOverlayColor) ?? -1) + 1) % colors.count]
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
                Platform.open(u)
            }
        case "app.shortcuts": sheet = .shortcuts
        case "dialog.rename": sheet = .renamePhotos(targets)
        case "dialog.labelNames": sheet = .labelNames
        case "dialog.captureTime": sheet = .captureTime(targets)
        case "dialog.cull": sheet = .cull(chosen)
        case "view.softProof": editor.softProof.toggle()
        case "view.secondWindow": secondWindowAsked = true
        case "view.focusSearch":
            path = []
            searching = true
        case "photo.tagFromTracklog": picker = .tracklog(targets)
        case "file.findMissing": picker = .findMissing
        case "file.importLightroom": picker = .lightroom
        case "photo.locate": if let a = state.active { picker = .locate(a) }
        case "app.exportPrevious": Task { await exportPrevious() }
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
                let fixed = model.fixedKeyRows()
                ForEach(fixed.map { $0.section }.reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }, id: \.self) { section in
                    Section(section) {
                        ForEach(fixed.filter { $0.section == section }) { row in
                            LabeledContent(row.label) {
                                Text(verbatim: Self.symbols(row.shortcut)).font(.body.monospaced()).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
                ForEach(["File", "Edit", "Photo", "View", "Window", "Tools", "Help"], id: \.self) { top in
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
            .inlineTitle()
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button(role: .confirm) { dismiss() } } }
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
