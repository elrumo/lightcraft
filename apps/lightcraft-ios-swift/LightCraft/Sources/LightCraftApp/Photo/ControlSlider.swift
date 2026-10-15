import LightCraftCore
import SwiftUI

#if os(iOS)
import UIKit
#endif

/// A develop slider as the phone has it: the label and the value over a track filled from the
/// default to the value (or a colour track for white balance, hue, saturation…), with a capsule
/// thumb. The whole row is the grab zone and the value follows the finger's movement (not where it
/// is), so a small slider can still be set finely; a vertical start scrolls the panel instead. A
/// double tap puts it back to its default; crossing the default and reaching an end tick.
struct ControlSlider: View {
    let spec: ControlSpec
    let value: Double
    var label: String? = nil
    let set: (Double) -> Void
    let editing: (Bool) -> Void
    let reset: () -> Void
    @State private var dragStart: Double?
    @State private var detents = 0
    @State private var trackWidth: CGFloat = 300

    var body: some View {
        #if os(macOS)
        // a Mac's edit panel is Lightroom's: the name, the track and the value on one row
        MacSliderRow(spec: spec, value: value, label: label, set: set, editing: editing, reset: reset)
        #else
        phoneSlider
        #endif
    }

    #if os(iOS)
    /// The phone's slider: the name and the value over the track.
    private var phoneSlider: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(LocalizedStringKey(label ?? spec.label))
                Spacer()
                Text(spec.format(value))
                    .monospacedDigit()
                    .foregroundStyle(dragStart != nil ? Color.accentColor : .secondary)
            }
            .font(.subheadline)
            GeometryReader { geo in
                track(width: geo.size.width)
            }
            .frame(height: 22)
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { trackWidth = $0 }
        }
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .gesture(drag)
        .onTapGesture(count: 2) {
            reset()
            detents += 1
        }
        .haptic(.selection, trigger: detents)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(LocalizedStringKey(label ?? spec.label)))
        .accessibilityValue(spec.format(value))
        .accessibilityAdjustableAction { direction in
            set(spec.clamp(direction == .increment ? value + spec.nudge : value - spec.nudge))
        }
        .accessibilityAction(named: "Reset") { reset() }
    }
    #endif

    private func fraction(_ v: Double) -> Double { (v - spec.min) / spec.span }

    private func track(width: CGFloat) -> some View {
        let thumbW: CGFloat = 38
        let usable = max(width - thumbW, 1)
        let x = CGFloat(fraction(value)) * usable
        let d = CGFloat(fraction(spec.default)) * usable
        return ZStack(alignment: .leading) {
            trackFill
                .frame(height: 6)
                .clipShape(.capsule)
                .padding(.horizontal, thumbW / 2)
            if spec.track.kind == "plain" || spec.track.kind == "centered" {
                Capsule()
                    .fill(Color.accentColor)
                    .frame(width: abs(x - d), height: 6)
                    .offset(x: min(x, d) + thumbW / 2)
            }
            Capsule()
                .fill(.white)
                .shadow(color: .black.opacity(0.25), radius: 3, y: 1)
                .frame(width: thumbW, height: 22)
                .scaleEffect(dragStart != nil ? 1.12 : 1)
                .offset(x: x)
                .animation(.snappy(duration: 0.15), value: dragStart != nil)
        }
    }

    /// The track: grey, or the control's colours.
    @ViewBuilder private var trackFill: some View {
        if let colors = Self.trackColors(spec) {
            LinearGradient(colors: colors, startPoint: .leading, endPoint: .trailing)
        } else {
            Color.lcFill
        }
    }

    /// A colour track's colours, low to high (`nil`: a plain grey track).
    static func trackColors(_ spec: ControlSpec) -> [Color]? {
        switch spec.track.kind {
        case "temp": [Color(red: 0.29, green: 0.52, blue: 0.95), Color(red: 0.95, green: 0.84, blue: 0.29)]
        case "tint": [Color(red: 0.27, green: 0.78, blue: 0.33), Color(red: 0.86, green: 0.31, blue: 0.86)]
        case "gradient": [Color(hex: spec.track.from) ?? .gray, Color(hex: spec.track.to) ?? .accentColor]
        case "hue", "rainbow": hueStops(band: spec.track.band)
        case "sat": [.gray, bandColor(spec.track.band)]
        case "lum": [.black, bandColor(spec.track.band), .white]
        default: nil
        }
    }

    /// The colour mixer's eight bands.
    static let bands: [Color] = [
        Color(red: 0.93, green: 0.25, blue: 0.22), Color(red: 0.96, green: 0.56, blue: 0.18), Color(red: 0.96, green: 0.84, blue: 0.20),
        Color(red: 0.30, green: 0.75, blue: 0.30), Color(red: 0.25, green: 0.80, blue: 0.82), Color(red: 0.22, green: 0.45, blue: 0.93),
        Color(red: 0.55, green: 0.30, blue: 0.85), Color(red: 0.88, green: 0.30, blue: 0.70),
    ]

    static func bandColor(_ band: Int?) -> Color {
        guard let band, band >= 0, band < bands.count else { return .accentColor }
        return bands[band]
    }

    /// A hue track: the whole wheel, or around one band.
    static func hueStops(band: Int?) -> [Color] {
        guard let band, band >= 0, band < bands.count else {
            return stride(from: 0.0, through: 1.0, by: 1.0 / 6).map { Color(hue: $0, saturation: 0.8, brightness: 0.9) }
        }
        let prev = bands[(band + bands.count - 1) % bands.count]
        let next = bands[(band + 1) % bands.count]
        return [prev, bands[band], next]
    }

    #if os(iOS)
    /// A horizontal pan only: a pan that starts vertical fails at once, so the panel's scroll view
    /// (which a SwiftUI `DragGesture` would starve) gets it.
    private var drag: some UIGestureRecognizerRepresentable {
        HorizontalPan { phase, dx in
            switch phase {
            case .began:
                guard spec.min < spec.max else { return }
                dragStart = value
                editing(true)
            case .changed:
                guard let start = dragStart else { return }
                // the value follows the finger's movement across the track's width
                let v = spec.clamp(start + Double(dx) / Double(max(trackWidth - 38, 1)) * spec.span)
                if spec.detent(from: value, to: v) { detents += 1 }
                if v != value { set(v) }
            case .ended:
                if dragStart != nil { editing(false) }
                dragStart = nil
            }
        }
    }
    #endif
}

#if os(iOS)
private struct HorizontalPan: UIGestureRecognizerRepresentable {
    enum Phase { case began, changed, ended }
    let onPhase: (Phase, CGFloat) -> Void

    func makeUIGestureRecognizer(context: Context) -> HorizontalPanRecognizer { HorizontalPanRecognizer() }

    func handleUIGestureRecognizerAction(_ r: HorizontalPanRecognizer, context: Context) {
        let dx = r.translation(in: r.view).x
        switch r.state {
        case .began: onPhase(.began, dx)
        case .changed: onPhase(.changed, dx)
        case .ended, .cancelled, .failed: onPhase(.ended, dx)
        default: break
        }
    }
}

final class HorizontalPanRecognizer: UIPanGestureRecognizer {
    private var origin = CGPoint.zero

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
        origin = touches.first?.location(in: view) ?? .zero
        super.touchesBegan(touches, with: event)
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) {
        if state == .possible, let p = touches.first?.location(in: view) {
            let dx = abs(p.x - origin.x), dy = abs(p.y - origin.y)
            if dy > 8, dy > dx {
                state = .failed
                return
            }
        }
        super.touchesMoved(touches, with: event)
    }
}
#endif

extension ControlSpec {
    /// Its range (never zero).
    var span: Double { Swift.max(self.max - self.min, .ulpOfOne) }

    /// `v` within the range, on a step.
    func clamp(_ v: Double) -> Double {
        var x = Swift.min(Swift.max(v, self.min), self.max)
        if step > 0 { x = (x / step).rounded() * step }
        return x
    }

    /// Moving from `old` to `new` crosses the default or arrives at an end (felt as a tick).
    func detent(from old: Double, to new: Double) -> Bool {
        (old - self.default) * (new - self.default) < 0 || ((new == self.min || new == self.max) && new != old)
    }

    /// One VoiceOver swipe's change.
    var nudge: Double { (step > 0 ? step : span / 100) * 5 }
}

extension Color {
    /// `#rrggbb`.
    init?(hex: String?) {
        guard var s = hex?.trimmingCharacters(in: .whitespaces), s.hasPrefix("#") else { return nil }
        s.removeFirst()
        guard s.count == 6, let n = UInt32(s, radix: 16) else { return nil }
        self.init(red: Double((n >> 16) & 0xFF) / 255, green: Double((n >> 8) & 0xFF) / 255, blue: Double(n & 0xFF) / 255)
    }
}
