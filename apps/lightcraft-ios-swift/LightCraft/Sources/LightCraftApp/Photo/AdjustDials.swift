#if os(iOS)
import LightCraftCore
import SwiftUI

/// One dial of the Dials style (Photos' way of adjusting): a control (its value on a ring, set with
/// the ruler), a switch, a choice (a menu), an action, or a page of more controls.
struct Dial: Identifiable {
    enum Kind {
        case control(ControlSpec)
        case toggle(Bool, (Bool) -> Void)
        case choice([(String, LocalizedStringKey)], String, (String) -> Void)
        case action(() -> Void)
        case page(DialPage)
    }

    let id: String
    let label: String
    let symbol: String
    let kind: Kind

    static func control(_ spec: ControlSpec) -> Dial {
        Dial(id: spec.id, label: spec.label, symbol: ControlSpec.symbol(spec.id), kind: .control(spec))
    }

    var spec: ControlSpec? {
        if case .control(let spec) = kind { spec } else { nil }
    }
}

/// The pages a dial opens: controls that need more room than a ruler.
nonisolated enum DialPage: String, Identifiable {
    case curve, mixer, pointColor, grading, sdr
    var id: String { rawValue }
}

/// Controls as Photos sets them: a row of dials (the one in the middle is the one being set), its
/// name and value above, and a ruler under them that slides under a fixed mark. Each ruler drag is
/// one interaction (one undo step) through the same `editing` and `set` a slider uses.
struct DialAdjuster: View {
    let dials: [Dial]
    let value: (ControlSpec) -> Double
    let set: (ControlSpec, Double) -> Void
    let editing: (ControlSpec, Bool) -> Void
    let reset: (ControlSpec) -> Void
    var open: (DialPage) -> Void = { _ in }
    @State private var selected: String?
    /// A dial was chosen (by a tap or by scrolling): no more choosing the first control for it.
    @State private var chosen = false
    @State private var width: CGFloat = 360
    @ScaledMetric(relativeTo: .body) private var side: CGFloat = 50

    var body: some View {
        let current = dials.first { $0.id == selected } ?? dials.first
        VStack(spacing: 6) {
            caption(current)
            ScrollView(.horizontal) {
                LazyHStack(spacing: 14) {
                    ForEach(dials) { dial in
                        DialFace(dial: dial, value: dial.spec.map(value), selected: dial.id == current?.id, side: side) {
                            tap(dial)
                        }
                        .id(dial.id)
                    }
                }
                .scrollTargetLayout()
            }
            .scrollTargetBehavior(.viewAligned)
            .scrollPosition(id: $selected, anchor: .center)
            .contentMargins(.horizontal, Swift.max((width - side) / 2, 0), for: .scrollContent)
            .scrollIndicators(.hidden)
            .frame(height: side + 8)
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
            if let spec = current?.spec {
                RulerSlider(spec: spec, value: value(spec), set: { set(spec, $0) }, editing: { editing(spec, $0) }, reset: { reset(spec) })
            } else {
                // (the same height: the dials mustn't jump when a switch is in the middle)
                Color.clear.frame(height: RulerSlider.height)
            }
        }
        .padding(.vertical, 8)
        .haptic(.selection, trigger: selected)
        .onScrollPhaseChange { _, phase in
            if phase == .interacting { chosen = true }
        }
        // the first control in the middle to begin with, once its spec has come (a group's
        // switches and choices can come before its sliders)
        .task(id: dials.first { $0.spec != nil }?.id) {
            guard !chosen, let first = (dials.first { $0.spec != nil } ?? dials.first)?.id else { return }
            selected = first
        }
    }

    /// The dial in the middle: its name, and its value or state.
    private func caption(_ dial: Dial?) -> some View {
        HStack(spacing: 6) {
            if let dial {
                Text(LocalizedStringKey(dial.label)).textCase(.uppercase)
                switch dial.kind {
                case .control(let spec):
                    Text(spec.format(value(spec))).monospacedDigit().foregroundStyle(.secondary)
                        .contentTransition(.numericText())
                case .toggle(let on, _):
                    Text(on ? "On" : "Off").foregroundStyle(.secondary)
                case .choice(let options, let current, _):
                    Text(options.first { $0.0 == current }?.1 ?? "Custom").foregroundStyle(.secondary)
                case .action, .page:
                    EmptyView()
                }
            }
        }
        .font(.footnote.weight(.semibold))
        .lineLimit(1)
        .frame(height: 18)
    }

    /// A tap brings a dial to the middle; a switch also switches, an action acts, a page opens.
    private func tap(_ dial: Dial) {
        chosen = true
        withAnimation(.snappy) { selected = dial.id }
        switch dial.kind {
        case .toggle(let on, let change): change(!on)
        case .action(let act): act()
        case .page(let page): open(page)
        case .control, .choice: break
        }
    }
}

/// A dial: a ring showing how far its control is from the default (in the track's colours for
/// temperature, tint and hue), its symbol, or its value while it is the one being set.
private struct DialFace: View {
    let dial: Dial
    let value: Double?
    let selected: Bool
    let side: CGFloat
    let tap: () -> Void

    var body: some View {
        Group {
            if case .choice(let options, let current, let choose) = dial.kind {
                Menu {
                    Picker(LocalizedStringKey(dial.label), selection: Binding(get: { current }, set: { choose($0) })) {
                        ForEach(options, id: \.0) { Text($0.1).tag($0.0) }
                    }
                    .pickerStyle(.inline)
                } label: {
                    face
                }
            } else {
                Button(action: tap) { face }
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(LocalizedStringKey(dial.label)))
        .accessibilityValue(accessibilityValue)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var face: some View {
        ZStack {
            ring
            content
        }
        .frame(width: side, height: side)
        .contentShape(Circle())
        .scaleEffect(selected ? 1 : 0.86)
        .animation(.snappy(duration: 0.2), value: selected)
    }

    @ViewBuilder private var ring: some View {
        switch dial.kind {
        case .control(let spec):
            let base = Circle().inset(by: 1.5)
            if let colors = ControlSlider.trackColors(spec) {
                base.stroke(AngularGradient(colors: colors + [colors[0]], center: .center, startAngle: .degrees(-90), endAngle: .degrees(270)), lineWidth: 3)
                    .opacity(0.55)
            } else {
                base.stroke(.tertiary, lineWidth: 3)
            }
            if let value {
                let f = Self.arc(spec, value)
                base.trim(from: 0, to: abs(f))
                    .stroke(selected ? Color.primary : Color.accentColor, style: StrokeStyle(lineWidth: 3, lineCap: .round))
                    .rotationEffect(.degrees(-90))
                    .scaleEffect(x: f < 0 ? -1 : 1)
            }
        case .toggle(let on, _):
            Circle().inset(by: 1.5).fill(on ? Color.accentColor : Color.clear)
            Circle().inset(by: 1.5).stroke(.tertiary, lineWidth: 3).opacity(on ? 0 : 1)
        default:
            Circle().inset(by: 1.5).stroke(.tertiary, lineWidth: 3)
        }
    }

    @ViewBuilder private var content: some View {
        if selected, case .control(let spec) = dial.kind, let value, value != spec.default {
            Text(spec.format(value))
                .font(.caption.weight(.semibold))
                .monospacedDigit()
                .minimumScaleFactor(0.5)
                .lineLimit(1)
                .padding(6)
        } else {
            Image(systemName: dial.symbol)
                .font(.title3)
                .foregroundStyle(isOn ? Color.white : (selected ? Color.primary : Color.secondary))
        }
    }

    private var isOn: Bool {
        if case .toggle(let on, _) = dial.kind { on } else { false }
    }

    private var accessibilityValue: Text {
        switch dial.kind {
        case .control(let spec): Text(value.map(spec.format) ?? "")
        case .toggle(let on, _): Text(on ? "On" : "Off")
        default: Text(verbatim: "")
        }
    }

    /// How far round the ring the value is: 0 at the default, 1 at the top of the range, -1 at the
    /// bottom (drawn the other way round).
    static func arc(_ spec: ControlSpec, _ v: Double) -> Double {
        let d = spec.default
        if v >= d {
            let r = spec.max - d
            return r > 0 ? Swift.min((v - d) / r, 1) : 0
        }
        let r = d - spec.min
        return r > 0 ? -Swift.min((d - v) / r, 1) : 0
    }
}

/// Photos' ruler: ticks that slide under a fixed mark in the middle as the finger moves (left for
/// more); the whole range is three widths long. A double tap puts the control back to its default;
/// crossing the default and reaching an end tick, as a slider does.
struct RulerSlider: View {
    static let height: CGFloat = 40
    let spec: ControlSpec
    let value: Double
    let set: (Double) -> Void
    let editing: (Bool) -> Void
    let reset: () -> Void
    @State private var dragStart: Double?
    @State private var horizontal: Bool?
    @State private var detents = 0
    @State private var width: CGFloat = 320

    var body: some View {
        Canvas { ctx, size in
            let mid = size.width / 2
            let perUnit = Double(size.width) * 3 / spec.span
            // sixty ticks over the whole range, every fifth (and the default) taller
            let unit = spec.span / 60
            for k in 0...60 {
                let t = spec.min + Double(k) * unit
                let x = mid + CGFloat((t - value) * perUnit)
                guard x >= -1, x <= size.width + 1 else { continue }
                let major = k % 5 == 0 || abs(t - spec.default) < unit / 2
                let h: CGFloat = major ? 16 : 9
                var tick = Path()
                tick.move(to: CGPoint(x: x, y: (size.height - h) / 2))
                tick.addLine(to: CGPoint(x: x, y: (size.height + h) / 2))
                ctx.stroke(tick, with: .color(.secondary), lineWidth: major ? 1.5 : 1)
            }
            var mark = Path()
            mark.move(to: CGPoint(x: mid, y: 4))
            mark.addLine(to: CGPoint(x: mid, y: size.height - 4))
            ctx.stroke(mark, with: .color(.accentColor), style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
        }
        .frame(height: Self.height)
        .mask(LinearGradient(stops: [.init(color: .clear, location: 0), .init(color: .black, location: 0.15), .init(color: .black, location: 0.85), .init(color: .clear, location: 1)], startPoint: .leading, endPoint: .trailing))
        .contentShape(Rectangle())
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
        .gesture(drag, including: spec.min < spec.max ? .all : .none)
        .onTapGesture(count: 2) {
            reset()
            detents += 1
        }
        .haptic(.selection, trigger: detents)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(LocalizedStringKey(spec.label)))
        .accessibilityValue(spec.format(value))
        .accessibilityAdjustableAction { direction in
            set(spec.clamp(direction == .increment ? value + spec.nudge : value - spec.nudge))
        }
        .accessibilityAction(named: "Reset") { reset() }
    }

    private var drag: some Gesture {
        DragGesture(minimumDistance: 6, coordinateSpace: .local)
            .onChanged { g in
                if horizontal == nil {
                    horizontal = abs(g.translation.width) >= abs(g.translation.height)
                    if horizontal == true {
                        dragStart = value
                        editing(true)
                    }
                }
                guard horizontal == true, let start = dragStart else { return }
                // the ruler moves with the finger: dragged left, the mark reads more
                let v = spec.clamp(start - Double(g.translation.width) / (Double(Swift.max(width, 1)) * 3) * spec.span)
                if spec.detent(from: value, to: v) { detents += 1 }
                if v != value { set(v) }
            }
            .onEnded { _ in
                if horizontal == true { editing(false) }
                horizontal = nil
                dragStart = nil
            }
    }
}

extension ControlSpec {
    /// A dial's symbol for a control (`light.exposure`, a mask's `exposure`…), by the last part of
    /// its id; others take a plain dial.
    static func symbol(_ id: String) -> String {
        let key = id.split(separator: ".").last.map(String.init) ?? id
        return symbols[id] ?? symbols[key] ?? "dial.medium"
    }

    private static let symbols: [String: String] = [
        "exposure": "plusminus.circle", "contrast": "circle.lefthalf.filled", "highlights": "sun.max",
        "shadows": "shadow", "whites": "circle", "blacks": "circle.fill", "maxEv": "sun.max.trianglebadge.exclamationmark",
        "temp": "thermometer.medium", "tempRel": "thermometer.medium", "tint": "drop.halffull", "tintRel": "drop.halffull",
        "vibrance": "camera.filters", "saturation": "drop", "hue": "paintpalette",
        "texture": "square.grid.3x3", "clarity": "triangle.lefthalf.filled", "dehaze": "aqi.medium",
        "amount": "dial.medium", "midpoint": "circle.and.line.horizontal", "roundness": "circle.square",
        "feather": "aqi.low", "size": "circle.grid.2x2", "roughness": "scribble",
        "sharpenAmount": "triangle", "sharpenRadius": "circle.dotted", "sharpenDetail": "magnifyingglass",
        "sharpenMasking": "square.dashed", "nrLuminance": "circle.dotted.circle", "nrDetail": "magnifyingglass",
        "nrContrast": "circle.lefthalf.filled", "nrColor": "paintpalette", "nrColorDetail": "magnifyingglass",
        "nrColorSmoothness": "water.waves", "denoise": "wand.and.sparkles", "sharpness": "triangle",
        "noise": "circle.dotted.circle", "moire": "squareshape.split.3x3", "defringe": "circle.righthalf.filled",
        "distortion": "rectangle.compress.vertical", "vignetting": "smallcircle.filled.circle",
        "vignettingMidpoint": "circle.and.line.horizontal", "shadowsTint": "shadow",
        "vignette.amount": "smallcircle.filled.circle", "grain.amount": "circle.grid.3x3.fill",
        "redHue": "r.circle", "redSat": "r.circle.fill", "greenHue": "g.circle", "greenSat": "g.circle.fill",
        "blueHue": "b.circle", "blueSat": "b.circle.fill",
    ]
}
#endif
