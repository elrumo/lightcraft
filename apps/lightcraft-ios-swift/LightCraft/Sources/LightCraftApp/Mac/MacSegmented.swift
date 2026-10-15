#if os(macOS)
import SwiftUI

/// A segmented choice that takes the width it is given. The system's keeps its natural width, which
/// would widen the inspector (and clip when it's narrowed): equal cells, the chosen one raised.
struct MacSegmented<Value: Hashable>: View {
    @Binding var selection: Value
    let options: [(Value, LocalizedStringKey)]

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options, id: \.0) { value, label in
                let on = value == selection
                Button {
                    selection = value
                } label: {
                    Text(label)
                        .font(.callout)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                        .frame(maxWidth: .infinity, minHeight: 22)
                        .background(on ? Color.primary.opacity(0.18) : .clear, in: .rect(cornerRadius: 5))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(2)
        .background(.quaternary, in: .rect(cornerRadius: 7))
    }
}
#endif
