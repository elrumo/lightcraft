#if os(iOS)
import SwiftUI
import UIKit

/// While `disabled`, turns off the swipe down and the pinch that close a photo screen pushed with a
/// zoom transition (UIKit installs them on the screen's view and SwiftUI has no switch for them): a
/// tool working with touches on the photo mustn't lose the screen to a stray swipe. They are found by
/// the name of their delegate, so if UIKit renames them the screen simply closes as before. The edge
/// swipe (back) stays.
struct DismissGuard: UIViewControllerRepresentable {
    let disabled: Bool

    func makeUIViewController(context: Context) -> Guard { Guard() }

    func updateUIViewController(_ vc: Guard, context: Context) {
        vc.disabled = disabled
        vc.apply()
    }

    final class Guard: UIViewController {
        var disabled = false

        override func viewDidLoad() {
            super.viewDidLoad()
            // (a background must not take the photo's touches)
            view.isUserInteractionEnabled = false
        }

        override func viewDidAppear(_ animated: Bool) {
            super.viewDidAppear(animated)
            apply()
        }

        override func viewDidLayoutSubviews() {
            super.viewDidLayoutSubviews()
            apply()
        }

        func apply() {
            var ancestor: UIView? = view
            while let v = ancestor {
                for g in v.gestureRecognizers ?? [] where Self.closes(g) { g.isEnabled = !disabled }
                ancestor = v.superview
            }
        }

        private static func closes(_ g: UIGestureRecognizer) -> Bool {
            guard let delegate = g.delegate else { return false }
            let name = String(describing: type(of: delegate))
            return name.contains("DismissSubInteraction") && !name.contains("EdgePan")
        }
    }
}
#endif
