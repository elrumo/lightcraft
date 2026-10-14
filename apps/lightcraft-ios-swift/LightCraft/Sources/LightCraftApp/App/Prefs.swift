#if os(iOS)
import Foundation
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
        squareGrid = store.object(forKey: "squareGrid") as? Bool ?? true
        badges = Badges(rawValue: store.string(forKey: "badges") ?? "") ?? .auto
        infoOverlay = InfoOverlay(rawValue: store.string(forKey: "infoOverlay") ?? "") ?? .off
        autoAdvance = store.bool(forKey: "autoAdvance")
        showCounts = store.object(forKey: "showCounts") as? Bool ?? true
        editGroup = store.string(forKey: "editGroup") ?? "light"
        clickZoom = store.object(forKey: "clickZoom") as? Double ?? 2
        haptics = store.object(forKey: "haptics") as? Bool ?? true
        mapTiles = store.object(forKey: "mapTiles") as? Bool ?? true
        maskOverlay = store.object(forKey: "maskOverlay") as? Bool ?? true
        maskPins = store.object(forKey: "maskPins") as? Bool ?? true
        faceBoxes = store.bool(forKey: "faceBoxes")
        removeAI = store.bool(forKey: "removeAI")
        cropGuide = store.string(forKey: "cropGuide") ?? "thirds"
        cropGuideOrientation = store.integer(forKey: "cropGuideOrientation")
        showHistogram = store.bool(forKey: "showHistogram")
        showClipping = store.bool(forKey: "showClipping")
        maskOverlayMode = store.string(forKey: "maskOverlayMode") ?? "color"
        maskOverlayColor = store.array(forKey: "maskOverlayColor") as? [Int] ?? [230, 30, 40]
        maskOverlayOpacity = store.object(forKey: "maskOverlayOpacity") as? Double ?? 50
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
    var squareGrid: Bool { didSet { store.set(squareGrid, forKey: "squareGrid") } }
    var badges: Badges { didSet { store.set(badges.rawValue, forKey: "badges") } }
    var infoOverlay: InfoOverlay { didSet { store.set(infoOverlay.rawValue, forKey: "infoOverlay") } }
    /// After rating or flagging a photo in the loupe, go on to the next.
    var autoAdvance: Bool { didSet { store.set(autoAdvance, forKey: "autoAdvance") } }
    var showCounts: Bool { didSet { store.set(showCounts, forKey: "showCounts") } }
    /// The Edit tool's open group (`light`, `color`…).
    var editGroup: String { didSet { store.set(editGroup, forKey: "editGroup") } }
    /// Double tap zooms to this (× the fitted size).
    var clickZoom: Double { didSet { store.set(clickZoom, forKey: "clickZoom") } }
    var haptics: Bool { didSet { store.set(haptics, forKey: "haptics") } }
    var mapTiles: Bool { didSet { store.set(mapTiles, forKey: "mapTiles") } }
    /// Masking: the selected mask drawn over the photo, how (`color`, `colorOnBw`, `imageOnBlack`,
    /// `imageOnWhite`, `whiteOnBlack`), in what colour (sRGB) and how strongly (0–100).
    var maskOverlay: Bool { didSet { store.set(maskOverlay, forKey: "maskOverlay") } }
    var maskOverlayMode: String { didSet { store.set(maskOverlayMode, forKey: "maskOverlayMode") } }
    var maskOverlayColor: [Int] { didSet { store.set(maskOverlayColor, forKey: "maskOverlayColor") } }
    var maskOverlayOpacity: Double { didSet { store.set(maskOverlayOpacity, forKey: "maskOverlayOpacity") } }
    /// Masking: every mask's pin on the photo.
    var maskPins: Bool { didSet { store.set(maskPins, forKey: "maskPins") } }
    /// The faces on a photo framed, their names under them.
    var faceBoxes: Bool { didSet { store.set(faceBoxes, forKey: "faceBoxes") } }
    /// Remove's Generative AI: strokes are filled by the MI-GAN model (when it's here).
    var removeAI: Bool { didSet { store.set(removeAI, forKey: "removeAI") } }
    /// The crop tool's composition guide (`crop.guides` kinds) and how the asymmetric ones turn (0–3).
    var cropGuide: String { didSet { store.set(cropGuide, forKey: "cropGuide") } }
    var cropGuideOrientation: Int { didSet { store.set(cropGuideOrientation, forKey: "cropGuideOrientation") } }
    /// The histogram over the photo while editing, and the clipping warning around the photo.
    var showHistogram: Bool { didSet { store.set(showHistogram, forKey: "showHistogram") } }
    var showClipping: Bool { didSet { store.set(showClipping, forKey: "showClipping") } }

    /// The steps a pinch moves the grid through.
    static let columnSteps = [1, 2, 3, 4, 5, 7, 10, 14]
}
#endif
