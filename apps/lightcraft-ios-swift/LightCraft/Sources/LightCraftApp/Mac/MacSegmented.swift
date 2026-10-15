#if os(macOS)
import SwiftUI

/// A segmented choice that takes the width it is given, in the inspector's field look: equal cells in
/// a field, the chosen one lit. The system's keeps its natural width, which would widen the inspector
/// (and clip when it's narrowed). It chooses from a binding, or (a bar of modes that a second press
/// turns off) says which is on and what pressing one does.
struct MacSegmented<Value: Hashable>: View {
    let options: [(Value, LocalizedStringKey)]
    let isOn: (Value) -> Bool
    let choose: (Value) -> Void

    init(selection: Binding<Value>, options: [(Value, LocalizedStringKey)]) {
        self.options = options
        isOn = { $0 == selection.wrappedValue }
        choose = { selection.wrappedValue = $0 }
    }

    init(options: [(Value, LocalizedStringKey)], isOn: @escaping (Value) -> Bool, choose: @escaping (Value) -> Void) {
        self.options = options
        self.isOn = isOn
        self.choose = choose
    }

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options, id: \.0) { value, label in
                let on = isOn(value)
                Button {
                    choose(value)
                } label: {
                    Text(label)
                        .font(.inspector)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                        .frame(maxWidth: .infinity, minHeight: MacUI.fieldHeight - 4)
                        .background(on ? MacUI.chosen : .clear, in: .rect(cornerRadius: MacUI.radius - 2))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(2)
        .background(MacUI.fill, in: .rect(cornerRadius: MacUI.radius))
    }
}
#endif
