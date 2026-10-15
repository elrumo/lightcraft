#if os(macOS)
import SwiftUI

// The parts every panel of the Mac's inspector is made of, so that they all line up the same way: a
// header with the tool's name, groups under it separated by a rule, and in a group rows of a label
// column and a control column, with controls of one height and one look (a field the pointer lights,
// a segmented choice, a bar of icons, a checkbox). The layout is Figma's: one left edge for titles,
// labels and checkboxes, one right edge for values, a quiet field fill instead of borders, a
// 4-point rhythm.

/// The inspector's measures.
enum MacUI {
    /// The panel's padding at both sides: titles, labels and checkboxes start here, values end here.
    static let inset: CGFloat = 16
    /// The label column of a row.
    static let labelWidth: CGFloat = 88
    /// Between a label and its control.
    static let gap: CGFloat = 8
    /// A row's height, and a control's.
    static let rowHeight: CGFloat = 28
    static let fieldHeight: CGFloat = 24
    static let radius: CGFloat = 6
    /// What a field is filled with, and with the pointer on it.
    static var fill: Color { Color.primary.opacity(0.07) }
    static var hover: Color { Color.primary.opacity(0.13) }
    static var chosen: Color { Color.primary.opacity(0.18) }
}

extension Font {
    /// The inspector's text: labels, values, menus.
    static var inspector: Font { .system(size: 12) }
    /// A group's title.
    static var inspectorTitle: Font { .system(size: 12, weight: .semibold) }
}

/// A tool's panel: its name at the top with what it can do at the right, a rule, then its groups.
struct MacPanel<Actions: View, Content: View>: View {
    let title: LocalizedStringKey
    @ViewBuilder var actions: () -> Actions
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Text(title).font(.system(size: 13, weight: .semibold)).accessibilityAddTraits(.isHeader)
                Spacer(minLength: 8)
                actions()
            }
            .padding(.horizontal, MacUI.inset)
            .frame(height: 44)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 0) { content() }
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

extension MacPanel where Actions == EmptyView {
    init(title: LocalizedStringKey, @ViewBuilder content: @escaping () -> Content) {
        self.init(title: title, actions: { EmptyView() }, content: content)
    }
}

/// A group of a panel: an optional title, its rows, a rule under it.
struct MacGroup<Content: View>: View {
    var title: LocalizedStringKey?
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let title {
                Text(title).font(.inspectorTitle).padding(.bottom, 6).accessibilityAddTraits(.isHeader)
            }
            content()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, MacUI.inset)
        .padding(.vertical, 12)
        .overlay(alignment: .bottom) { Divider() }
    }
}

/// A row: its label in the label column, its control filling the rest.
struct MacRow<Control: View>: View {
    var label: LocalizedStringKey?
    @ViewBuilder var control: () -> Control

    var body: some View {
        HStack(spacing: MacUI.gap) {
            if let label {
                Text(label)
                    .font(.inspector)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .frame(width: MacUI.labelWidth, alignment: .leading)
            }
            control().frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(minHeight: MacUI.rowHeight)
    }
}

/// A sub-heading inside a group ("Upright"), as quiet as a label.
struct MacCaption: View {
    let text: LocalizedStringKey

    var body: some View {
        Text(text).font(.inspector).foregroundStyle(.secondary).padding(.top, 4).accessibilityAddTraits(.isHeader)
    }
}

/// A line of explanation under what it explains.
struct MacHint: View {
    let text: Text

    var body: some View {
        text
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 4)
    }
}

/// A choice from a menu, drawn as a field: what is chosen, a chevron; the whole field opens the menu.
struct MacMenuField<Items: View>: View {
    let text: Text
    @ViewBuilder var items: () -> Items
    @State private var hovering = false

    var body: some View {
        Menu {
            items()
        } label: {
            HStack(spacing: 4) {
                text.font(.inspector).lineLimit(1)
                Spacer(minLength: 4)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 8)
            .frame(maxWidth: .infinity, minHeight: MacUI.fieldHeight, maxHeight: MacUI.fieldHeight)
            .background(hovering ? MacUI.hover : MacUI.fill, in: .rect(cornerRadius: MacUI.radius))
            .contentShape(Rectangle())
        }
        // (the button style, not the borderless one: that draws only the label's text and icon, none of its layout)
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .onHover { hovering = $0 }
    }
}

/// An icon on a field-coloured tile: an action, or (`on`) a choice that is made.
struct MacIconButton: View {
    let symbol: String
    let title: LocalizedStringKey
    var on = false
    /// Fills the row (a bar of them) instead of being a tile of its own.
    var wide = false
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 12))
                .foregroundStyle(on ? Color.accentColor : .primary)
                .frame(width: wide ? nil : 28, height: MacUI.fieldHeight)
                .frame(maxWidth: wide ? .infinity : nil)
                .background(on ? Color.accentColor.opacity(0.22) : hovering ? MacUI.hover : MacUI.fill, in: .rect(cornerRadius: MacUI.radius))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(Text(title))
        .accessibilityLabel(Text(title))
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

/// An icon on a field-coloured tile that opens a menu.
struct MacIconMenu<Items: View>: View {
    let symbol: String
    let title: LocalizedStringKey
    var tint: Color?
    @ViewBuilder var items: () -> Items
    @State private var hovering = false

    var body: some View {
        Menu {
            items()
        } label: {
            Image(systemName: symbol)
                .font(.system(size: 12))
                .foregroundStyle(tint ?? .primary)
                .frame(width: 28, height: MacUI.fieldHeight)
                .background(hovering ? MacUI.hover : MacUI.fill, in: .rect(cornerRadius: MacUI.radius))
                .contentShape(Rectangle())
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .onHover { hovering = $0 }
        .help(Text(title))
        .accessibilityLabel(Text(title))
    }
}

/// A row of icon buttons that share the width.
struct MacIconBar: View {
    struct Item: Identifiable {
        let id = UUID()
        let symbol: String
        let title: LocalizedStringKey
        var on = false
        let action: () -> Void
    }

    let items: [Item]

    var body: some View {
        HStack(spacing: 4) {
            ForEach(items) { item in
                MacIconButton(symbol: item.symbol, title: item.title, on: item.on, wide: true, action: item.action)
            }
        }
    }
}

/// A button in the fields' look: a word (and an icon), the width of the row or its own.
struct MacButton: View {
    let title: LocalizedStringKey
    var symbol: String?
    /// The button of a form (the one to press), in the accent colour.
    var prominent = false
    /// A choice that is made (B&W on): lit.
    var on = false
    var wide = true
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                if let symbol { Image(systemName: symbol).font(.system(size: 11)) }
                Text(title).font(.inspector).lineLimit(1)
            }
            .foregroundStyle(prominent ? Color.white : on ? Color.accentColor : .primary)
            .padding(.horizontal, 10)
            .frame(maxWidth: wide ? .infinity : nil, minHeight: MacUI.fieldHeight, maxHeight: MacUI.fieldHeight)
            .background(background, in: .rect(cornerRadius: MacUI.radius))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private var background: Color {
        if prominent { return Color.accentColor.opacity(hovering ? 0.85 : 1) }
        if on { return Color.accentColor.opacity(0.22) }
        return hovering ? MacUI.hover : MacUI.fill
    }
}

/// A checkbox at the left edge.
struct MacCheck: View {
    let title: LocalizedStringKey
    @Binding var isOn: Bool

    var body: some View {
        Toggle(isOn: $isOn) { Text(title).font(.inspector) }
            .toggleStyle(.checkbox)
            .frame(minHeight: MacUI.rowHeight, alignment: .leading)
    }
}

extension View {
    /// A text field (or a button that opens one) in the fields' look: the fill, no border, the same
    /// height and corners as a menu.
    func macFieldLook() -> some View {
        textFieldStyle(.plain)
            .font(.inspector)
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .frame(maxWidth: .infinity, minHeight: MacUI.fieldHeight, alignment: .topLeading)
            .background(MacUI.fill, in: .rect(cornerRadius: MacUI.radius))
    }
}
#endif
