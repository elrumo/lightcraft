#if os(iOS)
import SwiftUI

extension View {
    /// `sensoryFeedback`, unless Settings ▸ General ▸ Haptics is off.
    func haptic<T: Equatable>(_ feedback: SensoryFeedback, trigger: T) -> some View {
        modifier(HapticFeedback(trigger: trigger) { _, _ in feedback })
    }

    /// `sensoryFeedback` with a choice of feedback (`nil`: none), unless Haptics is off.
    func haptic<T: Equatable>(trigger: T, _ feedback: @escaping (T, T) -> SensoryFeedback?) -> some View {
        modifier(HapticFeedback(trigger: trigger, feedback: feedback))
    }
}

/// Reads the switch where `Prefs` keeps it (the standard defaults, `haptics`, on unless turned
/// off), so it needs nothing from the environment.
private struct HapticFeedback<T: Equatable>: ViewModifier {
    @AppStorage("haptics") private var on = true
    let trigger: T
    let feedback: (T, T) -> SensoryFeedback?

    func body(content: Content) -> some View {
        let on = on
        return content.sensoryFeedback(trigger: trigger) { old, new in on ? feedback(old, new) : nil }
    }
}
#endif
