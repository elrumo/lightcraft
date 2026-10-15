import Foundation

#if canImport(CoreGraphics)
import CoreGraphics
#endif

/// An iPhone Duo's hinge across a view, partly open: the strip to keep clear (the hinge and its margins), in the
/// coordinates of the view that measured it.
public struct DuoFold: Equatable, Sendable {
    public var frame: CGRect
    /// The hinge runs top to bottom (a book held open: pages left and right), else left to right (a tabletop).
    public var vertical: Bool

    public init(frame: CGRect, vertical: Bool) {
        self.frame = frame
        self.vertical = vertical
    }
}

/// How the photo screen arranges the photo and its tools in the room it has. Presentation only: the rules that
/// size classes alone cannot give on a device with two displays and a hinge (docs/ios-swift.md → iPhone Duo).
public enum PhotoLayout: Equatable, Sendable {
    /// The system's inspector beside the photo: a wide window on a device without a hinge (an iPad).
    case inspector
    /// A column of `width` points beside the photo, after `gap` points kept clear. A phone held sideways has no gap;
    /// a book (hinge top to bottom) puts the photo on its left page and the tools on its right one.
    case column(width: CGFloat, gap: CGFloat)
    /// The tools under the photo, after `gap` points kept clear. A tabletop (hinge left to right) fixes the tools to
    /// exactly the `below` points of its lower page.
    case stack(gap: CGFloat, below: CGFloat?)

    /// The side column's width on a phone held sideways: two fifths of the room, within these (316 is what it has
    /// always been).
    public static let columnWidths: ClosedRange<CGFloat> = 260...316
    /// The column of a wide window on a device with a hinge, flat (the inspector's usual width), until it is folded.
    public static let flatColumn: CGFloat = 340
    /// The least the photo keeps beside a column.
    public static let minPhoto: CGFloat = 280
    /// The room from which regular width is wide enough for tools beside the photo (a 669 pt Duo held upright is
    /// not: the photo would be 329 pt beside a 340 pt column).
    public static let beside: CGFloat = 720
    /// The least a page of the fold gives the tools (a column's width, a lower page's height) and the photo (an upper
    /// page's height).
    public static let minToolPage: CGFloat = 240
    public static let minLowerPage: CGFloat = 250
    public static let minUpperPage: CGFloat = 200

    /// Whether the tools are in a column beside the photo (else under it, or in the inspector).
    public var isColumn: Bool {
        if case .column = self { return true }
        return false
    }

    /// For automation (`ui.inspect`).
    public var name: String {
        switch self {
        case .inspector: "inspector"
        case .column: "column"
        case .stack: "stack"
        }
    }

    /// - Parameters:
    ///   - room: the area the screen has, whole (not what is left beside an inspector).
    ///   - fold: a hinge that is partly open across it, if any.
    ///   - regular: the horizontal size class is regular.
    ///   - compactHeight: the vertical size class is compact (a phone held sideways, a Duo's outer display on its side).
    ///   - inspector: the system inspector may be used (a device without a hinge): a column of our own keeps the same
    ///     look flat and folded, so folding only moves it.
    public static func choose(room: CGSize, fold: DuoFold?, regular: Bool, compactHeight: Bool, inspector: Bool) -> PhotoLayout {
        if let fold, room.width > 0, room.height > 0 {
            if fold.vertical {
                // a book: the photo on the left page, the tools on the right one
                let page = room.width - fold.frame.maxX
                if page >= minToolPage, fold.frame.minX >= minPhoto {
                    return .column(width: page, gap: fold.frame.width)
                }
            } else {
                // a tabletop: the photo on the upper page, the tools on the lower one
                let below = room.height - fold.frame.maxY
                if below >= minLowerPage, fold.frame.minY >= minUpperPage {
                    return .stack(gap: fold.frame.height, below: below)
                }
            }
        }
        if compactHeight {
            let width = min(columnWidths.upperBound, max(columnWidths.lowerBound, room.width * 0.4))
            return room.width - width >= minPhoto ? .column(width: width, gap: 0) : .stack(gap: 0, below: nil)
        }
        if regular, room.width >= beside {
            return inspector ? .inspector : .column(width: flatColumn, gap: 0)
        }
        return .stack(gap: 0, below: nil)
    }
}
