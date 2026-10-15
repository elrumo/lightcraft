import Foundation
import LightCraftCore
import Observation
import SwiftUI

/// The app's own settings (how it looks and behaves, not the library's: those are engine commands),
/// kept in `UserDefaults`. The egui app keeps the same ones in its `ui.json`.
@Observable
final class Prefs {
    private let store: UserDefaults

    init(store: UserDefaults = .standard) {
        self.store = store
        appearance = Appearance(rawValue: store.string(forKey: "appearance") ?? "") ?? .system
        gridColumns = store.object(forKey: "gridColumns") as? Int ?? 3
        gridSpan = GridSpan(rawValue: store.string(forKey: "gridSpan") ?? "") ?? .all
        thumbSize = store.object(forKey: "thumbSize") as? Double ?? 160
        squareGrid = store.object(forKey: "squareGrid") as? Bool ?? true
        badges = Badges(rawValue: store.string(forKey: "badges") ?? "") ?? .auto
        infoOverlay = InfoOverlay(rawValue: store.string(forKey: "infoOverlay") ?? "") ?? .off
        autoAdvance = store.bool(forKey: "autoAdvance")
        showCounts = store.object(forKey: "showCounts") as? Bool ?? true
        editGroup = store.string(forKey: "editGroup") ?? "light"
        editStyle = EditStyle(rawValue: store.string(forKey: "editStyle") ?? "") ?? .automatic
        clickZoom = store.object(forKey: "clickZoom") as? Double ?? 2
        haptics = store.object(forKey: "haptics") as? Bool ?? true
        mapTiles = store.object(forKey: "mapTiles") as? Bool ?? true
        maskOverlayVisibility = store.string(forKey: "maskOverlayVisibility") ?? "auto"
        maskPins = store.object(forKey: "maskPins") as? Bool ?? true
        faceBoxes = store.bool(forKey: "faceBoxes")
        removeAI = store.bool(forKey: "removeAI")
        expandModel = store.string(forKey: "expandModel") ?? "auto"
        selectionEngine = store.string(forKey: "selectionEngine") ?? "auto"
        modelsDisabled = store.stringArray(forKey: "modelsDisabled") ?? []
        importOnConnect = store.object(forKey: "importOnConnect") as? Bool ?? Platform.importsOnConnect
        libraryPath = store.string(forKey: "libraryPath")
        cropGuide = store.string(forKey: "cropGuide") ?? "thirds"
        cropGuideOrientation = store.integer(forKey: "cropGuideOrientation")
        showHistogram = store.bool(forKey: "showHistogram")
        showClipping = store.bool(forKey: "showClipping")
        filmstrip = store.object(forKey: "filmstrip") as? Bool ?? Platform.showsFilmstrip
        sectionStates = store.dictionary(forKey: "sectionStates") as? [String: Bool] ?? [:]
        inspectorShown = store.object(forKey: "inspectorShown") as? Bool ?? true
        navigator = store.object(forKey: "navigator") as? Bool ?? true
        gridInfo = store.bool(forKey: "gridInfo")
        beforeAfterMode = store.string(forKey: "beforeAfterMode") ?? "auto"
        maskOverlayMode = store.string(forKey: "maskOverlayMode") ?? "color"
        maskOverlayColor = store.array(forKey: "maskOverlayColor") as? [Int] ?? [230, 30, 40]
        maskOverlayOpacity = store.object(forKey: "maskOverlayOpacity") as? Double ?? 50
        sidebarLayout = store.data(forKey: "sidebarLayout").flatMap { try? JSONDecoder().decode(JSONValue.self, from: $0) } ?? .null
        onboarded = store.bool(forKey: "onboarded")
    }

    nonisolated enum Appearance: String, CaseIterable, Identifiable {
        case system, light, dark
        var id: String { rawValue }
        var label: LocalizedStringKey {
            switch self {
            case .system: "System"
            case .light: "Light"
            case .dark: "Dark"
            }
        }
        var scheme: ColorScheme? {
            switch self {
            case .system: nil
            case .light: .light
            case .dark: .dark
            }
        }
    }

    /// Years, Months or every photo (the capsule under the grid).
    nonisolated enum GridSpan: String, CaseIterable, Identifiable {
        case years, months, all
        var id: String { rawValue }
        var label: LocalizedStringKey {
            switch self {
            case .years: "Years"
            case .months: "Months"
            case .all: "All"
            }
        }
    }

    /// When a grid cell shows its rating, flag and label.
    nonisolated enum Badges: String, CaseIterable, Identifiable {
        case auto, always, never
        var id: String { rawValue }
        var label: LocalizedStringKey {
            switch self {
            case .auto: "When Rated"
            case .always: "Always"
            case .never: "Never"
            }
        }
    }

    /// How the Edit tool's controls are set: Photos' dials (a round button per control and a ruler),
    /// Lightroom's sliders, or dials under the photo and sliders in a column beside it.
    nonisolated enum EditStyle: String, CaseIterable, Identifiable {
        case automatic, dials, sliders
        var id: String { rawValue }
        var label: LocalizedStringKey {
            switch self {
            case .automatic: "Automatic"
            case .dials: "Dials"
            case .sliders: "Sliders"
            }
        }
    }

    /// What the loupe writes over a photo.
    nonisolated enum InfoOverlay: String, CaseIterable, Identifiable {
        case off, file, exposure
        var id: String { rawValue }
        var label: LocalizedStringKey {
            switch self {
            case .off: "Off"
            case .file: "File & Date"
            case .exposure: "Exposure"
            }
        }
    }

    var appearance: Appearance { didSet { store.set(appearance.rawValue, forKey: "appearance") } }
    /// Grid columns on a phone held upright (pinch changes it).
    var gridColumns: Int { didSet { store.set(gridColumns, forKey: "gridColumns") } }
    var gridSpan: GridSpan { didSet { store.set(gridSpan.rawValue, forKey: "gridSpan") } }
    /// A Mac's thumbnails: how wide a tile is asked to be, in points (a window gives as many columns as fit).
    var thumbSize: Double { didSet { store.set(thumbSize, forKey: "thumbSize") } }
    static let thumbSizes: ClosedRange<Double> = 72...360
    var squareGrid: Bool { didSet { store.set(squareGrid, forKey: "squareGrid") } }

    /// The grid shows each photo in its own shape, as large as its square cell allows (a Mac's
    /// "aspect ratio grid", Square Thumbnails off), on the phone too.
    var aspectGrid: Bool { !squareGrid }
    var badges: Badges { didSet { store.set(badges.rawValue, forKey: "badges") } }
    var infoOverlay: InfoOverlay { didSet { store.set(infoOverlay.rawValue, forKey: "infoOverlay") } }
    /// After rating or flagging a photo in the loupe, go on to the next.
    var autoAdvance: Bool { didSet { store.set(autoAdvance, forKey: "autoAdvance") } }
    var showCounts: Bool { didSet { store.set(showCounts, forKey: "showCounts") } }
    /// The Edit tool's open group (`light`, `color`…).
    var editGroup: String { didSet { store.set(editGroup, forKey: "editGroup") } }
    var editStyle: EditStyle { didSet { store.set(editStyle.rawValue, forKey: "editStyle") } }

    /// Dials rather than sliders here (`column`: beside the photo, not under it).
    func dials(column: Bool) -> Bool {
        #if os(macOS)
        // (dials are for a finger: a Mac's panel is Lightroom's stack of sliders)
        return false
        #else
        switch editStyle {
        case .automatic: !column
        case .dials: true
        case .sliders: false
        }
        #endif
    }

    /// The Mac's panels: the sections and boxes the user opened or closed (`light`, `color`, `curve`,
    /// `mixer`…); one never touched is as its panel has it (`isOpen(_:default:)`). And whether the
    /// inspector is showing.
    var sectionStates: [String: Bool] { didSet { store.set(sectionStates, forKey: "sectionStates") } }
    var inspectorShown: Bool { didSet { store.set(inspectorShown, forKey: "inspectorShown") } }

    func isOpen(_ id: String, default open: Bool = false) -> Bool { sectionStates[id] ?? open }

    func toggleOpen(_ id: String, default open: Bool = false) { sectionStates[id] = !isOpen(id, default: open) }
    /// Double tap zooms to this (× the fitted size).
    var clickZoom: Double { didSet { store.set(clickZoom, forKey: "clickZoom") } }
    var haptics: Bool { didSet { store.set(haptics, forKey: "haptics") } }
    var mapTiles: Bool { didSet { store.set(mapTiles, forKey: "mapTiles") } }
    /// Masking: the selected mask drawn over the photo, how (`color`, `colorOnBw`, `imageOnBlack`,
    /// `imageOnWhite`, `whiteOnBlack`), in what colour (sRGB) and how strongly (0–100).
    /// "auto" (shown until the selected mask is adjusted), "always" or "never".
    var maskOverlayVisibility: String { didSet { store.set(maskOverlayVisibility, forKey: "maskOverlayVisibility") } }
    var maskOverlayMode: String { didSet { store.set(maskOverlayMode, forKey: "maskOverlayMode") } }
    var maskOverlayColor: [Int] { didSet { store.set(maskOverlayColor, forKey: "maskOverlayColor") } }
    var maskOverlayOpacity: Double { didSet { store.set(maskOverlayOpacity, forKey: "maskOverlayOpacity") } }
    /// Masking: every mask's pin on the photo.
    var maskPins: Bool { didSet { store.set(maskPins, forKey: "maskPins") } }
    /// The sidebar's sections below My Photos: order, shown, folded (`host.sidebar`'s `layout`).
    var sidebarLayout: JSONValue { didSet { store.set(try? JSONEncoder().encode(sidebarLayout), forKey: "sidebarLayout") } }
    /// The faces on a photo framed, their names under them.
    var faceBoxes: Bool { didSet { store.set(faceBoxes, forKey: "faceBoxes") } }
    /// Remove's Generative AI: strokes are filled by the MI-GAN model (when it's here).
    var removeAI: Bool { didSet { store.set(removeAI, forKey: "removeAI") } }
    /// Crop ▸ Expand's Fill With: the model Generate paints with (`crop.expandModels` ids; `auto`:
    /// the best model for this device's profile, the engine's `recommended`). Settings → AI Models
    /// sets it too.
    var expandModel: String { didSet { store.set(expandModel, forKey: "expandModel") } }
    /// What selects an object or the subject (Masking): `auto` (Apple's models where this device has
    /// them), `apple`, or `classic` (LightCraft's own guess, no model).
    var selectionEngine: String { didSet { store.set(selectionEngine, forKey: "selectionEngine") } }
    /// The AI models switched off in Settings ▸ AI Models ("Use"), by id: the engine keeps the choice only while it
    /// runs (the egui app keeps it in `ui.json`'s `models_disabled`), so the app gives it back at launch.
    var modelsDisabled: [String] { didSet { store.set(modelsDisabled, forKey: "modelsDisabled") } }
    /// Show the import when a camera or memory card is plugged in.
    var importOnConnect: Bool { didSet { store.set(importOnConnect, forKey: "importOnConnect") } }
    /// The library a Mac opens instead of the default one (chosen when the default won't open), or
    /// nil. `LIGHTCRAFT_LIBRARY` in the environment wins.
    var libraryPath: String? { didSet { store.set(libraryPath, forKey: "libraryPath") } }
    /// The crop tool's composition guide (`crop.guides` kinds) and how the asymmetric ones turn (0–3).
    var cropGuide: String { didSet { store.set(cropGuide, forKey: "cropGuide") } }
    var cropGuideOrientation: Int { didSet { store.set(cropGuideOrientation, forKey: "cropGuideOrientation") } }
    /// The histogram over the photo while editing, and the clipping warning around the photo.
    var showHistogram: Bool { didSet { store.set(showHistogram, forKey: "showHistogram") } }
    var showClipping: Bool { didSet { store.set(showClipping, forKey: "showClipping") } }
    /// The grid's photos in a row under the photo.
    var filmstrip: Bool { didSet { store.set(filmstrip, forKey: "filmstrip") } }
    /// The whole photo small in a corner while zoomed in.
    var navigator: Bool { didSet { store.set(navigator, forKey: "navigator") } }
    /// File names over the grid's photos.
    var gridInfo: Bool { didSet { store.set(gridInfo, forKey: "gridInfo") } }
    /// Before/After: auto (side by side when wide), sideBySide, topBottom, split, splitTopBottom.
    var beforeAfterMode: String { didSet { store.set(beforeAfterMode, forKey: "beforeAfterMode") } }

    /// The first launch's welcome was seen (on this device: the library's own preferences sync, this must not).
    var onboarded: Bool { didSet { store.set(onboarded, forKey: "onboarded") } }

    /// The steps a pinch moves the grid through.
    static let columnSteps = [1, 2, 3, 4, 5, 7, 10, 14]
}
