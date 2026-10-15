import Foundation
import Testing

@testable import LightCraftCore

/// The Mac's keyboard without a Mac: spellings, the key map and what a key means where you are.
struct KeysTests {
    // MARK: spellings

    @Test func aShortcutIsItsPress() {
        #expect(KeyChord(shortcut: "Cmd+Shift+Z") == KeyChord("z", command: true, shift: true))
        #expect(KeyChord(shortcut: "Right") == KeyChord("right"))
        #expect(KeyChord(shortcut: "Enter") == KeyChord("return"), "Enter and Return are one key")
        #expect(KeyChord(shortcut: "Delete") == KeyChord("backspace"), "⌫ is Delete")
        #expect(KeyChord(shortcut: "Escape") == KeyChord("esc"))
        #expect(KeyChord(shortcut: "Space") == KeyChord("space"))
        #expect(KeyChord(shortcut: "F6")?.isFunctionKey == true)
        #expect(KeyChord(shortcut: "F")?.isFunctionKey == false, "the letter F is not a function key")
    }

    @Test func shiftedSymbolsAreTheirPlainKeyWithShift() {
        #expect(KeyChord(shortcut: "Shift+{") == KeyChord(shortcut: "Shift+["))
        #expect(KeyChord("{") == KeyChord("[", shift: true), "a press that types a brace is Shift+[")
        #expect(KeyChord(shortcut: "Shift+}") == KeyChord("]", shift: true))
        #expect(KeyChord(shortcut: "Cmd+=") == KeyChord("=", command: true))
        #expect(KeyChord(shortcut: "Cmd++") == KeyChord("=", command: true, shift: true), "Cmd and the plus key: Cmd+Shift+=")
    }

    @Test func aBadSpellingNamesNothing() {
        #expect(KeyChord(shortcut: "") == nil)
        #expect(KeyChord(shortcut: "Hyper+K") == nil)
        #expect(KeyChord(shortcut: "Cmd+") == KeyChord("=", command: true, shift: true))
    }

    @Test func aChordIsSpelledAsEguiSpellsIt() {
        #expect(KeyChord("z", command: true, shift: true).description == "Cmd+Shift+Z")
        #expect(KeyChord("left").description == "Left")
        #expect(KeyChord("return").description == "Enter")
        #expect(KeyChord("1", option: true).description == "Alt+1")
        #expect(KeyChord("[", shift: true).description == "Shift+[")
        for spelling in ["Cmd+Alt+Shift+E", "Ctrl+H", "Space", "F6", "Cmd+,", "\\", "/"] {
            #expect(KeyChord(shortcut: spelling)?.description == spelling, "\(spelling)")
        }
    }

    @Test func theMenusKeysAreTheOnesATextFieldNeverWants() {
        #expect(KeyChord(shortcut: "Cmd+C")?.isMenuKey == true)
        #expect(KeyChord(shortcut: "Ctrl+H")?.isMenuKey == true)
        #expect(KeyChord(shortcut: "P")?.isMenuKey == false)
        #expect(KeyChord(shortcut: "Shift+X")?.isMenuKey == false)
    }

    @Test func aKeyIsDrawnAsCaps() {
        #expect(KeyChord(shortcut: "Cmd+Shift+C")?.caps == ["⇧", "⌘", "C"], "the keyboard's order, ⇧ before ⌘")
        #expect(KeyChord(shortcut: "Cmd+Alt+Shift+E")?.caps == ["⌥", "⇧", "⌘", "E"])
        #expect(KeyChord(shortcut: "Ctrl+H")?.caps == ["⌃", "H"])
        #expect(KeyChord(shortcut: "Right")?.caps == ["→"])
        #expect(KeyChord(shortcut: "Delete")?.caps == ["⌫"])
        #expect(KeyChord(shortcut: "Space")?.caps == ["Space"])
        #expect(KeyChord(shortcut: "F6")?.caps == ["F6"])
        #expect(KeyChord(shortcut: "Shift+{")?.caps == ["⇧", "["], "a brace is Shift with its plain key")
        #expect(KeyChord(shortcut: "Home")?.caps == ["Home"])
        #expect(KeyChord(shortcut: "Escape")?.caps == ["esc"], "the key's own word, as it is printed on it")
    }

    @Test func keysAreSortedForTheInputMonitor() {
        #expect(KeyChord("left").isArrow && KeyChord("left").isNavigation && KeyChord("left").repeats)
        #expect(!KeyChord("left", command: true).repeats, "a held ⌘← does not run again")
        #expect(KeyChord("[").repeats && KeyChord("]").repeats, "the brush keys go on while held")
        #expect(!KeyChord("3").repeats, "a rating is set once")
        #expect(KeyChord("x").isTyping && KeyChord("x", shift: true).isTyping)
        #expect(!KeyChord("x", command: true).isTyping && !KeyChord("x", option: true).isTyping && !KeyChord("x", control: true).isTyping)
        #expect(!KeyChord("space").isTyping && !KeyChord("left").isTyping, "named keys are not letters")
        #expect(KeyChord("space").isNavigation && KeyChord("return").isNavigation && KeyChord("escape").isNavigation && KeyChord("tab").isNavigation)
        #expect(!KeyChord("x").isNavigation && !KeyChord("3").isNavigation)
    }

    // MARK: the key map

    private let fixed = [
        FixedKey(shortcut: "3", command: "photo.rate", params: ["rating": 3]),
        FixedKey(shortcut: "Shift+X", command: "photo.flag", params: ["flag": "reject", "advance": true]),
        FixedKey(shortcut: "Space", command: "view.zoomToggle"),
    ]

    private func map() -> KeyMap {
        KeyMap(
            ui: [("view.zoomToggle", "Z"), ("view.gridToggle", "G"), ("panel.crop", "C"), ("view.back", "Escape"), ("view.noKey", nil)],
            engine: [("photo.pick", "P"), ("photo.reject", "X"), ("crop.rotateAspect", "X"), ("library.next", "Right"), ("tool.wb", "G")],
            fixed: fixed
        )
    }

    @Test func aKeyIsBoundToItsCommands() {
        let keys = map()
        #expect(keys.bindings(for: KeyChord("p")).map(\.command) == ["photo.pick"])
        #expect(keys.bindings(for: KeyChord("p")).first?.origin == .engine)
        #expect(keys.bindings(for: KeyChord("c")).first?.origin == .ui)
        #expect(keys.bindings(for: KeyChord("q")).isEmpty)
    }

    @Test func aKeyTwoCommandsShareHasBoth() {
        #expect(map().bindings(for: KeyChord("x")).map(\.command) == ["photo.reject", "crop.rotateAspect"])
    }

    @Test func aUiCommandsKeyIsNotTheEnginesToo() {
        #expect(map().bindings(for: KeyChord("g")).map(\.command) == ["view.gridToggle"], "the engine's tool.wb gives G up")
    }

    @Test func theFixedKeysCarryTheirParamsAndKnowWhoRunsThem() {
        let keys = map()
        let three = keys.bindings(for: KeyChord("3")).first
        #expect(three?.command == "photo.rate" && three?.params == ["rating": 3] && three?.origin == .engine)
        let shifted = keys.bindings(for: KeyChord("x", shift: true)).first
        #expect(shifted?.params["advance"].bool == true)
        let space = keys.bindings(for: KeyChord("space")).first
        #expect(space?.origin == .ui, "Space names a UI command: the list of UI commands says so")
    }

    @Test func theHostsTableDecodes() throws {
        let json = #"{"fixed": [{"shortcut": "3", "command": "photo.rate", "params": {"rating": 3}}], "contextual": [["photo.reject", "crop.rotateAspect"]]}"#
        let table = try JSONDecoder().decode(FixedKey.Table.self, from: Data(json.utf8))
        #expect(table.fixed == [FixedKey(shortcut: "3", command: "photo.rate", params: ["rating": 3])])
        #expect(table.contextual == [["photo.reject", "crop.rotateAspect"]])
    }

    // MARK: what a key means where you are

    private func ruling(_ shortcut: String, _ context: KeyContext) -> KeyRules.Ruling? {
        guard let chord = KeyChord(shortcut: shortcut) else { return nil }
        return KeyRules.ruling(map().bindings(for: chord), in: context)
    }

    @Test func xRejectsAndInTheCropToolSwapsTheAspect() {
        #expect(ruling("X", KeyContext(screen: .loupe, tool: "edit")) == .run("photo.reject", [:], ui: false))
        #expect(ruling("X", KeyContext(screen: .grid)) == .run("photo.reject", [:], ui: false))
        #expect(ruling("X", KeyContext(screen: .loupe, tool: "crop")) == .run("crop.rotateAspect", [:], ui: false))
    }

    @Test func aPlainKeyRunsItsCommand() {
        #expect(ruling("3", KeyContext(screen: .grid)) == .run("photo.rate", ["rating": 3], ui: false))
        #expect(ruling("C", KeyContext(screen: .loupe)) == .run("panel.crop", [:], ui: true))
        #expect(ruling("Q", KeyContext(screen: .loupe)) == nil, "no binding, no ruling")
    }

    private func rules(_ command: String, _ context: KeyContext, origin: KeyBinding.Origin = .engine) -> KeyRules.Ruling? {
        KeyRules.ruling([KeyBinding(command: command, origin: origin)], in: context)
    }

    @Test func deleteActsOnWhatIsBeingEdited() {
        #expect(rules("photo.delete", KeyContext(screen: .loupe, tool: "masking", activeMask: true)) == .run("mask.delete", [:], ui: false))
        #expect(rules("photo.delete", KeyContext(screen: .loupe, tool: "masking")) == .ignore, "no mask chosen: never the photo")
        #expect(rules("photo.delete", KeyContext(screen: .loupe, tool: "remove", activeSpot: true)) == .run("spot.delete", [:], ui: false))
        #expect(rules("photo.delete", KeyContext(screen: .loupe, tool: "remove")) == .ignore)
        #expect(rules("photo.delete", KeyContext(screen: .loupe, tool: "redEye")) == .ignore)
        #expect(rules("photo.delete", KeyContext(screen: .grid)) == .run("photo.delete", [:], ui: false))
        #expect(rules("photo.delete", KeyContext(screen: .loupe, tool: "edit")) == .run("photo.delete", [:], ui: false))
        // ⌫ is the engine's album.removePhotos: out of the album its grid shows, else it deletes
        #expect(rules("album.removePhotos", KeyContext(screen: .grid, inAlbum: true)) == .run("album.removePhotos", [:], ui: false))
        #expect(rules("album.removePhotos", KeyContext(screen: .grid)) == .run("photo.delete", [:], ui: false))
        #expect(rules("album.removePhotos", KeyContext(screen: .loupe, tool: "masking", activeMask: true)) == .run("mask.delete", [:], ui: false))
        #expect(rules("album.removePhotos", KeyContext(screen: .loupe, tool: "redEye")) == .ignore)
        // ⌘⌫ deletes, album or not
        #expect(rules("photo.delete", KeyContext(screen: .grid, inAlbum: true)) == .run("photo.delete", [:], ui: false))
    }

    @Test func theGridsKeysAreNotTheLoupes() {
        let grid = KeyContext(screen: .grid)
        let loupe = KeyContext(screen: .loupe, tool: "edit")
        #expect(rules("brush.smaller", grid) == .run("photo.decreaseRating", [:], ui: false), "[ rates down in the grid")
        #expect(rules("brush.smaller", loupe) == .run("brush.smaller", [:], ui: false), "and sizes the brush elsewhere")
        #expect(rules("tool.brush", grid) == .run("album.toggleTarget", [:], ui: false))
        #expect(rules("view.softProof", grid, origin: .ui) == .run("stack.toggle", [:], ui: false))
        #expect(rules("view.softProof", loupe, origin: .ui) == .run("view.softProof", [:], ui: true))
        #expect(rules("panel.presets", grid, origin: .ui) == .run("photo.flag", ["flag": "pick", "advance": true], ui: false))
    }

    @Test func theToolsOwnKeys() {
        let crop = KeyContext(screen: .loupe, tool: "crop")
        #expect(rules("view.maskOverlay", crop, origin: .ui) == .run("view.cropOverlay", [:], ui: true), "O cycles the crop's guides")
        #expect(rules("view.cropOverlay", crop, origin: .ui) == .run("view.cropOverlayOrientation", [:], ui: true))
        #expect(rules("view.visualizeSpots", crop, origin: .ui) == .run("crop.aspect", ["aspect": "toggle"], ui: false), "A locks the aspect")
        let masking = KeyContext(screen: .loupe, tool: "masking")
        #expect(rules("view.cropOverlay", masking, origin: .ui) == .run("view.maskOverlayColor", [:], ui: true))
        let spot = KeyContext(screen: .loupe, tool: "remove", activeSpot: true)
        #expect(rules("view.filmstrip", spot, origin: .ui) == .run("spot.refreshSource", [:], ui: false))
        #expect(rules("view.filmstrip", KeyContext(screen: .loupe, tool: "remove"), origin: .ui) == .run("view.filmstrip", [:], ui: true))
    }

    // MARK: arrows in the grid

    private func groups(_ counts: [Int]) -> [DateGroup] {
        var start = 0
        return counts.enumerated().map { i, n in
            defer { start += n }
            return DateGroup(key: "g\(i)", label: "g\(i)", start: start, count: n)
        }
    }

    @Test func downAndUpInOneRun() {
        // 10 photos, 4 across: rows 0-3, 4-7, 8-9
        let none: [DateGroup] = []
        #expect(GridNavigation.rowDistance(from: 1, down: true, across: 4, groups: none, count: 10) == 4)
        #expect(GridNavigation.rowDistance(from: 5, down: true, across: 4, groups: none, count: 10) == 4)
        #expect(GridNavigation.rowDistance(from: 7, down: true, across: 4, groups: none, count: 10) == 2, "the short last row: its last cell")
        #expect(GridNavigation.rowDistance(from: 9, down: true, across: 4, groups: none, count: 10) == 0, "the last row stays")
        #expect(GridNavigation.rowDistance(from: 9, down: false, across: 4, groups: none, count: 10) == 4)
        #expect(GridNavigation.rowDistance(from: 2, down: false, across: 4, groups: none, count: 10) == 0, "the first row stays")
    }

    @Test func downAndUpAcrossDateHeaders() {
        // groups of 5 and 3 and 4, 3 across: [0 1 2 / 3 4] [5 6 7] [8 9 10 / 11]
        let g = groups([5, 3, 4])
        #expect(GridNavigation.rowDistance(from: 4, down: true, across: 3, groups: g, count: 12) == 2, "the group's last row: the next group's first row, same column (6)")
        #expect(GridNavigation.rowDistance(from: 2, down: true, across: 3, groups: g, count: 12) == 2, "row 0 column 2 of a group of 5: below it is only column 1, so the last cell")
        #expect(GridNavigation.rowDistance(from: 7, down: true, across: 3, groups: g, count: 12) == 3, "column 2 of the next group's first row is 10")
        #expect(GridNavigation.rowDistance(from: 5, down: false, across: 3, groups: g, count: 12) == 2, "up from a group's first row: the previous group's last row, same column (3 is column 0)")
        #expect(GridNavigation.rowDistance(from: 7, down: false, across: 3, groups: g, count: 12) == 3, "column 2 of the previous group's last row is missing: its last cell")
        #expect(GridNavigation.rowDistance(from: 11, down: false, across: 3, groups: g, count: 12) == 3)
        #expect(GridNavigation.rowDistance(from: 1, down: false, across: 3, groups: g, count: 12) == 0)
        #expect(GridNavigation.rowDistance(from: 11, down: true, across: 3, groups: g, count: 12) == 0)
    }

    @Test func hostileInputsDoNotMoveOrTrap() {
        let g = groups([4])
        #expect(GridNavigation.rowDistance(from: -1, down: true, across: 3, groups: g, count: 4) == 0)
        #expect(GridNavigation.rowDistance(from: 4, down: true, across: 3, groups: g, count: 4) == 0)
        #expect(GridNavigation.rowDistance(from: 0, down: true, across: 0, groups: g, count: 4) == 1, "no columns is one column")
        #expect(GridNavigation.rowDistance(from: 0, down: true, across: Int.max / 2, groups: [], count: 4) == 0, "a grid one row wide has no row below")
        let stale = [DateGroup(key: "x", label: "x", start: -5, count: 1 << 40)]
        #expect(GridNavigation.rowDistance(from: 2, down: true, across: 2, groups: stale, count: 6) >= 0)
        #expect(GridNavigation.rowDistance(from: 0, down: true, across: 2, groups: [], count: 0) == 0)
    }
}
