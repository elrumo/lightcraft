#if os(macOS)
import LightCraftCore
import SwiftUI

/// A develop slider as the Mac's edit panel has it, on one row: the name, the track, the value. The
/// track is absolute (a click puts the thumb there, a drag moves it), the value is a field to type
/// in, a double click on the name or the thumb puts it back to its default, and the arrow keys nudge
/// it when it has the focus. A drag is one undo step, drafts rendering as it moves (`editing`).
struct MacSliderRow: View {
    @Environment(AppModel.self) private var model
    let spec: ControlSpec
    let value: Double
    var label: String? = nil
    let set: (Double) -> Void
    let editing: (Bool) -> Void
    let reset: () -> Void
    @State private var dragging = false
    @State private var hovering = false
    @State private var text = ""
    @FocusState private var typing: Bool
    @FocusState private var focused: Bool

    private static let thumb: CGFloat = 12

    /// A band's or region's sliders are named for it ("Red Hue", "Midtones Luminance"), and its name is
    /// already chosen above them: "Hue", "Luminance".
    private var shown: String {
        let name = label ?? spec.label
        guard spec.id.hasPrefix("mixer.") || spec.id.hasPrefix("grading."), let space = name.firstIndex(of: " ") else { return name }
        return String(name[name.index(after: space)...])
    }

    var body: some View {
        HStack(spacing: 8) {
            Text(LocalizedStringKey(shown))
                .font(.inspector)
                .foregroundStyle(hovering || dragging ? .primary : .secondary)
                .lineLimit(2)
                .minimumScaleFactor(0.85)
                .frame(width: MacUI.labelWidth, alignment: .leading)
                .contentShape(Rectangle())
                .onTapGesture(count: 2, perform: reset)
                .help(Text("Double-click to reset"))
            track
                .frame(height: 20)
            valueField
        }
        .frame(minHeight: MacUI.rowHeight)
        .onHover { hovering = $0 }
        // (the arrow keys nudge it while it has the focus, not move to the next photo: Mac/MacInput.swift)
        .onChange(of: focused) { _, on in
            if on { model.arrowsTaken = spec.id } else if model.arrowsTaken == spec.id { model.arrowsTaken = nil }
        }
        .onDisappear { if model.arrowsTaken == spec.id { model.arrowsTaken = nil } }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(LocalizedStringKey(label ?? spec.label)))
        .accessibilityValue(spec.format(value))
        .accessibilityAdjustableAction { direction in
            nudge(direction == .increment ? 1 : -1, big: true)
        }
        .accessibilityAction(named: "Reset") { reset() }
    }

    // MARK: track

    private var track: some View {
        GeometryReader { geo in
            let usable = max(geo.size.width - Self.thumb, 1)
            let x = CGFloat(fraction(value)) * usable
            let d = CGFloat(fraction(spec.default)) * usable
            ZStack(alignment: .leading) {
                trackFill
                    .frame(height: 3)
                    .clipShape(.capsule)
                    .padding(.horizontal, Self.thumb / 2)
                if spec.track.kind == "plain" || spec.track.kind == "centered" {
                    Capsule()
                        .fill(Color.accentColor)
                        .frame(width: abs(x - d), height: 3)
                        .offset(x: min(x, d) + Self.thumb / 2)
                }
                Circle()
                    .fill(.white)
                    .frame(width: Self.thumb, height: Self.thumb)
                    .overlay { Circle().strokeBorder(.black.opacity(0.35), lineWidth: 1) }
                    .shadow(color: .black.opacity(0.3), radius: 1.5, y: 0.5)
                    .scaleEffect(dragging ? 1.2 : 1)
                    .offset(x: x)
                    .animation(.snappy(duration: 0.12), value: dragging)
            }
            .frame(maxHeight: .infinity)
            .contentShape(Rectangle())
            .gesture(drag(usable: usable))
            .onTapGesture(count: 2, perform: reset)
            .focusable()
            .focused($focused)
            .focusEffectDisabled()
            .onMoveCommand { direction in
                switch direction {
                case .left, .down: nudge(-1, big: false)
                case .right, .up: nudge(1, big: false)
                @unknown default: break
                }
            }
        }
        .disabled(spec.min >= spec.max)
    }

    /// The track: grey, or the control's colours (white balance, hue, saturation…).
    @ViewBuilder private var trackFill: some View {
        if let colors = ControlSlider.trackColors(spec) {
            LinearGradient(colors: colors, startPoint: .leading, endPoint: .trailing)
        } else {
            Color.secondary.opacity(0.35)
        }
    }

    private func fraction(_ v: Double) -> Double { (v - spec.min) / spec.span }

    /// Absolute: the thumb goes where the pointer is, and follows it.
    private func drag(usable: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { g in
                if !dragging {
                    dragging = true
                    editing(true)
                }
                let f = Double(min(max((g.location.x - Self.thumb / 2) / usable, 0), 1))
                let v = spec.clamp(spec.min + f * spec.span)
                if v != value { set(v) }
            }
            .onEnded { _ in
                dragging = false
                editing(false)
            }
    }

    /// An arrow key's step: the control's own, or a hundredth of its range (five of them for a VoiceOver swipe).
    private func nudge(_ direction: Double, big: Bool) {
        let step = (spec.step > 0 ? spec.step : spec.span / 100) * (big ? 5 : 1)
        editing(true)
        set(spec.clamp(value + direction * step))
        editing(false)
    }

    // MARK: value

    /// The number, typed over to set it (⏎ applies, esc puts the shown value back).
    private var valueField: some View {
        TextField("", text: $text)
            .textFieldStyle(.plain)
            .font(.system(size: 12).monospacedDigit())
            .multilineTextAlignment(.trailing)
            .foregroundStyle(dragging ? Color.accentColor : .secondary)
            .frame(width: 46)
            .focused($typing)
            .onAppear { text = spec.format(value) }
            .onChange(of: value) { _, new in if !typing { text = spec.format(new) } }
            .onChange(of: typing) { _, isTyping in if !isTyping { text = spec.format(value) } }
            .onSubmit {
                if let v = Self.number(text) {
                    editing(true)
                    set(spec.clamp(v))
                    editing(false)
                }
                text = spec.format(value)
            }
            .onExitCommand { text = spec.format(value); typing = false }
    }

    /// "+1.25", "7,050", "-12 %": the number in a shown value.
    static func number(_ shown: String) -> Double? {
        let cleaned = shown.filter { $0.isNumber || $0 == "." || $0 == "-" }
        return Double(cleaned)
    }
}
#endif
