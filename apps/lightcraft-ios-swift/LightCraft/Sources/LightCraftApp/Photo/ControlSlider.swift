#if os(iOS)
import LightCraftCore
import SwiftUI

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
    @State private var horizontal: Bool?
    @State private var detents = 0
    @State private var trackWidth: CGFloat = 300

    var body: some View {
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
        .gesture(drag, including: spec.min < spec.max ? .all : .none)
        .onTapGesture(count: 2) {
            reset()
            detents += 1
        }
        .haptic(.selection, trigger: detents)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(LocalizedStringKey(label ?? spec.label)))
        .accessibilityValue(spec.format(value))
        .accessibilityAdjustableAction { direction in
            let step = spec.step > 0 ? spec.step : (spec.max - spec.min) / 100
            let v = direction == .increment ? value + step * 5 : value - step * 5
            set(clamp(v))
        }
        .accessibilityAction(named: "Reset") { reset() }
    }

    private var span: Double { max(spec.max - spec.min, .ulpOfOne) }

    private func fraction(_ v: Double) -> Double { (v - spec.min) / span }

    private func clamp(_ v: Double) -> Double {
        var x = min(max(v, spec.min), spec.max)
        if spec.step > 0 { x = (x / spec.step).rounded() * spec.step }
        return x
    }

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
        switch spec.track.kind {
        case "temp":
            LinearGradient(colors: [Color(red: 0.29, green: 0.52, blue: 0.95), Color(red: 0.95, green: 0.84, blue: 0.29)], startPoint: .leading, endPoint: .trailing)
        case "tint":
            LinearGradient(colors: [Color(red: 0.27, green: 0.78, blue: 0.33), Color(red: 0.86, green: 0.31, blue: 0.86)], startPoint: .leading, endPoint: .trailing)
        case "gradient":
            LinearGradient(colors: [Color(hex: spec.track.from) ?? .gray, Color(hex: spec.track.to) ?? .accentColor], startPoint: .leading, endPoint: .trailing)
        case "hue", "rainbow":
            LinearGradient(colors: Self.hueStops(band: spec.track.band), startPoint: .leading, endPoint: .trailing)
        case "sat":
            LinearGradient(colors: [.gray, Self.bandColor(spec.track.band)], startPoint: .leading, endPoint: .trailing)
        case "lum":
            LinearGradient(colors: [.black, Self.bandColor(spec.track.band), .white], startPoint: .leading, endPoint: .trailing)
        default:
            Color(uiColor: .systemFill)
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

    private var drag: some Gesture {
        DragGesture(minimumDistance: 8, coordinateSpace: .local)
            .onChanged { g in
                if horizontal == nil {
                    horizontal = abs(g.translation.width) >= abs(g.translation.height)
                    if horizontal == true {
                        dragStart = value
                        editing(true)
                    }
                }
                guard horizontal == true, let start = dragStart else { return }
                // the value follows the finger's movement across the track's width
                let v = clamp(start + Double(g.translation.width) / Double(max(trackWidth - 38, 1)) * span)
                if (value - spec.default) * (v - spec.default) < 0 || ((v == spec.min || v == spec.max) && v != value) {
                    detents += 1
                }
                if v != value { set(v) }
            }
            .onEnded { _ in
                if horizontal == true { editing(false) }
                horizontal = nil
                dragStart = nil
            }
    }
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
#endif
