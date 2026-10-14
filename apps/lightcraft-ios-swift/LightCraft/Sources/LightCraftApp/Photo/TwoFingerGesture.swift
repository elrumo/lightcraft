#if os(iOS)
import SwiftUI
import UIKit

/// Two fingers on the photo move it and pinch it while a tool owns the one-finger drag (a brush
/// paints with it). It reads the touches of the whole screen's view without taking them from the
/// tools: the recognizers sit on the screen's hosting view and see every touch that lands on this
/// view's frame, alongside SwiftUI's own gestures.
///
/// - `fingers`: how many fingers are on the photo (0 when all are up), so a tool can drop a stroke
///   the moment a second finger lands.
/// - `pan`: the fingers' centre moved by this much (points).
/// - `pinch`: the fingers' spread changed by this factor around this point (this view's space).
/// - `ended`: the fingers let go (called once for the pan and once for the pinch).
struct TwoFingerGesture: UIViewRepresentable {
    /// The fingers move and zoom the photo (else they are only counted).
    let moves: Bool
    let fingers: (Int) -> Void
    let pan: (CGSize) -> Void
    let pinch: (CGFloat, CGPoint) -> Void
    let ended: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> Anchor {
        let view = Anchor()
        // (a background must not take the photo's touches)
        view.isUserInteractionEnabled = false
        context.coordinator.anchor = view
        view.onWindow = { [weak coordinator = context.coordinator] in coordinator?.attach() }
        return view
    }

    func updateUIView(_ view: Anchor, context: Context) {
        context.coordinator.parent = self
        context.coordinator.enable(moves: moves)
    }

    static func dismantleUIView(_ view: Anchor, coordinator: Coordinator) {
        coordinator.detach()
    }

    final class Anchor: UIView {
        var onWindow: (() -> Void)?

        override func didMoveToWindow() {
            super.didMoveToWindow()
            onWindow?()
        }
    }

    /// Counts the fingers down on the photo without ever recognizing (so it blocks nothing).
    final class FingerCounter: UIGestureRecognizer {
        var changed: ((Int) -> Void)?
        private var down = Set<UITouch>()

        override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
            down.formUnion(touches)
            changed?(down.count)
        }

        override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) { lifted(touches) }
        override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) { lifted(touches) }

        private func lifted(_ touches: Set<UITouch>) {
            down.subtract(touches)
            changed?(down.count)
            if down.isEmpty { state = .failed }
        }

        override func reset() { down = [] }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        var parent: TwoFingerGesture?
        weak var anchor: Anchor?
        private weak var host: UIView?
        private lazy var counter: FingerCounter = {
            let g = FingerCounter(target: nil, action: nil)
            g.changed = { [weak self] n in self?.parent?.fingers(n) }
            return g
        }()
        private lazy var panner: UIPanGestureRecognizer = {
            let g = UIPanGestureRecognizer(target: self, action: #selector(panned))
            g.minimumNumberOfTouches = 2
            g.maximumNumberOfTouches = 2
            return g
        }()
        private lazy var pincher = UIPinchGestureRecognizer(target: self, action: #selector(pinched))

        private var recognizers: [UIGestureRecognizer] { [counter, panner, pincher] }

        /// Put the recognizers on the nearest SwiftUI hosting view above the anchor (the screen's).
        func attach() {
            guard let anchor, anchor.window != nil else { return }
            var found: UIView?
            var v = anchor.superview
            while let u = v {
                if String(describing: type(of: u)).contains("HostingView") {
                    found = u
                    break
                }
                v = u.superview
            }
            guard let target = found ?? anchor.superview, target !== host else { return }
            detach()
            host = target
            for g in recognizers {
                g.delegate = self
                g.cancelsTouchesInView = false
                g.delaysTouchesBegan = false
                g.delaysTouchesEnded = false
                target.addGestureRecognizer(g)
            }
            enable(moves: moves)
        }

        func detach() {
            for g in recognizers { g.view?.removeGestureRecognizer(g) }
            host = nil
        }

        private var moves = false

        func enable(moves value: Bool) {
            moves = value
            panner.isEnabled = value
            pincher.isEnabled = value
        }

        @objc private func panned(_ g: UIPanGestureRecognizer) {
            guard let anchor else { return }
            switch g.state {
            case .changed:
                let t = g.translation(in: anchor)
                g.setTranslation(.zero, in: anchor)
                parent?.pan(CGSize(width: t.x, height: t.y))
            case .ended, .cancelled, .failed:
                parent?.ended()
            default:
                break
            }
        }

        @objc private func pinched(_ g: UIPinchGestureRecognizer) {
            guard let anchor else { return }
            switch g.state {
            case .changed:
                let s = g.scale
                g.scale = 1
                parent?.pinch(s, g.location(in: anchor))
            case .ended, .cancelled, .failed:
                parent?.ended()
            default:
                break
            }
        }

        func gestureRecognizer(_ g: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
            guard let anchor, anchor.window != nil else { return false }
            return anchor.bounds.contains(touch.location(in: anchor))
        }

        func gestureRecognizer(_ g: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
            true
        }
    }
}
#endif
