#if os(iOS)
import LightCraftCore
import SwiftUI
import UIKit

// iPhone Duo (iOS 27.1) and the iOS 27 toolbar APIs, in one file (docs/ios-swift.md → iPhone Duo).
//
// The app's floor is iOS 26 and CI builds with Xcode 26 (Swift 6.2), so every iOS 27 symbol sits behind
// `#if compiler(>=6.4) && !DUO_OFF` (an Xcode 26 toolchain never sees it; build with
// SWIFT_ACTIVE_COMPILATION_CONDITIONS=DUO_OFF to compile that path with a newer one) and `#available` (an iOS 26
// device never runs it). Each wrapper's fallback is what the app did before the Duo, so call sites carry no
// version checks.

// MARK: what the app knows about its window

/// The window the app is in, as `ui.inspect` reports it (`layout`): a tour can check a pose without anyone
/// looking at it. Views measure these (`duoReporting`, the photo screen's frames); none of it drives layout
/// except `fakeFold`, which stands in for a hinge when testing.
@Observable
final class DuoFacts {
    /// The window, full size, in points.
    var window: CGSize = .zero
    var horizontal = ""
    var vertical = ""
    var safeArea = EdgeInsets()
    /// `closed`, `partiallyOpen` or `fullyOpen` once the system has said; `none` on a device without a hinge.
    var hinge = "none"
    var regions: [Region] = []
    /// Frames of the views the layout cares about, in the window's coordinates, by name: `pager`, and the tools under
    /// the name of the arrangement they are in (`inspector`, `column`, `stack`: a view on its way out keeps its own
    /// name, so it cannot overwrite the one coming in).
    var frames: [String: CGRect] = [:]
    /// What the photo screen laid itself out as: `stack`, `column` or `inspector`; empty with no photo open.
    var photoMode = ""
    /// A fold made up for testing (`ui.set {fold: …}`): used wherever a real one would be.
    var fakeFold: FakeFold?
    /// The hinge strip the photo screen (or Compare) is keeping clear, in the window's coordinates like `frames`: nil
    /// while the device is flat.
    var fold: CGRect?

    nonisolated struct Region: Equatable {
        var kind: String
        var active: Bool
        var frame: CGRect
    }

    /// A hinge to pretend to have: where along the window it is (0 to 1) and how wide its keep-out strip is.
    nonisolated struct FakeFold: Equatable {
        var vertical: Bool
        var at: CGFloat
        var width: CGFloat

        func resolve(in size: CGSize) -> DuoFold {
            if vertical {
                return DuoFold(frame: CGRect(x: size.width * at - width / 2, y: 0, width: width, height: size.height), vertical: true)
            }
            return DuoFold(frame: CGRect(x: 0, y: size.height * at - width / 2, width: size.width, height: width), vertical: false)
        }
    }

    func json() -> JSONValue {
        func rect(_ r: CGRect) -> JSONValue {
            .array([.double(r.minX), .double(r.minY), .double(r.width), .double(r.height)])
        }
        var frames: [String: JSONValue] = [:]
        for (name, frame) in self.frames { frames[name] = rect(frame) }
        let region = { (r: Region) -> JSONValue in
            .object(["kind": .string(r.kind), "active": .bool(r.active), "frame": rect(r.frame)])
        }
        let inset = { (v: CGFloat) -> JSONValue in .double(v) }
        return .object([
            "window": .array([.double(window.width), .double(window.height)]),
            "sizeClass": .array([.string(horizontal), .string(vertical)]),
            "safeArea": .object(["top": inset(safeArea.top), "leading": inset(safeArea.leading), "bottom": inset(safeArea.bottom), "trailing": inset(safeArea.trailing)]),
            "hinge": .string(hinge),
            "regions": .array(regions.map(region)),
            "fakeFold": fakeFold.map { .object(["vertical": .bool($0.vertical), "at": .double($0.at), "width": .double($0.width)]) } ?? .null,
            "fold": fold.map(rect) ?? .null,
            "photoMode": .string(photoMode),
            "frames": .object(frames),
        ])
    }
}

extension View {
    /// Reports the window's size, size classes, safe area, reserved regions and hinge to `facts` (once, at the root).
    func duoReporting(_ facts: DuoFacts) -> some View {
        modifier(DuoReporter(facts: facts))
    }

    /// Reports this view's frame (window coordinates) to `facts.frames[name]` while `reporting`, and takes it back when
    /// the view goes (not if another view has reported under the name since). A view that is not on screen, an
    /// inspector's content while it is closed, says nothing: `reporting` is false for it.
    func duoFrame(_ name: String, in facts: DuoFacts, reporting: Bool = true) -> some View {
        modifier(DuoFrame(name: name, facts: facts, reporting: reporting))
    }
}

private struct DuoFrame: ViewModifier {
    let name: String
    let facts: DuoFacts
    let reporting: Bool
    /// What this view last reported.
    @State private var mine: CGRect?

    func body(content: Content) -> some View {
        content
            .onGeometryChange(for: CGRect.self) { reporting ? $0.frame(in: .global) : .null } action: { frame in
                guard !frame.isNull else { return }
                mine = frame
                if facts.frames[name] != frame { facts.frames[name] = frame }
            }
            .onDisappear {
                if let mine, facts.frames[name] == mine { facts.frames[name] = nil }
            }
    }
}

private struct DuoReporter: ViewModifier {
    let facts: DuoFacts
    @Environment(\.horizontalSizeClass) private var horizontal
    @Environment(\.verticalSizeClass) private var vertical

    /// Everything measured, so one `onChange` can tell when anything moved.
    private struct Measured: Equatable {
        var window: CGSize
        var safeArea: EdgeInsets
        var regions: [DuoFacts.Region]
    }

    func body(content: Content) -> some View {
        content
            .overlay {
                GeometryReader { proxy in
                    Color.clear
                        .onChange(of: Measured(window: proxy.size, safeArea: proxy.safeAreaInsets, regions: Self.regions(proxy)), initial: true) { _, new in
                            facts.window = new.window
                            facts.safeArea = new.safeArea
                            facts.regions = new.regions
                        }
                }
                .ignoresSafeArea()
                .allowsHitTesting(false)
            }
            .onChange(of: horizontal, initial: true) { _, new in facts.horizontal = Self.name(new) }
            .onChange(of: vertical, initial: true) { _, new in facts.vertical = Self.name(new) }
            .duoHinge(facts)
    }

    private static func name(_ c: UserInterfaceSizeClass?) -> String {
        switch c {
        case .compact: "compact"
        case .regular: "regular"
        default: "unknown"
        }
    }

    /// The reserved regions that touch the window (the fold and the camera), active or not.
    private static func regions(_ proxy: GeometryProxy) -> [DuoFacts.Region] {
        #if compiler(>=6.4) && !DUO_OFF
        if #available(iOS 27.1, *) {
            func list(_ kind: ReservedRegion.Kind, _ name: String) -> [DuoFacts.Region] {
                proxy.reservedRegions(kind: kind, options: .includeInactive).map { DuoFacts.Region(kind: name, active: $0.isActive, frame: $0.frame) }
            }
            return list(.division, "division") + list(.occlusion, "occlusion")
        }
        #endif
        return []
    }
}

extension View {
    @ViewBuilder fileprivate func duoHinge(_ facts: DuoFacts) -> some View {
        #if compiler(>=6.4) && !DUO_OFF
        if #available(iOS 27.1, *) {
            onHingeChange { _, new in
                facts.hinge = new.hinge.map { hinge in
                    let status = hinge.status == .closed ? "closed" : hinge.status == .partiallyOpen ? "partiallyOpen" : "fullyOpen"
                    return "\(status) \(Int(hinge.angle.degrees))°"
                } ?? "none"
            }
        } else {
            self
        }
        #else
        self
        #endif
    }
}

// MARK: the fold (`DuoFold` and what the photo screen makes of it: LightCraftCore/PhotoLayout.swift)

extension View {
    /// Reports the fold across this view's area into `fold`: nil while the device is flat, shut, or not a Duo.
    /// The `DuoFacts.fakeFold` of a test takes the place of the hinge.
    func onFold(_ fold: Binding<DuoFold?>) -> some View {
        modifier(FoldReader(fold: fold))
    }
}

private struct FoldReader: ViewModifier {
    @Binding var fold: DuoFold?
    private let facts = AppModel.shared.duo

    func body(content: Content) -> some View {
        content.background {
            GeometryReader { proxy in
                Color.clear.onChange(of: measure(proxy), initial: true) { _, new in
                    if fold != new { fold = new }
                    let origin = proxy.frame(in: .global).origin
                    facts.fold = new.map { $0.frame.offsetBy(dx: origin.x, dy: origin.y) }
                }
            }
        }
    }

    private func measure(_ proxy: GeometryProxy) -> DuoFold? {
        if let fake = facts.fakeFold { return fake.resolve(in: proxy.size) }
        #if compiler(>=6.4) && !DUO_OFF
        if #available(iOS 27.1, *) {
            guard let region = proxy.reservedRegions(kind: .division).first(where: \.isActive) else { return nil }
            // (a division strip that runs most of the way across is a hinge; a short one is something else)
            let vertical = region.frame.height >= proxy.size.height * 0.9
            guard vertical || region.frame.width >= proxy.size.width * 0.9 else { return nil }
            return DuoFold(frame: region.frame, vertical: vertical)
        }
        #endif
        return nil
    }
}

// MARK: toolbars

/// How long a toolbar item stays when a bar has run out of room. The system moves items that do not fit into its
/// overflow menu, last item first, unless they say they matter more.
nonisolated enum DuoPriority {
    case low, automatic, high
}

extension ToolbarContent {
    /// `priority` for this item (iOS 27.0 and later; before, a bar has no such choice).
    @ToolbarContentBuilder
    func duoPriority(_ priority: DuoPriority) -> some ToolbarContent {
        #if compiler(>=6.4) && !DUO_OFF
        if #available(iOS 27.0, *) {
            visibilityPriority(priority == .high ? .high : priority == .low ? .low : .automatic)
        } else {
            self
        }
        #else
        self
        #endif
    }
}

/// An "…" menu of actions that do not fit on the bar. On iOS 27 it is the system's overflow menu, so the actions
/// that overflowed and these are in one place (a bar with its own "…" next to the system's shows two); before, a
/// menu of our own at the trailing end.
struct DuoOverflow<Items: View>: ToolbarContent {
    private let placement: ToolbarItemPlacement
    private let items: Items

    /// `placement`: where the menu of our own goes before iOS 27 (the system's overflow menu has no say).
    init(placement: ToolbarItemPlacement = .topBarTrailing, @ViewBuilder items: () -> Items) {
        self.placement = placement
        self.items = items()
    }

    var body: some ToolbarContent {
        #if compiler(>=6.4) && !DUO_OFF
        if #available(iOS 27.0, *) {
            ToolbarOverflowMenu { items }
        } else {
            ToolbarItem(placement: placement) { Menu { items } label: { Label("More", systemImage: "ellipsis") } }
        }
        #else
        ToolbarItem(placement: placement) { Menu { items } label: { Label("More", systemImage: "ellipsis") } }
        #endif
    }
}
#endif
